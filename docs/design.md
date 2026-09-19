# MCP Capability Boundary 設計

## 目的

本書では対象システムを仮称 MCP Capability Boundary と呼ぶ。製品名は本設計の一部としない。

MCP Capability Boundary は、LLM に任意の CLI または upstream MCP server の権限を直接渡さず、管理者が明示した能力だけを別の MCP server として公開する。

本システムはツールの説明を改善するだけの wrapper ではなく、LLM から実行対象までの間に置かれる強制可能な trust boundary である。公開される入力、サーバーが補う固定値、呼び出せる実行対象、および結果として返せるデータ形式を設定から決定し、呼び出し時にも同じ制約を検証する。

## スコープ

MVP は次を対象とする。

- MCP client に対してツールを公開する STDIO server
- macOS および Linux の POSIX process model
- ローカル CLI を shell を介さずに起動する target
- STDIO で接続する upstream MCP server
- MCP 2026-07-28 と legacy initialize 世代の自動判定
- JSON Schema 2020-12 の限定された部分集合による入力検証
- 入力から CLI の argv または upstream MCP の arguments への明示的な写像
- timeout および出力量の上限

次は MVP の対象外とする。

- 公開側および upstream 側の Streamable HTTP
- HTTP の認証、複数利用者、および tenant ごとの認可
- CLI の `--help` から生成した定義の自動公開
- 任意コード、任意式、CEL、Rego による policy 記述
- 対話的 CLI、TTY、および長時間存続する子プロセス
- Windows process の起動、終了、および path semantics
- MCP の prompts、resources、sampling、および elicitation の中継
- ツール出力に含まれる自然言語を安全な命令として保証すること
- target 出力の property 射影、allowlist、redaction、および内容に基づく機密情報の除去
- JSON Schema では表せない path、URL、resource identifier などの意味検証
- optional input、および入力の有無や値によって argv 構造を切り替える conditional binding
- 設定可能な同時実行制御、待機 queue、および監査 sink
- 自律開発環境、sandbox、および修正ループの設計

Streamable HTTP は、遠隔利用または複数利用者が必要になり、identity、credential、認可、Origin 検証、および接続管理を同時に設計できる段階で再検討する。出力成形は、公開してよい出力を upstream の契約だけでは限定できない target を扱う段階で再検討する。意味検証は、列挙、型、長さ、および pattern だけでは安全な入力集合を表現できない具体的な公開ツールが必要になった段階で再検討する。optional input は、承認上の意味を変えない自己完結した option が複数の実用例で必要になった段階で再検討する。同時実行制御と監査は、複数の継続利用者、負荷分離、または事後追跡が運用要件になった段階で再検討する。自律開発環境と sandbox は、本書に定める E2E 振る舞い、プロパティ要件、および I/F が確定した後に別の設計として扱う。

## 用語

- **公開ツール**: MCP client が `tools/list` で発見し、`tools/call` で呼び出すツール。
- **target**: 公開ツールの処理を実行する CLI または upstream MCP server。
- **入力**: MCP client が公開ツールへ渡す `arguments`。
- **固定値**: 設定だけが保持し、入力では変更できない値。
- **binding**: 検証済みの入力または固定値を target の引数へ写像する宣言。
- **解決済み呼び出し**: executable、argv、環境、作業ディレクトリ、または upstream tool と arguments が確定した内部表現。

## システム境界

```text
MCP client / LLM
        |
        | tools/list, tools/call
        v
+------------------------------------+
| MCP Capability Boundary            |
|                                    |
| 公開schema -> 入力検証               |
|             -> binding -> 上限適用  |
+----------------+-------------------+
                 |
          解決済み呼び出し
                 |
        +--------+---------+
        |                  |
        v                  v
  local CLI          upstream MCP
  (argv execution)   (STDIO client)
```

設定の管理者は、公開する能力と target に渡す権限を決定する。LLM は公開ツールの入力だけを制御する。CLI と upstream MCP の実装および挙動は信頼境界内に置く。ただし、その出力は LLM に対する命令として信頼せず、server の診断や認可判断とも区別する。

公開ツールの schema や annotation は LLM の選択を助けるが、安全性は LLM または MCP client がそれらを守ることに依存しない。全入力は server 側で再検証される。

## 脅威前提

LLM とその生成した引数は非信頼とする。target は設定で選択した信頼済みの実行主体とし、本システムは target の挙動を監視または封じ込めない。target が返す content は、LLM に対する命令、server の診断、または認可判断としては信頼しない。

設定を変更できる主体は、本システムが公開する全能力を変更できる管理者である。server process の実行 user、OS kernel、および設定で指定した executable を置換できる主体も同等に信頼する。本システムは、host の管理権限を奪取した攻撃者、差し替えられた target binary、または設定外の経路から直接呼ばれた upstream を封じ込めない。

