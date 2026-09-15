# MCP Capability Boundary 検証設計

## 目的

本書は、[MCP Capability Boundary 設計](design.md)のE2E受入シナリオとプロパティ要件を、外部から観測可能な検証へ翻訳する。[実装アーキテクチャ](implementation-design.md)のcomponent名は検証対象の位置を示すために使うが、内部関数の直接呼び出しだけでE2E成功を代用しない。

test codeとfixtureは実装とともに変更できる。本書は「何を、どの境界から、何を観測して判定するか」を規定する。個別test名との対応は変更可能なmachine-readable manifestに置き、goal終端で本書とtest実体のdriftを再検査する。

## 検証層

| 層 | 対象 | 観測 |
|---|---|---|
| unit | parse、schema compile、binding、error adaptation | pureな入力と戻り値、target invocationなし |
| property | coreの不変条件 | 生成入力、縮小済みcounterexample、invocation count |
| CLI E2E | 管理CLIとCLI target | exit code、stdout/stderr、fake CLI観測、process残存 |
| MCP E2E | 公開STDIOとupstream STDIO | raw MCP message、request sequence、result、process lifetime |
| sandbox preflight | 自律開発環境 | 許可操作の成功と拒否操作の失敗 |
| 独立終了判定 | remote baseline、候補commit、規範、test、実装 | 全tracked path差分、clean checkoutの検証、requirementごとのdrift、実効permission |

unitとproperty testは原因の局所化に使う。E2E要件は必ずbinaryまたはSTDIO wire I/Fからも検証する。outer sandboxによる拒否を製品の入力拒否として数えない。

## test double

### fake CLI

fake CLIは起動時のsubcommandを固定fixtureとして受け、次をmachine-readableに観測できる。

- argvの要素数、順序、および各要素のbyte列
- cwd
- environmentの変数名と、test用sentinelとの一致結果
- stdinがEOFになる時刻
- stdoutとstderrへ出したbyte数
- exit codeまたはsignal
- descendant processのPID、process group、および終了時刻

secret値そのものは観測fileへ記録せず、fixtureが持つsentinelとの一致だけをbooleanで記録する。観測fileとcanaryはtestごとの一意なrepository-local temporary directoryへ置き、公開入力からpathを変更できない。

fake CLIは正常text、正常JSON、非0終了、malformed UTF-8、malformed JSON、出力超過、無応答、stdin待機、descendant生成、およびsignal無視を再現する。

### fake upstream MCP

fake upstreamは起動引数で次のmodeを固定する。

- modern discoveryだけを受理する。
- discoveryをmethod errorにして同じprocessでlegacy initializeを受理する。
- discovery受信後に終了し、再起動時のlegacy initializeだけを受理する。
- negotiationまたはtool callを遅延する。
- tool result、tool error、protocol error、malformed message、未対応content、およびmixed contentを返す。
- cancellationを受理する、無視する、またはprocessを終了する。
- `tools/list`へ未公開toolやargumentを追加する。

fake upstreamは受信したmethod順、process instance ID、tool request数、tool名、argumentの構造、cancellation、および終了理由を観測fileへ記録する。credential sentinelは値でなく一致結果だけを記録する。

### 公開MCP client

公開側E2Eは、MCP 2026-07-28のper-request metadataを送るraw clientと、2025-11-25のinitialize handshakeを行うraw clientを持つ。製品と同じSDKだけに依存したclientを唯一のoracleにしない。message byte上限、未知method、pipelined call、およびcancellationはraw frameで境界値を生成する。

## 設定I/Fの代表能力

設定I/Fの表現力は、実CLIやlive MCPへ接続せず、次の二つの副作用を持たないmock能力を公開STDIOから呼び出して検証する。これらのmockは外部network、Unix socket、およびtest対象fileへアクセスせず、受け取った値と固定fixtureだけからresultを返す。

### mock CLI能力

`mock_describe_resources`は、必須の`resource` enum、pattern付き`namespace`、1から100の`limit`、および1から8要素の`fields` arrayを入力に持つ。解決するargvは次の意味を持つ。

