# MCP Capability Boundary 実装アーキテクチャ

## 位置づけ

本書は、[MCP Capability Boundary 設計](design.md)を実装可能な component、型、および状態遷移へ落とす。外部から観測できる振る舞いと security property は `design.md` を規範とし、本書はその意味を狭めない。両者が矛盾する場合は実装を進めず、`design.md` のレビューへ戻る。

実装は Rust 2024 edition の Cargo workspace とし、macOS と Linux を対象にする。toolchain は準備時に Rust 1.98.1 を `rust-toolchain.toml` で固定し、すべての platform で同じ toolchain と `Cargo.lock` を使う。

## 採用する依存境界

| 責務 | 採用 | 境界 |
|---|---|---|
| MCP client/server | 公式 `rmcp` 3.x | 2026-07-28とlegacy lifecycleのwire処理だけを委譲し、policyをhandlerへ入れない |
| async runtime | Tokio 1.x | process、pipe、deadline、cancellationに限定する |
| YAML | `serde-saphyr` 1.x | strict optionを指定し、default挙動に依存しない |
| JSON | `serde_json` 1.x | MCP wireとの変換に使い、検証用の正規化値は独自型へ変換する |
| regex | `regex` 1.x | fallback engineを持たず、pattern lengthとcompiled sizeを制限する |
| POSIX signal | `nix` 0.x | process groupへのsignalとwait結果の判定だけに使う |
| 管理CLI | `clap` 4.x | `check`、`tools`、`serve`と必須引数だけを定義する |
| property test | `proptest` 1.x | seedとcase数を検証結果へ記録する |

直接依存は `Cargo.toml`、実際に解決した全versionは `Cargo.lock` を基準とする。自律実行中、toolchainとvendorは変更しない。manifestとlockfileのclone内改訂は変更提案として許すが、固定baselineとの差分とoffline検証を終了判定で審査する。vendorにない依存、feature変更に伴う再取得、またはtoolchain更新が必要になった場合は準備フェーズへ戻る。

Rustを再検討するのは、公式SDKが保証対象のMCP revisionを同時に扱えない場合、macOSまたはLinuxで必要なprocess group制御を安全に実装できない場合、あるいは単一binaryとしての配布が製品要件でなくなり別言語の運用基盤を再利用できる場合に限る。

## workspace と依存方向

```text
mcp-boundary (binary composition and management CLI)
       |
       v
mcp-boundary-runtime (MCP and POSIX adapters)
       |
       v
mcp-boundary-core (pure policy and resolution)

test-support (fake CLI and fake MCP binaries)
       ^
       |
integration and E2E tests
```

`mcp-boundary-core` は filesystem、process、clock、MCP SDK、およびasync runtimeへ依存しない。raw configurationを正規化し、公開catalogを生成し、入力を検証し、解決済み呼び出しを返す。

`mcp-boundary-runtime` はcoreが返した解決済み呼び出しだけを受け取り、CLI processまたはupstream MCPへ変換する。未検証のclient argumentsを受ける公開関数を持たない。

binary crateはcommand line、設定fileの読み取り、componentの構成、および終了処理だけを担う。policy判断を重複実装しない。test-supportは製品crateから依存されない。

依存方向はcompile時に固定する。runtimeからcoreへの依存は許すが、coreからruntime、MCP SDK、またはOS APIへの依存は許さない。

## 中核データモデル

### 設定の段階

```text
ConfigBytes
  -> ParsedConfig
  -> ValidatedConfig
  -> CapabilityCatalog
```

- `ConfigBytes` は上限確認済みのraw bytesとsource名だけを持つ。
- `ParsedConfig` はsyntax上の値とsource spanを持つが、実行に使えない。
- `ValidatedConfig` は全参照、schema、binding、path、environment、およびlimitが検証済みのimmutable valueである。
- `CapabilityCatalog` は公開順が確定したtool定義と、tool IDからcompiled capabilityへの索引を持つ。

