# MCP ツール公開 I/F（設定 version 2）

本書は `version: 2` の互換動作を記す。[MCP ツール公開 I/F（設定 version 3）](mcp-exposure-v3-design.md) は別の明示的な設定 version として扱い、この version 2 の `inputs` / `fixed` 構文や freeze の lifecycle を version 3 の規範へ持ち込まない。

## 目的と境界

管理者が選んだ upstream MCP ツールだけを公開する。選ばなかったツールは `tools/list` に現れず、名前を直接指定した `tools/call` でも実行できない。MCP client 側の enabled / approved 設定は、この公開判断の代わりにならない。

本書は MCP ツール公開に関する設定 version 2 の規範である。[基本設計](design.md)の version 1 にある、MCP 引数の手書き写像、公開 catalog 不変、`tools.listChanged` 非対応という規範を、この範囲に限って置き換える。CLI target、process 所有、credential、出力形式・上限の境界は基本設計に従う。version 1 の設定を黙って version 2 として解釈しない。

MCP 公開は二種類に限る。

- **proxy**: upstream の一つのツールを丸ごと許可する。入力 schema を手書きせず、upstream の現在の定義を公開する。これは upstream の同名ツールに対する広い権限であり、引数の追加・削除も原則として受け入れる。
- **restriction**: 同じツールの一部の top-level 入力だけを公開し、残りを固定または省略する。公開入力と固定値を broker が強制する。公開定義は freeze し、upstream の変更から自動拡張しない。

複数の upstream ツールを一つにまとめる、任意の新規 MCP ツールを構成する、入力 object の階層を組み換える、式や任意コードを実行する I/F は設けない。そうした必要性が実例で確認されるまでは、tool 合成を agent に任せ、境界の表現力へ投資する。

## 設定 I/F

```yaml
version: 2
server:
  name: time-boundary
  transport: { kind: stdio }
targets:
  time:
    kind: mcp
    transport:
      kind: stdio
      command: /absolute/path/to/mcp-server-time
      args: []
      cwd: /absolute/working/directory
    limits:
      timeout_ms: 30000
      output_bytes: 1048576
      stderr_bytes: 65536
    expose:
      get_current_time: {}                 # proxy: this tool, not the entire server
      convert_time:
        as: utc_to_timezone                # optional; no silent name collision
        restrict:
          inputs:
            time: {}
            target_timezone:
              enum: [Asia/Tokyo, Europe/London]
          fixed:
            source_timezone: UTC
```

`expose` は upstream の tool 名を key とする明示的な allowlist である。`{}` は proxy、`restrict` の存在は restriction を意味し、`mode: proxy` の反復を要求しない。`as` がなければ公開名は upstream 名と同じとする。CLI 公開ツールを含め、公開名の衝突は設定エラーであり、自動的な prefix や上書きはしない。`expose` にない upstream ツールは、後から増えても公開しない。

restriction の `inputs` は公開する top-level property と、その property に追加する制約の対応である。空 object は upstream の property 制約をそのまま使う。追加制約は enum、整数・数値の minimum / maximum、string の minLength / maxLength / pattern に限り、upstream の制約との共通部分を公開する。`pattern` は線形時間 engine の対応構文だけを受け入れる。`fixed` は公開 schema に現れない server 所有の JSON 値で、呼び出し時に broker が挿入する。同じ property を `inputs` と `fixed` の両方へ置くことはできない。どちらにもない optional property は送らない。upstream の必須 property がどちらにもない場合は restriction を成立させない。

restriction の公開 schema は upstream の top-level object property から安全に導出できる場合だけ生成する。`$ref`、複合条件、開いた object などによって選択・固定・追加制約の効果を証明できなければ、その restriction を使えない。静的な記述違反は `check` と `serve` の設定エラー、upstream 定義との不整合は対象ツール固有の利用不能として扱う。proxy への暗黙 fallback や、管理者が書いた全く別の公開 `inputSchema` での迂回はしない。upstream の optional property を restriction の公開入力として扱う必要が生じたときは、入力の有無が承認上の意味を変えないかを確認してから I/F を拡張する。

CLI ツールは従来どおり root の `tools` で定義する。version 2 では root `tools` の `invoke.mcp` を受け付けない。upstream との一対一の手書き写像を別の公開経路として残さない。

## 定義の取得と cache

broker は接続可能な upstream の `tools/list` から、選択したツールの定義を取得する。proxy の公開説明・annotation・入力 schema はその時点の upstream 定義を基にする。proxy 呼び出しは公開 tool 名を固定の upstream 名へ戻し、受け取った arguments object を構造と値を変えず一度だけ送る。broker は資源上限・公開名 allowlist・対応 content 形式を強制するが、古い proxy schema を呼び出し時の独立した拒否根拠にはしない。引数の適否は upstream が判断する。proxy の description / annotation は権限判定には使用しない。

restriction の初回生成に使った upstream 定義と、そこから導出した公開定義を snapshot として保持する。再接続や再起動後も、管理者が snapshot を更新するまでは公開 schema・description・annotation を自動拡張しない。呼び出し時にはその公開 schema と追加制約を broker が検証し、固定値を挿入してから送る。upstream 定義との差分だけで直ちに公開停止しない。新しい optional property など、既存の制約付き呼び出しが成立する変更はそのまま扱う。選択した upstream tool の消失や、新たな必須 property など既存の呼び出しが成立しないと確認できる変更では、対象の restriction を公開 catalog から外す。証明できない意味の変化を broker が推測して認可判断に使わない。upstream が arguments を拒否した場合は、その拒否を対象ツールのエラーとして返し、原因を断定しない。