upstream MCP に credential を渡す場合、その upstream は credential が持つ権限を行使できる。本システムが狭めるのは LLM から到達できる呼び出し面であり、upstream 自身の実装権限ではない。MVP は target の返却内容を property 単位で成形しないため、target が返した機密情報の公開を一般には防止できない。管理者は、返却内容を信頼できる target と tool だけを公開する。

通常利用ではbrokerをCodexのMCP server設定へ登録し、その起動contextがbrokerとtargetのOS権限を決める。自律開発用のouter sandboxは実装とmock検証を閉じ込めるための別境界であり、通常利用時のtargetを同じsandboxへ閉じ込める製品機能ではない。本システムは両者を同一の安全性保証として扱わない。

## E2E の振る舞い

### 起動

1. server は設定全体を読み、設定 version、target、公開ツール名、schema、binding、および参照整合性を検証する。
2. server は各公開ツールについて、受理した schema 部分集合から MCP の `inputSchema` を生成する。
3. server は全 binding が宣言済み入力または server 所有値だけを参照することを確認する。
4. 一つでも不正な定義があれば、server は MCP の受け付けを開始せず、設定位置を含む診断を stderr に出して失敗する。
5. 正常な設定は、同じ内容から常に同じ順序の公開ツールと schema を生成する。

CLI や upstream MCP の一時的な不通は、設定自体が検証可能なら server 全体の起動を妨げない。呼び出し時に target 固有のエラーとして返す。ただし、CLI および upstream MCP 起動 command の executable が存在しない、固定された作業ディレクトリが存在しない、または継承を宣言した環境変数が存在しないなど、静的に判定できる誤設定は起動失敗とする。

### ツール発見

`tools/list` は設定で宣言した公開ツールだけを返す。元の CLI の subcommand、flag、および upstream MCP の未選択ツールは公開しない。

公開 `inputSchema` には LLM が指定できる入力だけを含める。固定値、credential、executable、作業ディレクトリ、環境変数名、および upstream 接続情報は含めない。全 object schema は `additionalProperties: false` とし、宣言した全 property を必須にする。

一つの公開toolが受理する全入力は、承認上同じ種類の能力でなければならない。入力値によってreadとwrite、非破壊と破壊、または異なるcredential権限へ切り替わる操作は別々の公開toolに分け、それぞれの名前、説明、およびannotationを全入力に対して正しくする。MVPは操作の意味を推論してこの条件を自動判定しないため、管理者によるcatalog reviewの条件とする。

upstream MCP の `tools/list` が変化しても、公開ツール集合または公開 schema は自動的に拡張されない。設定と互換でなくなった変更は、そのツールの呼び出しを安全側に失敗させる。

公開catalogは最大256件を一つの`tools/list` resultで返し、paginationとlist-changed notificationを提供しない。`cursor`を持つ`tools/list`はunsupported paginationとしてprotocol errorにする。

### CLI の正常呼び出し

1. server は `tools/call` の tool 名を公開ツール集合から解決する。
2. 入力を公開 `inputSchema` で検証する。
3. binding を評価し、executable、argv、環境、作業ディレクトリ、および実行上限を含む解決済み呼び出しを生成する。
4. server は shell を介さず、executable と argv 配列を OS の process API に渡し、CLI の stdin を直ちに閉じる。
5. stdout と stderr を別々に、設定された上限まで収集する。
6. 終了 status と output 設定に従って MCP の tool result を生成する。

入力由来の文字列は一つの argv 要素として渡される。空白、引用符、`;`、`|`、`$()` などを含んでも新しい argv 要素や shell 構文にはならない。許可する文字列集合は schema の `enum`、長さ、および `pattern` で決める。

### CLI の拒否と失敗

- 未知の tool、未知の入力 property、および型または値制約への不一致は、process を起動する前に拒否する。
- timeout または cancellation では、server が起動した process group 全体の終了を試み、結果を返す前に reap する。
- stdout または stderr が上限に達した場合は process を終了し、切り詰めた内容を正常結果として扱わない。
- exit code 0 だけを成功とし、非 0 または signal による終了は target failure とする。
- `output.kind: json` で stdout 全体を一つの JSON value として parse できない場合は target output failure とする。
- エラーには credential、server 所有の機密固定値、全環境、または非公開 argv を含めない。

### upstream MCP の正常呼び出し

1. server は入力を CLI target と同じ順序で検証する。
2. target の upstream process がなければ起動し、modern discovery または legacy initialize による protocol negotiation を完了して、同じ broker process 内の後続呼び出しで再利用する。
3. binding を評価し、設定で固定された upstream server、upstream tool 名、および arguments を生成する。
4. 明示的に写像された property と固定値だけを upstream に送る。
5. upstream result を output 上限の内側で受信する。
6. 対応する content 形式であることを確認して結果を公開する。

