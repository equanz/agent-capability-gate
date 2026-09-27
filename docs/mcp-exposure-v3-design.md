# MCP ツール公開 I/F（設定 version 3）

## 位置づけ

本書は `version: 3` の MCP 公開 I/F の規範である。設定 version 2 の互換動作は [MCP ツール公開 I/F（設定 version 2）](mcp-exposure-design.md) に記す。version 2 の `inputs` / `fixed` を version 3 の `properties` として読み替えず、各設定 version を明示して扱う。

## 目的と責務

管理者設定は、どの upstream tool を公開するかと、その tool の引数にどの機械的制約をかけるかを決める。upstream MCP server は自身の tool 定義の提供元であり、broker はその定義を cache して設定済みの制約から downstream catalog を作る。

`expose` は upstream tool 名の allowlist である。MCP client 側の enabled / approved 設定はこの境界の代わりにならない。proxy は選んだ tool を upstream の定義で公開する。restriction は選んだ tool の properties に管理者設定を適用する。いずれも選ばれていない tool 名を公開せず、直接 call も upstream に転送しない。

設定による引数制約は、agent に credential や upstream の直接接続手段を渡さずに外部操作を許可する境界である。upstream の応答は引き続き信頼する。broker は tool の意味や結果を検証せず、応答内容を選別・整形しない。

## 公開設定

```yaml
version: 3
targets:
  time:
    expose:
      get_current_time: {} # upstream 定義のまま公開する proxy
      convert_time:
        as: utc_to_timezone
        description: Convert UTC time to an approved destination
        restrict:
          expose_unlisted_properties: false
          properties:
            time: {}
            target_timezone:
              enum: [Asia/Tokyo, Europe/London]
              description: Approved destination timezone
            source_timezone:
              fixed: UTC
```

version 2 と同様、`{}` は proxy、`restrict` がある定義は restriction である。proxy は upstream tool 一つの現在の定義を公開する。restriction は `properties` を使い、各 upstream property の公開、固定、除外を一箇所で定める。

restriction の tool entry にある `description` は upstream tool description を置き換え、各 property の `description` はその argument description を置き換える。説明は agent へ公開する情報であり、validation、fixed value、転送 arguments を変えない。proxy entry への description 上書きは設定エラーとする。その他の upstream metadata は現在の定義を引き継ぐ。

| 設定 | downstream での扱い |
|---|---|
| `name: {}` | upstream の property 制約を継承して公開する |
| 制約・`description` | 制約を強めた property として公開する。`description` は discovery 表示だけを置換する |
| `name: { fixed: <JSON> }` | agent には公開せず、call 時に broker が指定値を挿入する |
| `name: { omit: true }` | 公開せず、upstream arguments にも含めない |

一つの property には公開、固定、除外のいずれか一つだけを指定する。`fixed` は他の property 設定と併記しない。`omit` は `true` だけを受け付け、制約や固定値とは併記しない。`fixed` の値は JSON で表現可能な値であり、公開 schema、agent 向け診断、cache には含めない。

`restrict.expose_unlisted_properties` は省略時 `false`。false の場合、`properties` に書いた公開指定だけが agent に渡る。`fixed` と `omit` は公開されない。`properties` に書かれていない optional property も公開・転送しない。upstream が必須とする property が公開も固定もされず、除外されている場合、その tool は利用不能になる。

true の場合、upstream が現在定義しているすべての property を公開する。個別の `properties` 設定は一括公開に対する例外または追加制約となる。以後 upstream に追加された property も、定義を正常に取得できれば公開対象に含む。新しい optional property は optional、新しい required property は required として公開する。`omit: true` で required property を除外する場合は tool を block する。これは将来の property 追加も upstream に委ねる、管理者の明示的な一括許可である。upstream tool 自体の追加はこの設定に含まれず、tool allowlist に追加しない限り公開しない。

個別指定した upstream optional property は、既定では optional のまま公開する。`required: true` を指定すれば、downstream では必須にできる。省略された optional 値は upstream arguments に含めない。upstream required property は optional に弱められない。公開するか、`fixed` で満たす必要がある。`required: false` による緩和は設けない。

公開 tool 名は `as` があればその値、なければ upstream 名とする。CLI tool を含め公開名は一意でなければならず、衝突時に prefix 付与や上書きをしない。設定から判定できる衝突は設定エラーとする。

## 公開 schema と call の制約