snapshot は `$XDG_CACHE_HOME/mcp-boundary`、未設定時は `~/.cache/mcp-boundary` に置く。各 target の entry path は `catalog-v1/<config-digest>/<target-id>.json` とし、`config-digest` は config の絶対 path と bytes の SHA-256 から導出する。cache entry は少なくとも次の形とし、公開定義と upstream 定義を区別する。更新は一時 entry の検証後に原子的に置き換える。

```json
{
  "format_version": 1,
  "config_digest": "sha256:<config-path-and-bytes-digest>",
  "target": "time",
  "tools": {
    "get_current_time": {
      "upstream": "<bounded MCP tool definition>",
      "public": "<bounded MCP tool definition>"
    },
    "convert_time": {
      "upstream": "<bounded MCP tool definition>",
      "public": "<frozen restricted tool definition>"
    }
  }
}
```

credential や継承した環境変数の値、呼び出し arguments、tool result は保存しない。形式不正・digest 不一致・上限超過の cache は採用しない。cache は公開名や固定値の許可元ではなく、管理者設定の `expose` と `restrict` が常に上限を決める。ただし restriction の freeze した公開定義には境界上の意味があるため、cache とその親 directory は config と同じく agent から変更不能でなければならない。配置先が `~/.cache` であること自体は保護を保証しない。config の変更で key が変われば新しい snapshot を生成する。config を変えずに freeze を更新する操作は管理者に限り、更新前後の公開定義を review できる手段を設ける。agent による cache の削除・再生成を更新手段にしない。

broker の起動そのものは upstream 接続を待たない。最初の `tools/list` または対象ツールの call を契機に、上限付きで定義取得を試み、以降の list / call や再接続でも再試行できる。upstream が一時的に不通でも broker 全体は起動する。保護された cache に定義があればそれを表示し、その target の呼び出しだけを利用不能にする。cache がなければ該当ツールを `tools/list` から省く。既知の設定済み名への直接 call は `TARGET_UNAVAILABLE` を返す。接続が回復して定義を取得した時点で、公開 catalog を更新できる。cache を消すことによる restriction の再 freeze は管理者の操作であり、agent の自動復旧経路にしない。

## 公開 catalog の変更と失敗

公開 server は tools capability の `listChanged` を advertise する。upstream の `notifications/tools/list_changed`、再接続、および取得失敗からの回復を、選択済みツールだけの再取得契機とする。proxy の公開定義が変わったとき、または restriction の提供可否が変わったときは、新しい catalog を原子的に採用してから downstream へ `notifications/tools/list_changed` を送る。新規 upstream tool 名は設定変更なしに追加しない。通知自体に差分や理由を詰め込まず、後続 `tools/list` で現在の定義を返す。client が必ず再取得するとは仮定しない。

古い catalog を見た client から停止済み restriction が呼ばれた場合は、未知 tool の一般的な protocol error ではなく、`isError: true` の tool result で安定 code `TOOL_DEFINITION_CHANGED`、公開 tool 名、理由、`tools/list` の再取得または管理者への連絡という対処を返す。停止理由の最小限の記録は broker 内に保持する。通知の未受信・無視や競合をこの result で扱う。既存の proxy 名が upstream から消えた場合も同じ結果とする。未選択の名前は引き続き未知 tool とし、内部の upstream 一覧を漏らさない。

target の一時不通は定義変更と区別し、保護された cache がある限り catalog を消したり通知したりしない。接続復旧後の確認を経ずに cache の古さだけで schema change と断定しない。

version 2 の `check` は設定の静的妥当性だけを検証し、upstream の接続や動的な schema 互換性の成功を保証しない。管理 CLI の `tools` は target を起動せず、cache から構成できる公開 catalog と、cache 未取得・利用不能な選択済みツールの状態を `{"tools":[...],"unavailable":[{"name":"...","reason":"METADATA_UNAVAILABLE"}]}` のように区別して返す。稼働中 broker の最新 catalog と同一である保証は `tools/list` にだけある。これにより管理 CLI を実行するだけで credential を使った upstream process を起動しない。

## E2E 受入条件

fake upstream と raw downstream MCP client から、少なくとも次を検証する。

1. `expose` に選んだ proxy と restriction だけが発見・呼び出し可能で、未選択名への直接 call では upstream invocation が 0 件である。alias 衝突、unsupported restriction、必須 property の欠落は安全側に失敗し、proxy に変わらない。
2. proxy の初回 catalog は upstream 定義を示し、任意の arguments object を同名の固定 upstream tool へ一回だけ送る。upstream に optional または required argument が追加・削除された後は catalog 更新が通知より先に成立し、再 list は新定義を示す。新規 upstream tool 名は公開されない。
3. restriction は選択した入力だけを公開し、追加 enum / range / pattern を server 側で強制し、固定値の上書きを拒否する。upstream の optional argument 追加でも公開 schema は変わらず、有効な旧呼び出しは継続する。
4. upstream tool 消失または明確な互換破壊では、そのツールだけを catalog から外して通知する。古い client の call は `TOOL_DEFINITION_CHANGED` を含む解釈可能な tool errorとなる。upstream の単なる拒否は原因を断定しない target error となる。
5. cache 有りで upstream 不通なら server は起動し、cache の公開定義を表示しつつ当該 call だけが失敗する。cache 無しならその tool を省き、回復時に catalog を先に更新して通知する。壊れた cache は採用しない。
6. 二つの protocol 世代でも公開内容と呼び出し制約が同じで、未対応 output content や `_meta` の扱いは基本設計の境界を維持する。

この設計は upstream の意味や出力内容を監視・保証しない。proxy の入力面が広いこと、上流の意味変更が schema 差分だけでは検知できないことは、管理者の公開判断に残る。厳密な境界が必要なツールは restriction または CLI の固定 binding を使う。