公開ツールから upstream の任意 tool 名を指定する入力は提供しない。入力 object 全体の暗黙 passthrough も行わない。

upstream client は最初に `server/discover` を試し、modern protocol を示す肯定的な応答を得た場合だけ MCP 2026-07-28 を採用する。それ以外は legacy `initialize` handshake へ fallback する。discovery の拒否後も接続が利用可能なら同じ process を使い、接続または process が終了した場合は一度だけ新しく起動して legacy handshake を行う。negotiation が完了するまで tool request は送信しない。negotiation の結果は接続内で固定し、同じ tool request を両方式で送らない。MVP の設定 I/F に protocol revision の選択項目は設けない。

### upstream MCP の拒否と失敗

- upstream tool が存在しない場合、arguments が拒否された場合、通信が失敗した場合、および timeout は target 固有の失敗として返す。MVP は公開 schema と upstream schema の互換性を事前判定しない。
- upstream が未対応または protocol 上不正な content を返した場合、raw result を公開せず target output failure とする。
- upstream が返す annotation、description、およびエラー文は認可判断に使わない。
- upstream がserver request、`input_required` result、またはmulti-round-trip継続を要求した場合は実行せず、同じ公開callをtarget output failureにする。
- upstream の `tools/list` 変更通知は再検査の契機にはできるが、公開能力を自動変更しない。
- upstream process が異常終了した呼び出しは失敗とし、次の呼び出しでは新しい process を起動できる。request を upstream へ送信した後は、同じ公開呼び出しを自動 retry しない。
- 一つの upstream target へは一度に一件だけ送信する。timeout または cancellation では protocol revision が対応する cancellation を通知し、短い終了猶予の後も処理が続く場合は upstream process を終了する。次の呼び出しは新しい process で処理する。

### upstream MCP の lifetime

upstream process の lifetime は、それを所有する broker process を超えない。broker は終了時に自身が起動した upstream process を終了して回収する。独立 daemon として upstream を共有しない。

process の再利用は broker による実装上の所有であり、protocol session の存在を前提としない。MCP 2026-07-28 では各 request を独立に扱い、legacy protocol では negotiated connection の範囲だけで session state を保持する。

MVP の STDIO 接続は認証済みの Agent identity を持たないため、同じ broker process を共有する呼び出しを同じ trust domain として扱う。Agent ごとの状態または credential の分離が必要な構成では、起動主体が Agent ごとに broker process を分ける。将来、broker が認証済み principal を識別する場合は、principal ごとの upstream session 所有を再検討する。

### 結果とエラー

malformed MCP envelope、未知のtool、およびbrokerの容量枯渇はprotocol-level errorとする。公開schemaに対する入力不正とpolicy拒否はtargetを起動せず、LLMが修正可能な`isError: true`のtool resultとする。targetの起動後に起きる失敗も`isError: true`のtool resultとする。

実装は少なくとも次の安定した内部 error code を持つ。MCP の protocol version に応じた wire 表現は adapter が決める。

- `INVALID_ARGUMENTS`
- `POLICY_DENIED`
- `TARGET_UNAVAILABLE`
- `TARGET_TIMEOUT`
- `TARGET_FAILED`
- `OUTPUT_LIMIT_EXCEEDED`
- `INVALID_TARGET_OUTPUT`
- `CANCELLED`
- `SERVER_BUSY`

LLM が修正可能な入力エラーには、公開 property 名と違反した公開制約を含められる。server 所有の値と target の内部構成は含めない。

## プロパティ要件

### 能力の非拡張性

1. **P01 明示公開**: 設定にない tool、引数、target、および credential は LLM から利用できない。
2. **P02 upstream 非連動**: CLI または upstream MCP の機能追加は、設定変更なしに公開能力を増やさない。
3. **P03 未知入力の無影響**: schema にない入力は拒否され、解決済み呼び出しへ影響しない。
4. **P04 固定値の優越**: 固定値と同じ名前または意味を持つ入力を渡しても、固定値を上書きできない。
5. **P05 target 固定**: executable、upstream server、および upstream tool は LLM 入力から選択できない。

### 決定性

6. **P06 同値入力の同値解決**: 同じ設定と同じ有効入力は、環境から取得する明示的な server 所有値を除き、同じ解決済み呼び出しを生成する。
7. **P07 順序の安定**: `tools/list` と argv の順序は、map の列挙順や upstream の返却順に依存しない。
8. **P08 部分実行なし**: 検証または binding が失敗した呼び出しは target を一度も起動しない。

### 実行分離