restriction の公開 schema は upstream の `inputSchema.properties` を基に、現在の管理者設定を適用して導出する。公開 schema の root は object で `additionalProperties: false` とする。公開されていない caller property は拒否し、upstream に転送しない。

property policy の対象は input schema root の直接の child property である。nested object を別々の公開引数へ平坦化したり、nested property ごとに個別の `fixed` / `omit` を指定したりしない。公開した object property は一つの値として扱い、その nested schema を broker が安全に検証・公開できなければ tool を block する。

property に指定できるローカル制約は `enum`、数値の `minimum` / `maximum`、文字列の `minLength` / `maxLength`、線形時間で実行できる regex `pattern`、および `description` とする。対応 engine が線形時間を保証できない regex 構文は受け付けない。upstream schema とローカル制約は両方を満たす conjunction として公開 schema と call validator で強制し、upstream の制約を緩めない。制約が同時に適用できない、または validator が sound に強制できない場合は tool を block する。数値 range の逆転や enum の空の共通部分など、明らかな静的矛盾は block するが、regex など一般の制約集合が充足可能かの証明は要求しない。`required: true` は optional property を必須にする追加制約である。

call 時には採用済み catalog の公開 schema で caller arguments を検証し、公開 property の値と設定された固定値だけで upstream arguments を再構成する。未公開 property、除外 property、未知の caller property は転送しない。fixed value の上書きは受理しない。`expose_unlisted_properties: true` が許可する property も、取得した upstream schema と closed な公開 schema を経由する。

upstream root が対応する object schema でない場合、または参照、union、schema-valued `additionalProperties`、意味を変え得る未対応 keyword など broker が安全な公開 schema と call validator に写せない構造を含む場合、その tool を block する。root の `additionalProperties` が省略または `true` でも、明示された `properties` から公開 schema を導出できるなら受け入れる。`expose_unlisted_properties: true` が含むのも列挙された `properties` であり、任意の追加 key ではない。未対応 schema を理由に proxy へ切り替えない。proxy はそのまま公開する別の管理者選択である。

## 定義取得と cache

upstream server は自身の tool 定義の提供元であり、management config は公開 policy の提供元である。broker は定義取得の再利用と、upstream 接続前後に catalog を提示するために定義を cache する。cache は管理者の承認履歴でも、schema freeze でもない。cache の存在や再起動、config の変更を新しい公開許可の承認として扱わない。

broker 起動時に upstream process を開始しない。process 起動後、最初の downstream `tools/list` または公開 tool の call が来た時点で target を開始・接続し、`tools/list` から定義を取得する。取得できれば新しい定義を採用してから list response を返すか tool call を実行する。取得できない場合、`tools/list` は利用可能な cache 由来 catalog を返し、call は失敗させる。接続中は同じ定義から生成した catalog を再利用し、同じ接続世代の後続 `tools/list` や call ごとには upstream `tools/list` を実行しない。upstream の `notifications/tools/list_changed`、接続の再確立、process restart は再取得契機となる。

process startup 時は cache があれば、broker は現在の管理者設定と cache 内の upstream 定義から暫定 catalog を導出する。cache は upstream 接続前の list response と、一時不通中の catalog 表示に使える。新しい process の最初の demand では upstream へ discovery し、成功すれば current definition で暫定 catalog を置き換える。現在の設定が cached definition と合わない場合、cache を承認とみなさず暫定的に tool を block する。call は fresh discovery を試み、接続できない場合は `TARGET_UNAVAILABLE` を返す。正常取得した現在の定義がなお設定と合わない場合に限り、管理者対応が必要な block として扱う。

再取得した定義と現在の設定が整合する場合、新しい upstream 定義から新しい公開 schema と validator を導出してよい。整合しない tool は block 状態として catalog に残す。他の tool への更新は継続できる。`tools/list` では全 target の再取得結果と derived projection をまとめて一つの immutable catalog generation として構築し、原子的に採用する。再取得できなかった target は最後の catalog projection を維持する。call は refresh 後に採用した generation を捕捉し、その tool state で入力を検証し、upstream arguments を再構成する。後続 refresh が新 generation を採用しても、すでに捕捉された call state は変化しない。公開 catalog が変わったときは、採用後に downstream へ `notifications/tools/list_changed` を送る。初回の catalog 生成時には、同世代の `tools/list` response が現在値を返す。