`ParsedConfig` またはdeserialization途中の型をcatalogとexecutorへ渡せないAPIにする。起動時は一つの`ValidatedConfig`からcatalogとtarget registryを構築し、部分的に成功した設定を保持しない。

### 入力値

検証用の値は次の閉じた型へ正規化する。

```text
InputValue =
  Null
  | Boolean(bool)
  | String(UTF-8)
  | Integer(i64)
  | Number(f64, finite, negative-zero normalized)
  | Array(Vec<InputValue>)
  | Object(BTreeMap<String, InputValue>)
```

objectはkeyの辞書順を持つ。`number` schemaはintegerも受理するが、`integer` schemaは小数部を持つ値を受理しない。`enum`と`uniqueItems`ではintegerと同じ数学値を持つnumberを等価として扱う。

stringの`minLength`と`maxLength`はUnicode scalar value数で評価し、全体のstring上限はUTF-8 byte数で評価する。arrayとobjectの深さはrootを1として数え、decode後の再帰処理も同じ32段上限を使う。

### 解決済み呼び出し

```text
ResolvedInvocation =
  Cli {
    target_id,
    executable: AbsolutePath,
    argv: Vec<OsString>,
    cwd: ExistingAbsoluteDirectory,
    environment: SecretEnvironment,
    deadline,
    stdout_limit,
    stderr_limit,
    output_kind
  }
  | Mcp {
    target_id,
    tool_name,
    arguments: JsonObject,
    deadline,
    output_limit
  }
```

型のconstructorはcore内に閉じ、検証済みの値からだけ生成する。`SecretEnvironment`は値の`Debug`、`Display`、serializeを実装しない。test用にはexecutable、argv位置、cwd、環境変数名、および値の一致だけを選択的に観測できるredacted viewを提供する。

## 設定の読み取りと正規化

設定は次の順序で処理する。

1. file metadataに依存せず、最大1 MiBまで読み、次の1 byteが存在すれば失敗する。
2. YAML 1.2として一つのdocumentだけをparseする。JSONはこの部分集合として同じ経路を通る。
3. duplicate key、merge key、alias、custom tag、未知field、および複雑なmapping keyを拒否する。
4. YAML 1.1固有のbooleanとoctal解釈を無効にし、non-finite numberを拒否する。
5. typed raw configへ変換し、source span付きの診断を保持する。
6. schemaとbindingをcompileし、target参照とenvironmentを解決する。
7. tool IDの辞書順でcatalogを確定する。

`serde-saphyr` はduplicate keyをerror、merge keyをerror、alias展開上限を0、strict booleanを有効、unsupported tagを拒否、document数を1、入力byteとnode/depth budgetを製品上限以下に設定する。依存のoptionだけでaliasを完全に拒否できないversionでは、event parserでalias eventを検出してから同じbytesをdeserializeする。二つのparse結果を組み合わせて値を生成してはならず、前段は拒否判定だけに使う。

unknown fieldは全階層で拒否する。map順は入力順に依存させず、公開tool、schema property、environment名、およびupstream argument propertyを辞書順へ正規化する。argvだけはbindingで宣言した順序を保つ。

absolute executableとcwdはlexicalなabsolute pathであることに加えて、起動時にmetadataを検査する。executableはregular fileかつ実行可能、cwdはdirectoryでなければならない。symlinkの解決先固定やbinary hash検証はMVPで保証しない。

`environment.inherit` はbroker起動時のsnapshotから解決する。欠落した変数は設定失敗にし、各targetのchild environmentは空mapから`inherit`と`set`だけを加える。同名が両方にある設定は曖昧なため拒否する。

## schema compiler とvalidator

受理するJSON Schema keywordは独自のclosed enumへcompileする。一般的なJSON Schema evaluatorへraw schemaを渡さない。これにより、未許可keywordがlibrary更新で暗黙に有効になることを防ぐ。