9. **P09 shell・stdin 非使用**: CLI target は shell command string として実行せず、stdin を直ちに閉じる。
10. **P10 環境 deny-by-default**: child process へ渡す環境は空の状態から、設定で明示した固定値と継承変数だけで構成する。
11. **P11 作業領域固定**: 作業ディレクトリは設定で決まり、入力から変更できない。
12. **P12 有限入力**: MCP message、JSON depth、string、array、設定ファイル、tool 数、pattern、および同時呼び出し数に有限の上限を持ち、上限超過を target 起動前に拒否する。
13. **P13 有限実行**: 各呼び出しは timeout、stdout、および stderr の上限を持つ。
14. **P14 process 所有**: CLI 呼び出しの完了時にその process を回収し、upstream process の lifetime は broker process を超えない。一つの upstream target は一件ずつ処理し、timeout または cancellation で終了しない process を終了する。送信済みの upstream request は自動 retry しない。

### 検証の一貫性

15. **P15 単一意味**: `tools/list` で公開する schema と server が呼び出し時に強制する型制約は、同じ正規化済み定義から生成する。
16. **P16 設定の原子的採用**: 設定は全体が有効な場合だけ採用し、旧定義と新定義を混在させない。

### 機密性と結果の分離

17. **P17 server 所有機密値の非公開**: server が生成する `tools/list`、診断、およびエラーに、credential と機密固定値を含めない。target 自身が同じ値を出力した場合の検出と除去は保証しない。
18. **P18 target 出力の識別**: target の stdout、stderr、および upstream content は、server の診断や policy 判断と混同しない。

### トランスポート非依存性

19. **P19 意味論の一致**: 同じ identity、設定、および入力に対して、公開側 transport または negotiated protocol revision の違いは検証結果と解決済み呼び出しを変えない。
20. **P20 protocol・transport adapter の局所化**: modern discovery、legacy initialize、STDIO、および将来の HTTP に固有の lifecycle、framing、header、および認証処理は、policy と target 実行から分離する。

## 設定 I/F

設定は YAML 1.2 で表現し、JSON はその互換な入力として受理する。duplicate key、custom tag、および alias は拒否する。設定 root は version、server、targets、および tools から成る。

```yaml
version: 1

server:
  name: home-ops
  transport:
    kind: stdio
  limits:
    request_bytes: 1048576
    json_depth: 32

targets:
  kubectl-home:
    kind: cli
    executable: /opt/homebrew/bin/kubectl
    cwd: /Users/example/src/home-infra
    environment:
      inherit: [KUBECONFIG]
      set:
        LC_ALL: C
    limits:
      timeout_ms: 30000
      stdout_bytes: 1048576
      stderr_bytes: 65536

  source-control:
    kind: mcp
    transport:
      kind: stdio
      command: /usr/local/bin/source-control-mcp
      args: [serve]
      cwd: /Users/example/src/home-infra
      environment:
        inherit: [SOURCE_CONTROL_TOKEN]
        set:
          LC_ALL: C
    limits:
      timeout_ms: 30000
      output_bytes: 1048576
      stderr_bytes: 65536

tools:
  get_kubernetes_resources:
    title: Get Kubernetes resources
    description: Read a permitted Kubernetes resource in the fixed cluster.
    annotations:
      readOnlyHint: true
      destructiveHint: false
      idempotentHint: true
      openWorldHint: true
    input_schema:
      type: object
      additionalProperties: false
      properties:
        resource:
          type: string
          enum: [pods, deployments, services]
        namespace:
          type: string
          minLength: 1
          maxLength: 63
          pattern: "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$"
      required: [resource, namespace]
    invoke:
      target: kubectl-home
      cli:
        argv:
          - literal: --context
          - literal: home
          - literal: get
          - input: /resource
          - literal: --namespace
          - input: /namespace
          - literal: --output=json
    output:
      kind: json

  list_private_repositories:
    title: List private repositories
    description: List repositories owned by the configured organization.
    input_schema:
      type: object
      additionalProperties: false
      properties:
        limit:
          type: integer
          minimum: 1
          maximum: 100
      required: [limit]
    invoke:
      target: source-control
      mcp:
        tool: list_repositories
        arguments:
          organization:
            literal: example-org
          visibility:
            literal: private
          limit:
            input: /limit
    output:
      kind: structured
```

`version` は設定 I/F の互換性を表し、MCP protocol version とは独立する。未知の field は起動時に拒否する。公開tool名とtarget IDは1文字以上128文字以下とし、ASCIIの英数字、underscore、hyphen、およびdotだけを受理する。executable と upstream MCP の `command` は absolute path とし、`PATH` による探索や shell alias の解決を行わない。