cache には upstream tool metadata だけを保存する。cache から catalog を表示する場合も、cached public schema を承認根拠として盲目的に使用せず、現在の policy を cached upstream definition に適用して導出する。cache 書き込みに失敗しても、正常に取得した定義と catalog は process 内で使用できる。cache がない、壊れている、または対象 upstream と結び付けられない場合は cache miss として扱い、接続可能なら再取得する。

cache は upstream target ごとに隔離し、別 target の定義を誤って採用しない。永続 cache は絶対パスの `$XDG_CACHE_HOME` が設定されていればその下、そうでなければ `$HOME/.cache/mcp-boundary/catalog-v3/<target-identity>/<target-id>.json` に置く。identity は target ID、upstream command、working directory、resource limits から導出する。launch arguments は credential を含み得るため、一つでも設定されている target は arguments を hash せず、永続 cache の読み書きを行わない。environment の `inherit` または `set` が一つでもある target も、秘密値からの fingerprint を避けるため永続 cache を使わない。これらの target も broker process 内では正常取得した定義を接続世代中に再利用し、再起動後は初回 demand で upstream discovery する。launch arguments が空で、設定 environment もない target は永続 cache を再利用できる。upstream target の同一性を確認できない変更では古い定義を再利用せず、新たに discovery する。config 全体の byte hash や credential の hash を許可承認の代わりにしない。

server 起動時、または upstream が一時的に不通のときは、最後に正常取得した cache から導出できる catalog を提示してよい。ただし target に接続できない間の call は `TARGET_UNAVAILABLE` として失敗させる。cache がなく定義も取得できない場合は、管理者設定で選択された tool 名を unavailable stub として提示し、call では target 不通を返す。cache は実際の tool 実行や credential への接続性を代替しない。

config と cache、およびその親 directory を agent から書き込み不能にすることは deployment / sandbox の責務である。runtime は filesystem permission を検査しない。cache 内の upstream schema を catalog / validator の入力に採用するため、その保護と cache format の整合性検証は必要である。credential、環境変数値、fixed value、call arguments、tool result は cache に保存しない。

## upstream 変更と block

upstream の schema を取得するたび、その schema を信頼境界内の現在定義として扱い、設定済み policy と照合する。schema 変更の承認を推定する互換性判定は置かない。公開範囲は管理者設定と現在の upstream 定義の合成で決まる。

| upstream の変更 | `expose_unlisted_properties: false` | `expose_unlisted_properties: true` |
|---|---|---|
| 未記載の optional property 追加 | 非公開のまま | optional property として公開 |
| 未記載の required property 追加 | 現在の設定がその property を公開または固定していないため tool を block | required property として公開 |
| 設定で mode を指定した property が upstream から削除 | tool を block | 同左 |
| 設定済み property の schema 変更 | upstream とローカル制約を同時に強制する。明らかな静的矛盾、fixed value の不適合、または安全に強制できない場合は block | 同左 |
| 設定にない optional property の削除 | 公開影響なし | 現在の公開定義から除去 |
| 選択済み upstream tool の削除 | tool を block | tool を block |

`expose_unlisted_properties: true` は新しい property の追加を許可するため、公開 schema と validator が安全に構成できることを要する。未知 schema keyword を安全に扱えない場合はその tool を block し、proxy には fallback しない。upstream の意味や default value の変化は schema だけから検証しない。broker は upstream が返す error を受け入れ、成功を保証しない。tool output の意味検証、filtering、redaction はこの設計の範囲外である。

### blocked tool の公開

config と正常に取得した upstream 定義を整合できない場合、または tool の schema を安全に解釈できない場合、その tool は実行不可として block する。選択済み公開名を `tools/list` に残し、`inputSchema` は空の closed object とする。description は利用不能であることと理由を示し、schema がない／使えないことを agent に伝える。agent が以前の schema に基づいて古い引数を送っても、block 判定を引数検証より先に行い、upstream tool call を行わず error result を返す。

error result は `isError: true` とし、読みやすい本文および安定した機械向け field を含める。例えば、config と現在の upstream 定義の不整合は `code: ADMIN_ACTION_REQUIRED`、`reason: PROPERTY_NOT_IN_UPSTREAM`、`retryable: false`、`action: CONTACT_ADMIN` とする。fixed value や credential は含めない。本文にも、再試行を止めて公開 tool 名と理由を管理者へ報告するよう促す。これは agent が人へ確実に page する保証ではなく、監督 process による新たな page 機構も要求しない。