```text
describe
--read-only
--resource <resource>
--namespace <namespace>
--limit <limit>
--fields <field-1> <field-2> ...
--output json
```

subcommand、`--read-only`、option名、`--output json`、executable、cwd、およびenvironmentはliteralまたはtarget設定からだけ決まる。`resource`、`namespace`、`limit`は`input`、`fields`は`each`で写像する。fake CLIは外部処理をせず、観測したargv、cwd、environment名、およびstdin EOFの真偽をJSON stdoutへ返す。

このcaseは、固定optionの間へ必須scalarと必須arrayを配置し、入力がoption名や実行対象を増やさないCLI能力を表現できることを確認する。optional flag、値によるsubcommand切替、およびarray要素ごとの可変prefixは要求しない。その形が具体的な公開能力に必要になれば、別toolへの分割で表現できるかを先に検討し、できない場合だけbinding拡張の再検討条件とする。

### mock MCP能力

`mock_list_records`は、必須の`owner` stringと1から100の`limit`を入力に持つ。設定済みの`mock-upstream` targetにある固定tool `list_records`へ、次のbindingを使う。

```json
{
  "query": {
    "object": {
      "owner": { "input": "/owner" },
      "visibility": { "literal": "private" }
    }
  },
  "limit": { "input": "/limit" }
}
```

たとえば入力`{"owner":"team-a","limit":20}`は、`{"query":{"owner":"team-a","visibility":"private"},"limit":20}`だけへ解決される。fake upstreamは受信argumentsをstructured resultとして返す一方、`tools/list`では未公開toolと追加argumentも提示する。公開catalog、入力schema、および送信argumentsがそれらによって増えないことを確認する。

この二つを外部I/Fから通せず、原因がmock固有の制約でない場合は、設定I/Fの表現力不足として実装を止める。test用telemetryが必要なprocess終了試験だけは`.work`配下を利用できるが、それを公開能力のfile操作として数えない。

## 検証入口

repository rootの`verify`は次の安定した入口を提供する。

```text
verify format
verify lint
verify unit
verify property
verify e2e-cli
verify e2e-mcp
verify requirements
verify all
```

`verify all`はformat、lint、unit、property、E2E、およびrequirementsを包含し、途中失敗を成功へ変換しない。全commandはnetworkなし、候補の`Cargo.lock`を`--locked --offline`で使い、repository-local temporary directoryで実行できる。clone内の`verify`は改善できるが、自己申告の成功だけで完了を判定しない。

`verify requirements`は変更可能な`tests/requirements.toml`を読み、P01からP20とE01からE17がそれぞれ一回以上、存在するtestまたは明示的なreview criterionへ対応していることを構文的に確認する。ID記載だけでは意味的なcoverageを証明しないため、goal終端のfinal reviewを省略しない。

property testはcase数とseedを固定入力として受け、失敗時にseedと縮小済みcounterexampleをreportへ残す。通常の`verify all`は再現可能な既定seedを使う。追加seedによる探索は補助検証であり、既定seedの成功を置き換えない。

検証結果はrepositoryへcommitする成果物ではなく、`.work/reports`の実行ごとに一意なdirectoryへ次を含む一つのreportとして生成する。

- remoteから読み返したbaseline SHAと候補commit SHA
- toolchainとlockfile digest
- 実行した検証入口
- test count、skip、failure
- property seedとcounterexample
- platform
- requirement manifestのcoverage

## 独立終了判定の検証契約