CLI と upstream MCP の process は absolute `cwd` を必須とする。child environment は空の状態から構成し、`environment.inherit` に列挙した変数と `environment.set` の固定値だけを渡す。環境変数名はASCIIの英字またはunderscoreで始まり、続きがASCIIの英数字またはunderscoreであるものに限定する。`PATH`、`HOME`、locale、および credential も暗黙には継承しない。`inherit` に列挙した変数が broker 起動環境に存在しなければ、設定の採用を拒否する。process APIへ渡る設定文字列にU+0000が含まれる場合も拒否する。

targetの`limits`は必須とする。`timeout_ms`は1以上300000以下、CLIの`stdout_bytes`とupstream MCPの`output_bytes`は1以上16777216以下、`stderr_bytes`は1以上1048576以下とする。終了猶予とbroker全体の同時呼び出し上限は設定から変更できない。

### 入力 schema

MVP は JSON Schema 2020-12 の次の keyword を受理する。

- `type`: `object`、`array`、`string`、`integer`、`number`、`boolean`、`null`
- `properties`、`required`、`additionalProperties: false`
- `items`、`minItems`、`maxItems`、`uniqueItems`
- `enum`
- `minimum`、`maximum`
- `minLength`、`maxLength`、`pattern`

`default` は MVP では受理しない。server が常に補う値は入力 property にせず、`literal` binding として明示する。`format` は validator によって挙動が異なるため受理しない。

公開toolの`input_schema` rootは必ず`type: object`とする。引数を持たないtoolも、空の`properties`と空の`required`を持つ閉じたobjectとして表す。nested schemaでは上記の全typeを利用できる。

全 object schema は `properties` の全 key を `required` に列挙しなければならない。入力の有無によって呼び出し構造または承認上の意味が変化することを避けるため、optional property は受理しない。

MVP は `const`、`$ref`、`$defs`、`oneOf`、`anyOf`、`allOf`、`not`、`if`、`then`、`else`、任意の `additionalProperties` schema、および外部 schema 参照を拒否する。複数の入力形状を持つ一つの公開操作が実際に必要になった場合は、まず別々の公開ツールへ分割する。分割によって操作の意味または固定値が不自然に重複する場合に、discriminator property が `const` を持つ branch だけで構成された `oneOf` を再検討する。定義の反復による誤りが増えた場合は、`$defs` とローカル `$ref` を再検討する。

### 入力 resource 上限

MVP は一つの MCP message と一つの設定ファイルをそれぞれ 1 MiB、JSON の入れ子を 32、公開 tool 数を 256、一つの string を UTF-8 で 256 KiB、一つの array を 1024 要素、`pattern` の UTF-8 表現を 16 KiB、regex engine の compiled size を 1 MiB までに制限する。schema の `maxLength` と `maxItems` は、対応する全体上限とは独立して適用する。`server.limits` は message byte と JSON depth をこの上限より小さくできるが、大きくできない。上限超過は target 起動前に拒否する。

同時に実行できる公開 `tools/call` は broker process 全体で 16 件までとする。CLI target はこの範囲で並行実行できるが、一つの upstream MCP target は一件だけ実行する。上限到達時または使用中の upstream target に対する追加呼び出しは待機 queue に入れず、target を起動せずに `SERVER_BUSY` として拒否する。この上限は MVP では設定できない。

message byte 上限は JSON decode より前に強制する。採用する MCP framework が decode 前の上限を設定できない場合は、STDIO transport adapter が上限付きで一メッセージを読み取ってから framework に渡す。

`pattern` は線形時間を保証する正規表現 engine で評価する。その engine が対応しない構文は設定採用時に拒否し、別の正規表現 engine へ fallback しない。JSON Schema の `pattern` と同じく部分一致として評価する。

MVP の `integer` は符号付き 64 bit の範囲に限定する。`number` は finite な IEEE 754 binary64 として表現できる値に限定し、overflow、NaN、および infinity を拒否する。CLI argv へ変換する `integer` は不要な符号や先頭 zero を持たない十進表記、`number` は同じ binary64 値へ round-trip できる最短の JSON number 表記とし、負の zero は `0` に正規化する。schema の数値境界と `enum` にも同じ表現域を適用する。

### バインディング

binding は任意の文字列テンプレートを使わず、型付けされた node の列として表現する。

CLI argv node は次を持つ。

- `literal`: 設定所有の単一 argv 要素
- `input`: JSON Pointer で参照する必須 scalar 入力由来の単一 argv 要素
- `each`: 必須 scalar 配列の各要素を別々の argv 要素として出力