reason は `PROPERTY_NOT_IN_UPSTREAM`、`REQUIRED_PROPERTY_UNSATISFIED`、`FIXED_VALUE_INVALID`、`SCHEMA_CONSTRAINT_CONFLICT`、`UNSUPPORTED_SCHEMA`、`TOOL_NOT_FOUND` など安定した分類を使う。公開名の衝突は設定検証時に拒否し、runtime blocked reason にはしない。エラーに caller value や fixed value を含めない。

error class は混同しない。

| 状態 | `tools/list` | call の結果 |
|---|---|---|
| fresh upstream schema と policy の不整合 | blocked stub を同じ名前で表示 | `ADMIN_ACTION_REQUIRED`。upstream を呼ばない |
| upstream が一時不通 | cache 由来 catalog を表示。cache がなければ unavailable stub | `TARGET_UNAVAILABLE`。再試行可能として示す |
| upstream が tool call を拒否 | 現在の schema/catalog を維持 | upstream call failure を返す。config mismatch と断定しない |
| config で選択していない tool 名 | 公開しない | 未知 tool として扱い、upstream を呼ばない |

cached metadata と現在の policy が合わないが fresh discovery が未完了の場合、その古い cache だけで管理者向け mismatch と確定しない。call は fresh discovery を試み、接続不能なら `TARGET_UNAVAILABLE`、取得済みの現在定義でも不整合なら `ADMIN_ACTION_REQUIRED` を返す。

block / unblock、公開 schema、description、または公開 tool 集合が変わるときは、新しい catalog generation を採用してから `list_changed` を通知する。agent は通知を受け取れない、または無視することがある。古い catalog から blocked tool を call されたときも、同じ tool name で理由付き error を返す。

## 同時更新と security boundary

catalog は immutable generation の `Arc` を持ち、全 target の candidate projection を完成させてから一度の swap で採用する。`tools/list` はその swap と downstream response / `list_changed` 通知を一つの sequence として直列化する。call も refresh と tool state capture を直列化し、捕捉した immutable state を入力検証から arguments 再構成まで使う。公開 request handler は並行実行されるため、この serialization が更新途中の generation の観測や、古い list response が新しい `list_changed` より後に届く状態を防ぐ。in-flight call は捕捉時の generation で完了し、採用後に受け付ける call は新 generation を使う。更新後に旧 tool definition から送られた call も現在の generation で解決し、現在 blocked なら実行せず理由を返す。

broker は tool allowlist、公開 schema、ローカル制約、固定値を machine-enforced boundary とする。agent に upstream credentials や upstream process の直接アクセスを与えない。`tools/list` の metadata と error に credential / fixed value を含めない。cache/config の OS 権限分離は deployment が保証し、cache への書き込み権限を持つ主体を信頼する。

## 受入条件

version 3 の実装は、少なくとも次を満たす。

1. version 2 config を version 3 として暗黙に読むことがなく、version 3 config では `{}` proxy と `restrict` の挙動が一意に決まる。
2. `properties` で公開、制約追加、description、fixed、omit を一箇所で指定でき、同一 property の矛盾した mode を拒否する。
3. unlisted の既定は非公開である。true の場合だけ新しい upstream property を取り込み、optional / required の必須性を正しく公開する。tool allowlist は拡張しない。
4. optional property は optional のまま公開でき、`required: true` で必須化できる。required upstream property を optional 化せず、隠すなら fixed value で満たす。
5. closed public schema が未知の caller property を拒否し、upstream arguments は許可された値と fixed value だけを含む。追加制約は upstream constraint を広げない。
6. upstream schema が未対応または policy と整合しない場合、tool を stub として list に残し、call を upstream に送らず安定した error code/reason を返す。proxy fallback は起きない。
7. 定義変更を採用する前に新しい catalog generation を完全に構築し、採用後にだけ `list_changed` を送る。list/call が世代をまたいで混在しない。
8. 同じ接続 generation の各 downstream list/call のたびに upstream `tools/list` を繰り返さず、cache miss、初回需要、再接続、upstream notification で定義を更新できる。configured environment または nonempty launch arguments を持つ target は秘密値の fingerprint を保存せず、永続 cache miss として初回 demand で再取得する。
9. upstream 不通時は broker startup を妨げず、stale catalog を表示し得るが call は `TARGET_UNAVAILABLE` とする。fresh metadata の policy mismatch、transient connection failure、upstream call rejection を区別する。
10. credential、launch arguments、environment value とその deterministic fingerprint、fixed value、call arguments、result を cache や診断に出さず、cache/config の書き込み保護を runtime permission check にすり替えない。