compile時に次を確認する。

- 公開toolの`input_schema` rootが閉じたobjectである。
- objectの全propertyが`required`に一度だけ現れ、`additionalProperties`がfalseである。
- arrayの`items`が一つあり、`minItems <= maxItems <= 1024`である。
- stringとarrayの局所上限が全体上限を超えない。
- 数値境界が表現域内で、`minimum <= maximum`である。
- `enum`が空でなく、各値がschema typeと互換で、数学的に同値な重複を持たない。
- patternが16 KiB以下で、`regex` crateが対応し、compiled sizeが1 MiB以下である。
- bindingのJSON Pointerがschema上で必ず存在する値を指す。
- tool名とtarget IDが1文字以上128文字以下の許可文字だけから成り、環境変数名が許可形式である。
- process APIへ渡る設定文字列がU+0000を含まない。
- target limitが製品上限内で、target kindとoutput kindの組合せが許可されている。

JSON PointerはRFC 6901のescapeだけを許すabsolute pointerとする。objectの必須propertyを辿るpointer、または必須array全体を指すpointerだけをcompileできる。動的なarray index、存在が保証されない経路、およびroot object全体の暗黙passthroughは拒否する。

validatorはcompiled schemaだけを受け、最初の違反で停止せず、公開propertyに関する違反を安定したpath順で返す。ただし診断数は32件で打ち切る。診断は公開schemaのpathとconstraintだけを含み、target設定を含めない。

CLI bindingのscalar変換は次で固定する。

- string: U+0000を含まないUTF-8内容を一つのargv要素としてそのまま渡す。U+0000を含む値は入力不正にする。
- integer:符号付き十進の最短表記にする。
- number:同じbinary64へround-tripする最短JSON表記にし、負のzeroは`0`にする。
- boolean、null、object、array: `input`では拒否する。
- `each`: string、integer、numberの一段arrayだけを各一要素へ変換する。

## 呼び出しpipeline

```text
MCP request
  -> bounded transport decode
  -> protocol context normalization
  -> catalog lookup
  -> input normalization and validation
  -> invocation resolution
  -> admission control
  -> target executor
  -> result validation
  -> protocol result adaptation
```

各段は前段の成功型だけを入力にする。target processを起動できるのはadmission control通過後のexecutorだけである。global slotまたはupstream target slotを取得できない場合は待たずに`SERVER_BUSY`を返し、解決済み呼び出しを実行しない。

call deadlineはadmission成功直後から、targetの起動、negotiation、request、output収集、終了、およびreapまでを含む。client cancellationは同じtermination pathへ変換する。

## bounded STDIO adapter

公開側とupstream側のSTDIOは、MCP SDKがJSON decodeする前に一messageのraw byte数を数えるadapterを通す。delimiter到達前に上限を超えた場合は、そのmessageをparseせず接続を失敗させる。decode後は深さ、string、およびarray上限をcoreのnormalizerで再検査する。

stdoutはMCP wire専用で、診断を一byteも書かない。診断はstderrへ一行一JSON objectで出す。upstream processについてもstdoutをprotocol専用として扱い、stderrは設定した上限内でdrainするが公開resultへ暗黙には含めない。

## CLI executor

### 起動

CLIはshellを介さず、absolute executable、argv vector、absolute cwd、および空から構成したenvironmentで起動する。stdinはpipeを作らずnull/closedにする。childは起動時に自身をleaderとする新しいPOSIX process groupへ置く。

stdoutとstderrは別taskで同時にdrainする。各readerはchunkを受け取るたびにbyte数を加算し、上限を一byteでも超えた時点で共有cancellationを発火する。上限ちょうどの出力は許容する。

### 終了状態

```text
Prepared -> Running -> Exited -> Reaped
                    \-> Terminating -> Reaped
```