[自律開発設計](autonomous-development.md#独立した終了判定)の終了判定は、remote baselineにある規範、候補commit、開始時の実効permission snapshot、およびループ外に固定した検証契約を入力とする。clone内のreport、local ref名、Git hook、または変更可能な`verify`のexit codeを唯一のoracleにしない。

判定入口は少なくとも固定baseline SHA、candidate commit SHA、独立cloneのabsolute path、および開始時permission snapshotの識別子を受ける。結果はbaseline／candidate SHA、全変更pathと判定区分、規範・permission drift、offline検証結果、P/E要件coverage、preflight結果、および未実行platformを機械可読に返す。入力不備、判定失敗、および全条件成功を異なる終了statusにし、判定未実行を成功statusへ変換しない。実装形式は準備時のrunnerに委ねるが、この入出力境界はgoal開始前に実測する。

機械検査はbaselineを祖先に持つcandidate SHAのtree objectを自分のGit設定で読み、patch表示やclone内のdiff driverではなく、path、type、mode、およびcontent IDを比較して次を欠落なく列挙する。

- baseline SHAと候補SHAの全tracked pathに対する追加、削除、変更、type／mode変更。
- 規範文書、goal受入条件、permission、`.codex`、`.agents`、vendor、toolchain、およびCI/release設定のbaselineとの差分。
- `verify`、Cargo manifest／lockfile、およびtestの差分。差分自体は失敗としないが、必須検証の欠落、vendor外の依存、および規範より弱いassertionを別判定する。
- candidate commitから作ったclean checkoutと、cloneのtracked working tree、未追跡・ignored fileの境界。未commit fileが検証結果にだけ影響していないこと。

規範・permission・受入条件の変更を機械検査が検出した場合、候補側の値を承認済み条件として使わない。diffが空でも、途中で条件を変えて戻した可能性は否定できない。そこでclean checkoutに対してループ外の判定主体が全検証を再実行し、baselineのP/E要件からcandidate testを逆引きする。実効permissionのpreflightはGit diffとは独立に開始時と終了時の両方で確認する。

この判定主体は、candidate codeを実行するため、自律loopと同様にhost secret、network、Unix socket、および外部状態へのwriteを許さない。結果には機械差分、検証report、drift review、permission preflight、未実行platformを分けて記録する。

## E2E受入シナリオの対応

| ID | 主検証 | 外部からの刺激 | 成功の観測 |
|---|---|---|---|
| E01 | CLI E2E | valid／invalid config、scalar root schema、target limit境界、target/output kind不一致で`check` | target未起動、exit code、安定したconfig path診断 |
| E02 | CLI E2E | 同じconfigを順序違いで`tools` | target未起動、同一の正規化JSONとtool順 |
| E03 | MCP E2E | valid／invalid configで`serve` | validだけrequest受付、invalidはstdoutへMCP messageなし |
| E04 | CLI E2E | fake CLIの正常text／JSON・key順違い・構造上限・非0・signal・invalid JSON | argv一回、text保持、JSONのstructured＋決定的compact text、失敗時も生stderrなし |
| E05 | unit＋MCP E2E | 各schema制約の境界内外、CLI-bound stringのU+0000 | invalid時invocation 0、公開path付きtool error |
| E06 | raw MCP E2E | 各resource境界の直前・一致・1超過、17件pipeline | 上限内だけ受付、超過はinvocation 0または`SERVER_BUSY` |
| E07 | CLI E2E | metacharacterを含む許可文字列とstdin待機 | argv一要素、canaryなし、即時EOF |
| E08 | CLI＋MCP E2E | target設定に似た未知入力を付加 | executable、argv固定部、cwd、環境、upstream toolが不変 |
| E09 | CLI process E2E | timeout、cancel、output超過、signal無視、descendant | group全体終了、reap、期限内のstable error |
| E10 | MCP E2E | nested inputとliteralをupstreamへbinding | 設定済みtoolへ期待argumentsを一回だけ送信 |
| E11 | MCP E2E | fake upstreamのcatalogを拡張 | 公開catalogと受理schemaが不変 |
| E12 | MCP E2E | output kind不一致、invalid／mixed content、protocol error、timeout、stderr超過 | raw内容を成功公開せず、`_meta`なし、超過process終了 |
| E13 | MCP process E2E | 連続call、idle終了、公開STDIO EOF、SIGINT、SIGTERM | instance再利用、必要時だけ再起動、全process回収、broker終了status |
| E14 | MCP process E2E | 同一targetへ同時call、cancel無視 | 二件目`SERVER_BUSY`、一件目終了後は新processで成功可能 |
| E15 | CLI＋MCP E2E | secret sentinelを含むenvironmentとtarget error | broker生成resultとstderrにsentinelなし |
| E16 | raw MCP E2E | modernとlegacy clientから同じcall | catalog、validation、resolved invocationの意味が同一 |
| E17 | MCP process E2E | modern、legacy同一process、legacy再起動mode | negotiation成功、tool request一回、revisionによるbinding差なし |

## プロパティ要件の対応

| ID | 主検証 | 生成または検査する関係 |
|---|---|---|
| P01 | property＋E02 | config外のtool、argument、target、credentialがcatalogとinvocationへ現れない |
| P02 | property＋E11 | target側catalogの任意追加で公開catalogが変化しない |
| P03 | property | valid inputへの任意property追加はinvalidになりinvocation数0 |
| P04 | property | 入力の追加・並べ替え・同名試行でliteralとenvironment由来値が変化しない |
| P05 | property | 任意入力でexecutable、target ID、upstream toolが変化しない |
| P06 | property | 同じnormalized configとinputは同じresolved invocationになる |
| P07 | property＋E02 | map挿入順とupstream返却順の置換で公開順とargv順が変化しない |
| P08 | property | parse、validation、bindingの任意失敗でexecutor spyのcall数0 |
| P09 | E07 | shellを介さずstdinがEOFである |
| P10 | property＋E08 | child environmentのkey集合がinheritとsetの和に一致する |
| P11 | property＋E08 | cwdが入力と無関係で設定値に一致する |
| P12 | boundary property＋E06 | 各有限上限の一致値を許可し、一超過をinvocation前に拒否する |
| P13 | E09＋E12 | deadlineとoutput limitの任意terminal pathが有限時間で終わる |
| P14 | process E2E | child回収、upstream直列化、broker以下のlifetime、送信後retryなし |
| P15 | property | catalog生成とruntime validatorが同一compiled schemaを参照する |
| P16 | unit＋E03 | configの任意一箇所がinvalidならcatalogとtarget registryを一件も採用しない |
| P17 | secret property＋E15 | broker生成の全error pathとdiagnosticにsecret sentinelが現れない |
| P18 | unit＋E12 | target bytesはtyped result以外のpolicy、diagnostic、catalog入力にならない |
| P19 | differential E2E | modern／legacyでcoreへ渡るcontext差がresolved invocationを変えない |
| P20 | architecture review＋E16/E17 | core crateがMCP transport型へ依存せずadapterだけがrevisionを扱う |

## drift検査

goal終端では、testからrequirementを集計するだけでなく、P01からP20、E01からE17の順に規範から検証実体を読む。各IDについて次を確認する。

1. 刺激が要件の境界値と失敗modeを含む。
2. assertionが単なる成功終了でなく、規範が要求する観測を検査する。
3. mockが検証対象の処理を置き換えていない。
4. 実装とtestが同じhelperをoracleとして共有していない。
5. skip、条件付き除外、およびplatform未実行がreportに現れる。
6. test変更が要件を狭めず、反例または不足を追加している。

意味的な対応を説明できないID、規範より弱いassertion、または実行不能な必須testが一件でもあればgoalは完了しない。規範の変更が必要なら自律修正を止め、設計レビューへ戻る。

E02のfinal reviewでは、各公開toolの全有効入力が同じ承認上の能力に収まり、名前、説明、`readOnlyHint`、`destructiveHint`、`idempotentHint`、および`openWorldHint`がその全域に対して正しいことを確認する。これは意味validatorの自動testへ置き換えない。入力値で承認上の能力が切り替わる場合は、設定を別toolへ分割する。

## platform判定

通常の自律goalは実行hostのnative process semanticsを必須検証する。macOS goalはmacOS結果だけでローカル完了できるが、Linux対応済みとは表示しない。release readinessは同じsource revision、toolchain、lockfile、およびtest suiteについてmacOSとLinux双方のreportを要求する。

process group、signal、pipe EOF、およびpath semanticsはplatformごとの差があるため、unit mockだけで対応済みとしない。CIまたは隔離済みnative runnerの結果をrelease判定へ使う。