`input` は object、array、boolean、または null を暗黙に文字列化しない。`each` は入れ子配列を受理しない。string、integer、および number から argv への変換規則は設定 version ごとに固定する。CLI argvへ写像されるstringにU+0000が含まれる場合は、process APIへ渡す前に入力不正として拒否する。入力値から option 名、argv の再分割、条件分岐、または新たな binding node を生成しない。配列が空であってはならない呼び出しは、schema の `minItems` で制約する。

upstream MCP arguments は property ごとに次の value source を明示する。

- `literal`: 設定所有の JSON value
- `input`: 入力中の JSON value
- `environment`: allowlist 済みの server 所有環境変数
- `object`、`array`: 上記 source から再帰的に構成する container

公開入力 object 全体を upstream arguments として暗黙に転送する機能は提供しない。

### クレデンシャル

credential は入力または通常の固定値に直接記述しない。credential とその権限は、broker process またはその背後の target を起動する主体が所有する。MVP では、起動主体が broker に与えた環境変数のうち target が明示的に許可したものだけを渡すか、target が自身の起動 identity から credential を取得する。broker が扱う値は schema、設定診断、および通常のエラーから除外する。

利用者ごとの認証、delegation、credential の更新、または broker と target で異なる identity が必要になった場合は、credential provider と認証フローを一体として再検討する。provider の採用時も、LLM は provider、credential 名、scope、または値を選択しない。

## 公開 MCP I/F

server は MCP の tools capability だけを公開する。公開ツール名は設定内で一意であり、upstream server 名に依存しない安定した識別子とする。

MVP は MCP 2026-07-28 の modern request と、少なくとも MCP 2025-11-25 の legacy `initialize` handshake を受理する。接続または最初の request から protocol 世代を自動判定し、その差を MCP adapter で吸収する。追加の旧 revision は採用する MCP SDK が同じ境界を保ったまま提供できる場合に限り相互運用できるが、製品の互換性保証には含めない。

modern client には `server/discover` と request ごとの metadata に基づいて応答し、legacy client には `initialize`、`initialized`、および negotiated protocol version に基づいて応答する。どちらの場合も同じ正規化済み catalog、入力検証、binding、および result policy を使用する。旧版互換のために入力検証または policy を弱めない。

公開serverはcatalogが不変であるため`tools.listChanged`をadvertiseしない。`tools/list`は全件を返して`nextCursor`を付けない。`tools/call`の`inputResponses`、`requestState`、またはMVPが公開しないmulti-round-trip fieldは受理せずprotocol errorにする。成功とtool errorはmodern protocolでは`resultType: complete`を持ち、legacy protocolではnegotiated revisionが定める形へadapterが変換する。

MCP annotation は client の表示や承認判断を助けるために設定から転記するが、server の認可判断には使わない。設定で指定していない upstream annotation は継承しない。

MVP の CLI output は UTF-8 text または stdout 全体が一つの JSON value である structured output とする。`output.kind: text`はstdoutを変更せず一つのtext content blockへ入れる。`output.kind: json`はstdout全体をparseし、structured contentへ入れるとともに、同じ値を決定的なcompact JSONへserializeした一つのtext content blockをlegacy互換のために返す。compact JSONはobject keyをUTF-8 byte列の昇順へ再帰的に並べ、不要な空白を持たず、数値をparse済みの同じ値へround-tripする最短表記にする。stdout と stderr は結合しない。exit code 0 だけを成功とする。stderrはprocess制御とoperator診断のために上限内で収集する。通常はtargetが返した生のstderrをtool resultまたはbroker生成診断へ含めないが、`MCP_BOUNDARY_LOG=debug`時のoperator eventには安全な`utf8`/`hex`表現で含める。

MVP の upstream MCP output は text content と JSON の structured content だけを中継する。MCP targetの`output.kind: text`は一つ以上のtext contentだけを許し、structured contentを許さない。`output.kind: structured`はstructured contentを必須とし、互換用のtext contentを併存できる。CLI targetでは`text`と`json`、MCP targetでは`text`と`structured`だけを設定採用時に受理する。image、audio、resource link、および embedded resource は未対応として拒否する。対応形式を増やす場合も、形式ごとの size、URI、および content validation を先に定義する。

複数の text content block は順序を保ち、structured content は接続で採用した protocol revision が許す JSON 形状のまま公開する。upstream の `isError` は成功へ変換しない。upstream result の `_meta` は公開側へ転送しない。対応済み content と未対応 content が混在する result は全体を失敗とし、一部分だけを公開しない。upstream notification は公開 catalog、binding、または policy を変更しない。

CLI JSONとupstream structured contentはoutput byte上限に加えて、JSON depth 32、string 256 KiB、およびarray 1024要素の構造上限を満たさなければならない。JSON numberは符号付き64 bit整数、符号なし64 bit整数、またはfiniteなIEEE 754 binary64のいずれかへparseできる値に限定し、表現域を超える値は不正なtarget outputとする。これらはbroker自身の有限処理を保つための形状制約であり、property単位の公開可否判断やredactionは行わない。