正常終了は、processのexit codeが0、必要なpipeがEOF、出力が上限内、出力形式が妥当、かつchildをreapした場合だけ成立する。text outputはUTF-8 stdoutを一つのtext blockとして保持する。JSON outputはstdout全体を一つのJSON valueとしてparseし、depth、string、array、およびnumberの構造上限を検査する。structured contentと、object keyをUTF-8 byte列順へ並べた最短compact JSONのtext blockを、同じ正規化値から生成する。

timeout、client cancellation、output超過、pipe error、またはbroker shutdownではprocess groupへSIGTERMを送り、500 ms待つ。残存していればSIGKILLを送り、leaderをwaitしてから結果を返す。leaderが先に終了してもpipeを保持するdescendantがいればdeadlineまで収集を続け、deadline到達時は既知のprocess groupを終了する。

signal送信の失敗とwait失敗は成功へ変換しない。終了理由の優先順位は`CANCELLED`、`OUTPUT_LIMIT_EXCEEDED`、`TARGET_TIMEOUT`、I/Oまたはprocessの`TARGET_FAILED`とし、同時発生時にも結果を決定的にする。

## upstream MCP executor

各upstream targetは一つのactorが所有し、child process、transport、negotiated revision、およびactive requestをactor外へ共有しない。

```text
Dormant
  -> ProbingModern
       -> ReadyModern
       -> InitializingLegacySameProcess -> ReadyLegacy
       -> RestartingLegacy -> ReadyLegacy
  -> Calling
       -> ReadyModern | ReadyLegacy
       -> Terminating -> Dormant
  -> ShuttingDown -> Stopped
```

初回callは次の規則でnegotiationする。

1. absolute commandからprocessを一度起動し、`server/discover`を送る。
2. 10秒またはcall deadlineの残時間の短い方まで肯定的なmodern応答を待つ。
3. method拒否などで接続が利用可能なら同じprocessへlegacy `initialize`を送る。
4. 接続またはprocessが終了した場合は一度だけ新しいprocessを起動し、最初のmessageとしてlegacy `initialize`を送る。
5. negotiation完了前にtool requestを送らない。legacy再起動にも失敗した場合はcallを失敗させ、さらにretryしない。

MCP SDKのauto negotiationがprocess終了後の再起動まで所有できない場合、actorがmodern probeとlegacy restartを明示的に構成する。SDKのdefault protocol versionや暗黙retryへ依存しない。

Ready状態のprocessはbroker内で再利用する。同じtargetがCalling状態なら追加callを`SERVER_BUSY`で拒否する。request送信後のtransport error、timeout、およびprocess終了では同じ公開callを再送せず、processを終了してDormantへ戻す。

timeoutまたはcancellationではnegotiated revisionが許すcancellationを一度通知し、500 ms後もcallが終了しなければprocess groupをSIGTERM、さらに500 ms後にSIGKILLしてreapする。broker shutdownでも同じ終了処理を使う。

upstreamから受け取る一messageとresult全体はdecode前後の両方で上限を検査する。structured contentにはCLI JSONと同じdepth、string、array、およびnumberの構造上限を適用する。upstream stderrも設定した`stderr_bytes`まで別に収集し、一byteでも超えればprocessを終了する。stderrを公開resultへ暗黙に含めない。textとstructured contentだけを順序どおり採用し、設定したoutput kindに適合する場合だけ公開する。未知content、混在した未対応content、およびinvalid structured contentは全体を失敗させる。`_meta`はresultの成否を変えず、公開resultから破棄する。

## 公開MCP adapter

公開adapterはMCP 2026-07-28のper-request metadataとlegacy connection metadataを共通の`CallerContext`へ変換する。`CallerContext`のprotocol情報はresultのwire表現にだけ使い、catalog選択、validation、binding、target、limit、またはcredentialを変更しない。