## 管理 CLI I/F

管理 CLI は設定検証と server 起動を分ける。

```text
mcp-boundary check  --config <absolute-path>
mcp-boundary tools  --config <absolute-path> --format json
mcp-boundary serve  --config <absolute-path>
```

`check` は target を呼び出さず、parse、schema 部分集合、binding、参照、absolute path、および静的 filesystem 条件を検証する。成功時は終了 code 0、失敗時は非 0 とし、機械可読な診断 code、設定内の path、および説明を stderr に出す。

`tools` は target を呼び出さず、`tools/list` に相当する正規化済み公開定義を JSON で出力する。credential 値と非公開 target 設定は出力しない。設定 review と snapshot test はこの出力を基準にできる。

`serve` は `check` と同じ検証を通過した設定だけを採用し、STDIO 上で MCP server を開始する。stdout は MCP message 専用とし、server の診断は stderr へ出す。MVP は hot reload を行わない。

公開STDIOの入力がEOFになった場合、またはbrokerがSIGINTもしくはSIGTERMを受けた場合、`serve`は新規callの受付を停止し、実行中のCLIとupstream processを通常の終了経路で終了・回収してから終了する。終了猶予内に回収できなければ安全な診断をstderrへ出し、runtime failureとして終了する。

`--config`はabsolute pathだけを受理する。管理CLIのexit codeは、成功を0、command line usage errorを2、設定不正を3、`serve`開始後のbroker runtime failureを4とする。target固有のtool errorは`serve` process自体のexit codeを変更しない。

診断はstderrへ一行一JSON objectで出し、少なくとも`code`、`message`、`config_path`、設定内のJSON Pointerである`path`を持つ。syntax位置を特定できる場合は1始まりの`line`と`column`を加える。fieldが適用できない診断では省略し、空文字や仮の値を入れない。

`tools --format json`はMCP envelopeを付けず、`{"tools":[...]}`をUTF-8 JSONとしてstdoutへ一つ出し、末尾にnewlineを一つ付ける。tool配列とobject keyは正規化済みの安定順を使う。`check`と`tools`はstdoutへ診断を出さず、`serve`以外はMCP messageを出さない。

## 内部 I/F

主要 component は protocol 固有の値ではなく、次の内部境界で接続する。

```text
McpAdapter
  -> CapabilityCatalog.list(caller_context)
  -> CapabilityExecutor.call(caller_context, tool, arguments, cancellation)

CapabilityExecutor
  -> InputValidator
  -> InvocationResolver
  -> TargetExecutor
  -> ResultAdapter

TargetExecutor
  -> CliExecutor | McpExecutor
```

`InvocationResolver` の結果は immutable な tagged union とする。

```text
ResolvedInvocation =
  | CliInvocation {
      target_id, executable, argv, cwd, environment, limits
    }
  | McpInvocation {
      target_id, upstream_tool, arguments, limits
    }
```

検証前の入力を `TargetExecutor` に渡せる呼び出し経路を作らない。テストは解決済み呼び出しを観測できるが、credential 値を含む完全な environment は観測対象にしない。

`caller_context` は adapter が正規化した protocol 世代と version、相関 ID、および将来の認証済み principal を保持する。modern の request metadata と legacy の connection metadata はここへ正規化する。MVP の STDIO では principal を持たず、MCP の自己申告 `clientInfo` を認可 identity として扱わない。

## 設計判断

### 設定は raw command ではなく capability を表す

raw command の allowlist や禁止文字検査だけでは、flag の別表記、subcommand の追加、CLI version 差、および意味的に危険な値を管理しきれない。設定は許可する公開操作から target 呼び出しへの写像を表し、CLI の文法全体を再現しない。

### 入力制約は限定した JSON Schema で表す

JSON Schema は LLM と client に入力形状を伝え、server が同じ型制約を強制する共通 I/F として使う。MVP は schema の部分集合と明示的な binding だけを扱う。filesystem root、symlink、network destination、credential scope など、schema だけで完全には拘束できない性質について保証しない。

### target 出力を暗黙に信頼判断へ使わない

MVP は target 出力の対応形式と byte 上限を検査するが、property の射影、redaction、および内容に基づく機密情報の除去は行わない。target 出力を server の診断、認可判断、または設定変更として解釈しないことによって、target の content と境界自身の判断を分離する。

### 自動検出は公開判断にしない

CLI の `--help` や upstream `tools/list` から設定候補を生成することは将来提供できるが、候補は管理者が採用するまで実行可能にならない。依存先の更新を権限拡張として自動反映しない。

### STDIO を最初のトランスポートとする

STDIO は単一利用者のローカル実行で transport 認証を持ち込まず、capability model と target 実行の検証に集中できる。HTTP は単なる listen option ではなく、identity と運用境界を追加するため後続とする。

## E2E 受入シナリオ

実装は少なくとも次を外部 I/F から検証可能にする。

1. **E01 設定検査**: `check` が有効な設定を受理し、optional property を含む不正な設定を target 起動前に設定 path 付きで拒否する。
2. **E02 catalog 出力**: `tools` が server を起動せず、宣言済みツールと縮小済み schema だけを安定した順序で返す。
3. **E03 原子的起動**: 有効な設定で `serve` が起動し、不正な設定では MCP request を一件も受け付けない。
4. **E04 CLI 結果**: 有効な CLI 入力が、期待した executable と argv 配列を一度だけ起動する。text modeはUTF-8 stdoutを一つのtext blockとして保持し、JSON modeはexit code 0かつ有効なJSON出力の場合だけstructured contentと同値のcanonical JSON textを返す。非0、signal終了、またはJSON parse失敗は安定したtarget errorになる。
5. **E05 入力制約**: 未知 property、型不一致、および enum、長さ、範囲、pattern への不一致が、process 起動前に拒否される。線形時間 engine が対応しない pattern、上限を超える pattern、および表現域外の数値は設定採用時または入力検証時に拒否される。
6. **E06 resource 上限**: message、JSON depth、string、array、設定ファイル、tool 数、pattern compiled size、および同時呼び出し数の上限超過が、target 起動前に拒否される。
7. **E07 shell・stdin 分離**: shell metacharacter を含む許可済み文字列が単一 argv 要素として渡され、追加 command を実行しない。CLI の stdin は即座に EOF になる。
8. **E08 呼び出し固定**: 入力から executable、固定 option、作業ディレクトリ、環境変数、および credential を変更できない。CLI と upstream process は設定した cwd と明示的な environment だけで起動する。
9. **E09 CLI 終了処理**: timeout、cancellation、および output 超過で CLI process group が終了・回収され、有限時間内に安定した error code を返す。
10. **E10 upstream 写像**: 有効な upstream MCP 入力が、指定済み upstream tool に明示写像された arguments だけを一度送る。
11. **E11 upstream 非連動**: upstream の tool または schema が増えても、公開 `tools/list` と受理入力が拡張されない。
12. **E12 upstream 失敗**: upstream の設定output kindとの不一致、未対応またはprotocol上不正なcontent、server request、`input_required`、通信失敗、timeout、およびstderr上限超過が成功結果として公開されない。`_meta`は破棄され、未対応contentと対応済みcontentの混在は全体が失敗する。
13. **E13 upstream lifetime**: upstream process が初回呼び出しで起動され、同じ broker 内で再利用され、broker 終了時に回収される。異常終了後の呼び出しは再起動できるが、送信済み request は自動 retry されない。
14. **E14 upstream 直列化**: upstream target は一件ずつ呼び出され、使用中の同じ target への追加呼び出しは待機させず `SERVER_BUSY` になる。timeout または cancellation の終了猶予後に残る process は終了され、次の呼び出しは新しい process を使う。
15. **E15 診断の機密性**: credential を含む target failure でも、server が生成する stderr 診断に credential 値が現れない。target が自ら出力した credential の検出はこの条件に含めない。
16. **E16 公開 protocol 互換**: MCP 2026-07-28 client と legacy initialize client が、同じ設定について同じ公開ツール、入力制約、および解決済み呼び出しを得る。
17. **E17 upstream protocol 互換**: upstream の fake server が modern discovery と legacy initialize のいずれだけを実装していても自動判定で接続でき、同じ設定と入力から同じ upstream tool と arguments を一度だけ送る。

## プロパティベース検証の対象

E2E に加えて、実装は生成入力に対して次を検証する。

- schema に適合しない任意の property 追加は target invocation 数を増やさない。
- 各入力 resource 上限の境界値を一つ超えた入力は target invocation を生成しない。
- 任意の文字列入力は argv の宣言済み位置に高々一要素として現れ、executable または隣接する固定要素を変えない。
- 任意の設定所有固定値は、入力 JSON の追加・並べ替え・同名 property によって変わらない。
- 任意の binding failure は target invocation を生成しない。
- 任意の target output は上限を超えて成功結果に含まれない。
- 任意の error path で、server 自身が生成する外部結果および診断へ secret と指定された値を含めない。
- 設定の serialize、parse、normalize を経ても公開 schema と解決済み呼び出しが変わらない。

## 参照仕様

- [Model Context Protocol 2026-07-28: Tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
- [Model Context Protocol 2026-07-28: Transports](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports)
- [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12)