serverはtools capabilityだけをadvertiseする。SDKが提供するprompts、resources、sampling、elicitation、task、またはlogging handlerは登録しない。SDK更新でdefault capabilityが増えた場合はconformance testが失敗し、明示的に無効化できるまで更新を採用しない。

public `tools/list`と管理CLIの`tools`は同じ`CapabilityCatalog`から生成する。protocol revision固有のfieldを除き、tool名、説明、annotation、およびinput schemaの意味を一致させる。

## error と診断

内部errorは次の三つを分離する。

```text
BrokerError {
  stable_code,
  exposure_class,
  public_path?,
  correlation_id,
  private_cause
}
```

- protocol error: malformed envelope、未知method、未知tool、capacityまたはlifecycle上のserver拒否。
- tool error: target起動後の失敗、timeout、output failure、cancellation。
- operator diagnostic: 設定失敗、内部不変条件違反、shutdown failure。

wire分類は次で固定する。

| 状態 | wire表現 |
|---|---|
| malformed envelope、未知method、未知tool | JSON-RPC protocol error |
| `SERVER_BUSY`、shutdown中の新規call | JSON-RPC server error |
| `INVALID_ARGUMENTS`、`POLICY_DENIED` | target未起動の`isError: true` tool result |
| target unavailable、timeout、failure、output failure、cancellation | `isError: true` tool result |

外部resultとstderr診断は`stable_code`ごとの固定templateから作り、`private_cause`の`Display`や`Debug`を連結しない。target ID、公開property path、OS error kind、およびcorrelation IDは出せるが、executable、cwd、argv、environment value、upstream arguments、およびraw target outputは出さない。

全environment valueをsecretとして扱う。固定argvとupstream literalはcredentialではない前提だが、通常errorへ表示しない。target自身が成功または失敗contentへsecretを出した場合の検出はMVPの保証外である。

## admission とshutdown

brokerは16個のglobal call slotを持つ。slotはtarget起動直前に`try_acquire`し、callのresult変換とprocess回収が終わるまで保持する。待機queueを設けない。upstream actorもtargetごとに一つのslotを持ち、global slot取得後にtarget slotを取得できなければglobal slotを解放して`SERVER_BUSY`を返す。

公開STDIOのEOF、SIGINT、またはSIGTERMでshutdownを開始する。開始後の新しいcallは`TARGET_UNAVAILABLE`をstable codeに持つJSON-RPC server errorとして拒否する。active CLIとupstream actorへcancellationをbroadcastし、各process groupを通常のtermination pathで回収する。全childのreapまたは固定されたshutdown deadline到達までbroker processを終了しない。deadline後もchild回収に失敗した場合はstderrへsafe diagnosticを出し、非0で終了する。

## 実装単位の境界

最初の実装単位はCLIのvertical sliceとする。設定parse、catalog、公開MCP、validation、resolution、fake CLI実行、result、resource limit、termination、およびsafe diagnosticを外部I/Fまで通す。この単位はE01からE09、E15、E16と、該当するP01からP20を検証可能にする。

次の実装単位でupstream actor、modern/legacy negotiation、reuse、serialization、cancellation、およびoutput adaptationを加え、E10からE14、E17とP02、P14、P19、P20のupstream側を完成させる。

二つの単位は同じcore型、bounded transport、error model、および検証入口を使う。upstream実装のためにCLI側のpolicy経路を複製しない。これらは一つのMVP自律goal内の連続したmilestoneであり、最初の単位が成功した時点で利用者へ作業を戻さず、次の単位へ進む。依存、権限、または規範の変更が必要になった場合だけ準備または設計レビューへ戻る。MVP完成は両方と最終drift検査の成功を必要とする。

## 参照

- [MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Rust regex crate: untrusted input](https://docs.rs/regex/latest/regex/#untrusted-input)
- [serde-saphyr options](https://docs.rs/serde-saphyr/latest/serde_saphyr/options/struct.Options.html)
- [Rust Unix process extensions](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html)
