# 自律開発・検証ループ設計

## 目的

本設計は、[MCP Capability Boundary 設計](design.md)と[実装アーキテクチャ](implementation-design.md)の実装、テスト、修正、および再検証を Codex が継続できる環境を定める。要件から検証への翻訳は[検証設計](verification-design.md)に定める。

自律性は host への広い権限によって実現せず、外へ影響する操作だけを技術的に封じ、ローカルな試行錯誤はcommitを含めて許可することで実現する。ローカル成果の正しさは、ループが編集できない基準と独立した終了判定で確かめる。

## 優先する境界とリスクの扱い

| 対象 | ループでの価値 | リスク | 扱い |
|---|---|---|---|
| source、test、fixture、文書、`verify`、Cargo manifest | 修正と要件の翻訳に必須 | 偽の合格、規範の一時改変、依存構成の変化 | clone内writeを許可し、baseline基準の独立終了判定で低減する。途中で改変して戻す履歴は追えないことを受容する |
| local Git commitとref | checkpoint、差分review、復旧 | hook・config改変、ref破損、checkpoint喪失 | 独立cloneの`.git`だけwriteを許可する。remote baselineから復旧できる範囲の損失を受容する |
| 実効permission、vendor、toolchain、終了判定 | sandboxと検証の信頼元 | 自己緩和、別の実行環境へのすり替え | loopからの変更を技術的に拒否し、開始時・終了時に実効性を確かめる |
| host secret、clone外file、network、Unix socket、外部tool面 | mock開発には不要 | 漏洩、remote push、live targetへの副作用 | 事後diffでは回復できないため事前に拒否する |

この優先順位は、変更可能なlocal fileをすべて安全とみなすことではない。失敗がclone内に留まり、固定baselineと別環境で再検証できる場合だけ「検知してから判断する」方式を採る。外部状態への副作用と実効権限の緩和は、検知後の復旧ではなく強制的な境界を必要とする。

## 成立条件

自律開発ループは次を満たしてから開始する。

- `design.md` の E2E 振る舞い、プロパティ要件、および I/F が実装の判断基準として採用されている。
- 設計、受入条件、検証契約、依存境界、および開発用permissionを含む準備済みbaseline commitがremoteへpushされ、そのcommit SHAがループの外で固定されている。
- 自律ループはそのSHAから作った独立cloneで動き、既存checkoutやremoteのGit directoryとrefを共有しない。
- Rust toolchain、Cargo workspace、依存lockfile、および対応platformが実装アーキテクチャどおりに固定されている。
- build、単体テスト、property test、E2E、および静的検査を一つの検証入口から起動できる。
- CLI target と upstream MCP を置き換える決定的な test double が repository 内にある。
- テストは実 credential、live service、利用者の home directory、および production data を必要としない。
- 依存取得と toolchain 準備を対話的な準備フェーズで完了し、自律実行中は offline で検証できる。
- 自律実行に使うCodex permission profileと承認方針でpreflightが成功し、Remote projectの`.codex/config.toml`だけで新sessionが実際に提示するtool一覧を許可集合へ固定でき、ループ外の終了判定が同じbaselineから再現できる。

これらの成立条件を満たさない状態では、検証不能な実装を積み増さない。

## スコープ

自律開発ループは次を行える。

- 独立clone内のsource、test、fixture、文書、検証入口、および依存manifestの編集
- project-local build directory と専用 test directory への一時出力
- 準備済みvendorだけを使うoffline build
- fake CLI と fake upstream MCP を使う単体、property、および E2E テスト
- formatter、linter、type checker、schema validator、および test runner の実行
- 独立clone内でのGit status、diff、stage、local commit、およびlocal branchによるcheckpoint
- global AGENTS.md、Codex設定、memory、および既存skillの、特定pathに限定した読み取り

次は自律開発ループに許可しない。

- dependency、toolchain、MCP server、plugin、skill、または OS package の取得と導入
- public network、private network、Unix socket、および live MCP server への接続。ただしループの成果に不可欠と事前に確認したCodex側MCP toolだけは、serverとtool名を固定した例外として扱える
- 実 credential、`.env`、SSH key、cloud configuration、および上記の明示的な例外以外の利用者固有の設定の読み取り
- repository 外の永続ファイルの変更
- baseline remote、既存checkout、およびループ外の判定環境への書込み
- release、publish、push、merge、および外部issueやmessageの作成
- 実効sandbox、permission profile、承認方針、またはループ外で固定したgoal受入条件の自己変更
- 設計上の根拠がない test の skip、期待値の弱体化、または検査対象の削除による合格化

remoteへのpush、dependency取得、toolchain導入、platform追加、外部互換試験、release、およびlive systemを使う試験は、ループ外の明示的な作業として扱う。clone内で規範や検証入口を編集しても、それは変更提案であり、ループが自分の完了条件を変更したことにはならない。

### 規範とテストの関係

baseline SHAに固定された`design.md`のE2E受入シナリオ、プロパティ要件、およびI/F、`implementation-design.md`の責務境界、`verification-design.md`の観測条件、およびループ外で承認した個別goalの受入条件を規範とする。test codeとfixtureは規範を実行可能な形へ翻訳する成果物であり、実装とともに追加、修正、および整理できる。初期testの完全性や正しさは前提にしない。

test の成功だけでは goal の完了を判定しない。最終検証では、規範に列挙された各 E2E シナリオとプロパティ要件から、対応する test、property test、静的検査、または review criterion を逆引きする。対応の欠落、規範より狭い検査、成功条件の弱体化、および実装と test が同じ誤った前提を共有する状態を drift として扱う。

testの変更は、その根拠となる規範上の項目を説明できなければならない。clone内の規範文書を編集して改善案を残すことはできるが、baselineとの差分を終了判定が検出し、承認されるまでgoalの規範にはならない。規範の変更なしに成果を完成できない場合は、提案差分を残して設計判断へ戻る。

## 分離モデル

```text
準備主体 -> baseline commitをremoteへpush -> SHAと実効permissionをループ外で固定
                                             |
                                             v
                                  独立clone / command sandbox
                                  source・test・文書・local commit
                                             |
                                             v
                                   fake CLI / fake MCP
                                             |
                                             v
                                  ループ外の独立終了判定

独立clone X-> baseline remote / 既存checkout / host secret / live network
```

### Codex コマンドサンドボックス

Codex が起動する shell command とその child process は、project-specific permission profile の範囲で動く。profile は filesystem と command network の技術的な上限であり、prompt またはレビュー規約の代替ではない。

自律実行では承認方針を `never` とする。これは承認待ちをなくすだけで、permission profile を越える権限を与えない。境界外の操作は失敗し、その失敗を理由に full access へ切り替えない。

自律ループは接続先ホスト上の Codex Remote projectとして開始する。Remote sessionはそのホストのproject設定、permission、MCP、およびskillを使うため、承認方針、permission profile、別ツール面、およびMCPの無効化は独立cloneの`.codex/config.toml`に固定する。ローカルCLIの`-a`や`-c`だけで設定した値はRemote sessionの開始条件にならない。設定を変更した後は新しいRemote sessionを開始し、設定fileの検査と実効tool一覧の検査を完了してからgoalを開始する。

subagentを使う場合は、Lunaを既定modelとし、同時実行数を1に固定する。primary agentだけがgoalの完了判定、permission変更の要求、および最終統合を担当する。Remote sessionでsubagentの実効tool一覧とsandbox denialを一度確認するまで、subagentを使うgoalを開始しない。

Git directoryをwrite可能にするのは独立cloneだけである。通常checkoutまたはworktreeでは既存refやobject storeを共有し得るため、checkpointの自由度と破損時の復旧可能性を両立する隔離単位として使わない。cloneからremote設定とcredential helperを除き、local Git設定、hook、およびrefが壊れてもbaseline remoteと既存checkoutを変更できないことをpreflightで確認する。local commitは復旧用checkpointであって、完了証明やremote公開ではない。

### Codex の別ツール面

command network の制御は、Web検索、外部MCP、app、connector、browser、Computer Use、およびCodex cloudの通信を拘束しない。自律開発taskでは、必要なCodex側MCP toolだけをserverとtool名の明示的な許可集合として残し、それ以外の別ツール面を無効にする。初期のMVP実装に必要なCodex側MCP toolはない。fake upstream MCPは製品test用のsandbox内child processであり、この許可集合には含めない。後から許可集合を増やす場合は、別ツール面の通信・副作用・credential境界をループ外で確認し、開始条件として実効tool一覧を再検査する。push禁止はgoal上の指示だけに依存せず、command networkのdeny、clone外writeのdeny、および別ツール面の制限を組み合わせる。

モデルとの通信と host sandbox 内の command network は別経路である。モデルを利用できることを、テスト対象が外部 network を利用できる証拠としない。

### テスト対象

MCP Capability Boundary とそこから起動される fake CLI および fake MCP は Codex command sandbox を継承する。ただし、outer sandbox が拒否したことを製品自身の検証成功として数えない。

製品の引数写像、入力拒否、timeout、output 上限、secret redaction などは、test double の観測結果で検証する。製品自身へ OS sandbox 機能を追加する場合は、outer sandbox の影響を分離できる専用 container または VM で別途 conformance test を行う。

Docker socket は host と同等の強い権限を間接的に渡し得るため、通常の自律開発 profile には含めない。container を必要とする試験は、用途を限定した別環境で実行する。

### 通常利用との境界

このpermission profileは、自律的な実装・mock検証にだけ適用する。完成したbrokerをCodexのMCP server設定から起動する通常利用はこの開発goalの外にあり、そのprocessとtargetの権限を本profileで変更しない。通常利用の権限が必要な能力は、開発sandboxを緩めてlive試験するのではなく、同じI/Fを持つ副作用のないfake CLIとfake MCPで検証する。

通常利用時の起動context、Codex設定、およびtarget固有のcredentialは製品導入時に別途構成する。それらが存在することをMVP実装testの前提にせず、outer sandboxの拒否を製品のcapability enforcement成功としても数えない。

## 権限プロファイルの要件

permission profileはこの独立clone専用に作成し、globalな汎用profileとして共有しない。cloneのabsolute path、追加read path、およびtest用write pathは準備時の実測から決める。baselineのremoteと既存checkoutはworkspace rootに加えない。

profile は次の性質を持つ。

1. filesystem root は既定拒否とする。
2. common toolchain に必要な最小 runtime pathと、global AGENTS.md、Codex設定file、memory directory、既存skill directoryだけを read とする。これらのhost固有pathはproject configで`~`から解決し、home directoryやCodex home全体は許可しない。
3. 独立clone rootとその`.git`はwriteとし、local commitを成立させる。
4. 実効permissionとagent設定を置く`.codex`、`.agents`、準備済みvendor、toolchain固定file、およびrelease/CI設定はread-onlyとする。規範文書、`verify`、Cargo manifest、lockfile、およびtestはclone内で編集できるが、変更した規範を自己承認しない。
5. `.env`、credential file、key、token、明示例外以外の利用者設定、およびGit credential helperへのアクセスはdenyとする。Codex設定にsecretが含まれるようになった場合は、全fileのreadを維持せず、ループ外で安全な参照方法を再設計する。
6. build cache と test temporary data は repository 内の専用 directory に置く。
7. system temporary directory は、toolchain が不要なら deny とする。必要な場合も project 固有の path に限定する。
8. command network は disabled とする。
9. Unix socket は許可しない。
10. login shellは使用しない。Gitのglobal/system configとcredential helperは使わず、clone内に準備したlocal author identityだけを使う。

準備フェーズで`.work/cargo-home`、`.work/target`、`.work/tmp`、および`.work/reports`を作り、Git管理外にする。`verify`は`CARGO_HOME`、`CARGO_TARGET_DIR`、`TMPDIR`をそれぞれこの配下のabsolute pathへ固定し、利用者のCargo設定、build cache、およびsystem temporary directoryへfallbackしない。

概念上の profile は次の形になる。実際の path と追加 read 権限は preflight の観察結果で確定する。

```toml
default_permissions = "mcp-boundary-loop"
allow_login_shell = false

[permissions.mcp-boundary-loop]
description = "Offline autonomous development for MCP Capability Boundary."

[permissions.mcp-boundary-loop.filesystem]
":root" = "deny"
":minimal" = "read"
":tmpdir" = "deny"
":slash_tmp" = "deny"
glob_scan_max_depth = 8
"~/.codex/AGENTS.md" = "read"
"~/.codex/config.toml" = "read"
"~/.codex/memories" = "read"
"~/.codex/skills" = "read"
"~/.agents/skills" = "read"

[permissions.mcp-boundary-loop.filesystem.":workspace_roots"]
"." = "write"
".git" = "write"
".codex" = "read"
".agents" = "read"
".github" = "read"
"rust-toolchain.toml" = "read"
"vendor" = "read"
"**/*.env" = "deny"

[permissions.mcp-boundary-loop.network]
enabled = false
```

profileが`:workspace`を継承するか、上記のように明示的な最小profileとするかは、実toolchainのpreflightで決める。`:workspace`の既定では`.git`がread-onlyなので、継承を採る場合も独立cloneの`.git`への明示的なwrite overrideと実commit試験が必要である。どちらの場合もsecret、設定path、clone外write、およびnetworkのdenyを維持する。[Codex権限プロファイル](https://learn.chatgpt.com/ja-JP/docs/permissions)の構成仕様に従い、実効profileをpreflightで確認する。

`Cargo.toml`、`Cargo.lock`、`.cargo/config.toml`、`verify`、および規範文書は試行錯誤のため編集できる。終了判定は、それらの差分をbaselineから検出し、vendorにない依存やtoolchain変更を要する差分を未完成として扱う。規範文書または`verify`の変更が受入条件を緩める場合も未完成とし、ループ外の再承認へ戻す。vendor、toolchain、実効permission、およびループ外の終了判定は自律ループから変更できない。

旧 `sandbox_mode` 設定と permission profile を同じ実行に混在させない。選択した permission system が実際に有効であることを起動後に確認する。

## 準備フェーズ

準備フェーズは対話的な承認方針で行い、次を一度だけ整える。

1. remote repositoryを作り、repository、規範設計、既存差分、およびRust 1.98.1 toolchainを確認する。準備未完了の内容をbaselineとしてpushしない。
2. 実装アーキテクチャに定めたCargo workspaceと最小のcompile可能なcrate境界を作る。
3. 直接依存を選定して`Cargo.lock`へ固定し、`cargo vendor`で`vendor/`へ配置する。
4. Cargoがnetworkと利用者のCargo homeなしに、repository-localな`.cargo/config.toml`と`.work/`だけを使って`--locked --offline`でbuildできる設定を作る。
5. fake CLI、fake MCP、およびtest fixtureの最小skeletonがlive resourceを参照しないことを確認する。
6. `verify`と、規範IDを検査する`tests/requirements.toml`の初期形を作る。
7. filesystem、network、process、およびcommandの必要権限を列挙し、独立clone専用のpermission profileとpreflightを作る。
8. format、lint、compile、skeleton test、およびpreflightが準備環境で成功することを確認する。
9. 準備成果を含むbaseline commitをremoteへpushする。cloneの開始元となるcommit SHAをremoteから読み返して確定し、SHA、規範file、検証契約、およびpermissionのsnapshotをclone外の終了判定側に固定する。baseline commitが判定終了までremoteに残るよう保持する。remote作成とpushは自律ループでは行わない。
10. baselineから既存checkoutとGit directoryを共有せず、Git alternatesも持たない独立cloneを作る。remote設定を削除し、Gitのglobal/system configとcredential helperを使わない起動環境、local author identity、および`.work`を準備する。
11. 独立cloneの`.codex/config.toml`に`approval_policy = "never"`、`default_permissions = "agent-capability-gate-loop"`、別ツール面の無効化、およびCodex側MCPの明示的な無効化を含める。Remote projectとしてそのcloneを開き、新しいsessionでまず`preflight --session`により実効設定と破棄可能なlocal checkpointを確認する。その後にbaseline検証と実際に提示されたtool一覧を確認する。CLIの起動引数やpromptだけを設定根拠にしない。
12. cloneを編集できない別の実行主体が、候補commitの取得、baselineとの差分検査、cleanな候補checkoutでの全検証、および規範からtestへのdrift reviewを行えることを開始前に試す。

準備時に昇格して成功した command は、自律実行の検証済み経路として扱わない。最終 profile で同じ検証が成功しない限り、ループを開始しない。

permission profile、approval、MCP、または別ツール面の設定を変更した session では、その変更が現在の実行へ反映されたとみなさない。Remote project設定を読み直す新しい session で、実効 profileとtool一覧を確認してから続行する。

## 事前検証

preflightはclone内の一時dataと破棄可能なlocal commitだけを使って次を確認する。

- source、規範設計、fixture、および lockfile を読み取れる。
- source、test、規範文書、検証入口、および依存manifestを編集できる。
- cloneの`.git`へlocal commitを積めるが、Git common directoryとobject storeは既存checkoutと独立し、alternatesを持たず、既存checkoutやbaseline remoteのrefには書き込めない。
- `.codex`、`.agents`、vendor、toolchain固定file、CI、および実効permissionへ書き込めない。
- global AGENTS.md、Codex設定file、memory、既存skillを読み取れるが、それらのfileやdirectoryへ書き込めない。Codex home内の他の領域は引き続き読み取り不可である。
- repository 外の sentinel file を読み書きできない。
- `.env` とcredential sentinel、およびCodex home直下の無害なsentinelを読み取れない。
- public、private、および loopback network へ接続できない。
- Unix socket を利用できない。
- compiler、formatter、linter、および test runner を起動できる。
- offline buildと全test suiteが成功する。Gitのglobal/system config、credential helper、およびremote設定を必要としない。
- fake CLI と fake MCP の child process が同じ境界を継承する。
- timeout および cancellation 後に child process が残らない。

否定条件の確認には実 credential や利用者ファイルを使わず、準備フェーズで作る無害な sentinel を使う。Codex home直下のsentinelは、例外的なread pathがhome全体のreadへ広がっていないことを確認する。sandbox denial と test assertion failure を区別して記録する。

remoteのbaselineへのpush拒否を試すために自律sessionから実際のpushを試行しない。cloneにremoteを持たせないこと、command networkとclone外writeがdenyであること、外部tool surfaceが利用できないことを個別に確認する。local file pathへの`git push`もclone外writeを要するなら失敗し、clone内だけのcopyは外部公開とは扱わない。

## テスト構成

[検証設計](verification-design.md)がunit、property、CLI E2E、MCP E2E、test double、および全P/E要件の観測条件を定める。testとfixtureは自律実行中に変更できるが、規範IDとの対応と最終drift検査を維持する。

E2Eは実装内部を直接呼ぶtestだけで代用せず、管理CLI、公開STDIO wire I/F、fake CLI、およびfake upstream MCPを通す。test用の観測file、canary、counterexample、およびreportはrepository-local temporary directoryだけに置く。

### プラットフォーム適合性

macOS と Linux は、それぞれ native または隔離済み CI runner で同じ test suite を実行する。一方の platform だけの成功を release 完了としない。

通常のローカル自律ループは現在の host platform だけを検証する。他 platform の runner が利用できない場合は、その platform を未検証として保持し、対応済みとは表示しない。CI への送信、remote runner の起動、および結果取得はループ外の許可された作業とする。

## 自律修正ループ

```text
設計と受入条件を読む
        |
        v
全検証を実行 ---- 成功 ----> diff と全体を再読
        |                         |
       失敗                       v
        |                    最終検証
        v                         |
失敗を分類                        v
        |                       完了
        v
最小の修正
        |
        v
関連検証 -> 全検証へ戻る
```

各反復は次の順序で行う。

1. baseline SHAに固定された規範、最新のsourceとtest、およびworking tree差分を読む。clone内の規範改訂は提案として区別する。
2. 単一の全検証入口を実行し、失敗を requirement gap、regression、test defect、environment failure のいずれかに分類する。
3. 失敗した受入条件と、観測された根拠を一つに絞る。
4. 合意済みの I/F と不変条件を変えない最小の修正を行う。
5. 影響範囲の狭い検証を実行する。
6. 狭い検証が成功したら全検証を実行する。
7. 新しい失敗には新しい仮説を立て、根拠のない同一 retry を繰り返さない。
8. 全検証成功後に diff と主要 I/F を再読し、不要物、重複、および迂回実装を削る。
9. 規範の各 E2E シナリオとプロパティ要件について検証手段を逆引きし、test との drift を検査する。
10. 整理と drift 修正による変更へ必要な検証を再実行する。

reflection は「不足している要件」「その証拠」「次に行う修正」だけを保持する。長期設計文書へ進捗や試行履歴を追記しない。

permission denial、test失敗、または環境障害を一度受けただけでは利用者へ判断を戻さない。既存の権限と検証契約で可能な代替手段を調べ、根拠のある異なる方法を試す。必須要件を満たす経路がなく、新しい権限、tool、依存、または規範判断が不可欠と分かった場合は、要件、失敗の証拠、試した代替手段、および最小の必要変更を示して利用者へ通知する。通知待ちの間に境界外の実行や検証省略へ切り替えない。

意味のある検証成功ごとにlocal commitをcheckpointとして積める。commitの粒度は復旧と差分reviewのために選び、commit自体を完成条件にしない。local refやhookが壊れた場合はbaseline SHAから独立cloneを作り直し、回収できるcheckpointだけを適用する。既存checkoutとbaseline remoteを復旧操作の対象にしない。

## 検証入口

repository は、実装言語にかかわらず概念上次の入口を持つ。

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

`verify all` はローカルで実行可能な必須検証とrequirement manifest検査を包含し、Cargoを`--locked --offline`で実行する。個別入口は反復速度のために使い、完了判定には代用しない。

test timeout、random seed、parallelism、および fixture path は再現可能な値として記録する。property test が失敗した場合は縮小済み counterexample と seed を保存し、同じ defect の regression test にできるようにする。

## 独立した終了判定

自律loopの`verify all`とself reviewは修正のfeedbackであり、完了証明ではない。loopを停止した後、cloneの外で固定したbaseline SHAと検証契約を持つ判定主体が、次の順で候補を検査する。判定側も候補codeを実行するため、secret、network、Unix socket、およびhostへのwriteを許さない隔離環境を使う。

1. 最終成果をbaselineを祖先に持つ一つのlocal commit SHAへ確定し、tracked working treeをcleanにする。未追跡・ignored fileを検証や成果物へ暗黙に含めない。`.work`は成果物でなく、candidate commitから作るclean checkoutには存在しない。
2. 候補commitを読み取り専用で取得し、判定側が自分のGit設定でclean checkoutを作る。cloneのlocal ref、config、hook、`verify`の成功申告、およびreportを判定の入力として信頼しない。
3. remoteから読み返した固定baseline SHAと候補SHAの全tree entryをpath、type、mode、およびcontent IDで比較する。規範文書、goal受入条件、`.codex`、`.agents`、permission、vendor、toolchain、およびCI/release設定の削除・変更・追加を検出する。clone外の実効permission snapshotも開始時のものと比較する。
4. 規範・permission・受入条件に差分があれば、候補側の新しい条件を採用せず、変更提案として利用者へ戻す。`verify`やCargo manifest／lockfileの差分は、必須検証の欠落、vendor外の依存、または再準備の必要性がないかbaseline契約で判定する。
5. clean checkoutでbaseline検証契約に従う全検証を再実行し、P01–P20とE01–E17を規範からcandidate testへ逆引きしてdriftをreviewする。test codeとfixtureを変更してよいが、requirement IDの記載だけをcoverageとしない。
6. 同じeffective permission profileのpreflightをloop終端で再実行し、開始時snapshotと比べる。final diffは途中で変更して元に戻した事実や実効sandboxの状態を証明しないため、機械差分と実効権限検査を別条件にする。

機械差分の合格はbaselineの破壊が残っていないことだけを示す。途中の一時的な規範・test・Git metadataの改変は最終差分に残らない可能性を受容し、独立したclean checkoutでの再検証と規範からのdrift reviewで成果の品質を判定する。ただし外部副作用とpermissionの自己緩和は事後検知で受容せず、sandboxとtool surfaceで事前に封じる。独立判定が成功するまでpushしない。

## MVP 自律 goal

remote baseline、独立clone、preflight、および終了判定の準備が成功した後、[MVP goal contract](mvp-goal-contract.md)を使って一つのgoalでMVP全体を実装する。CLIとupstream MCPはgoalを分割せず、[実装アーキテクチャ](implementation-design.md)に定めた順序で連続して進める。local commitを積んでもmilestoneの完了だけを理由に利用者へ作業を戻さない。

goalの成果、変更権限、必須検証、および停止条件はcontractに置く。clone内の`verify all`が成功した後も、[独立した終了判定](#独立した終了判定)を通過するまで完了しない。macOSでローカル完了した候補を、未検証のLinuxを含むrelease-readyと表示しない。remoteへの候補pushは自律goalに含めない。

## 停止と再準備

次の場合は、許可済みの代替経路を調べても必須要件を満たせないと確認したうえで変更を積み増さず、候補commitまたは提案差分を残して準備・設計判断へ戻る。

- 必須検証に新しい dependency、network、credential、service、socket、または workspace 外 write が必要になった。
- 実効permission、承認方針、外部tool surface、vendor、toolchain、またはループ外の終了判定を変更する必要が生じた。
- E2E 振る舞い、公開 I/F、trust boundary、または非目標を変える設計判断が必要になった。
- test から規範への対応を説明できず、解決に規範の変更が必要になった。
- remote baseline SHA、独立clone、または終了判定が再現せず、失敗を候補差分へ帰属できない。
- 同じ blocking condition に対して新しい根拠のない反復しか残っていない。
- test が host または live resource に副作用を与えた可能性がある。

停止時は、失敗した受入条件、再現 command、観測した error、必要と判明した権限または設計判断、および最小の推奨変更を示す。権限を暗黙に拡張したり検証を省略したりしない。

## 設計判断

### 準備と反復を分ける

依存取得や権限調整を修正ループ内に置くと、反復のたびに外部状態と権限が変わり、テスト結果を変更差分へ帰属できない。準備フェーズで環境を固定し、自律フェーズを offline にする。

### `never` と sandbox を独立させる

承認方針は停止条件を、permission profile は技術的に可能な操作を決める。自律実行では承認をなくしても、filesystem と network の境界を維持する。

### テストダブルを標準経路とする

本製品は任意 CLI や MCP を実行できるため、live target を使う E2E は副作用と非決定性を持ち込む。protocol、process、および failure mode を再現できる test double を通常の検証対象とし、外部互換性だけを別試験に分離する。

### test は変更可能な翻訳物とする

実装の初期段階では、規範を十分に表す test を最初から完成させることを期待しない。test と fixture の変更を許可し、実装から得た理解を検証へ反映できるようにする。一方、test suite と実装だけの自己整合を完成とはみなさず、goal の終端で規範から検証手段を逆引きすることによって drift を検出する。

### エージェントサンドボックスを製品の安全性と混同しない

outer sandbox は自律開発中の事故を制限する。製品の入力検証や process 制御が正しいことは、fixture の観測と専用 conformance test で証明する。outer sandbox が拒否したという事実だけでは製品要件を満たさない。

## 参照

- [OpenAI Docs: Agent approvals and security](https://learn.chatgpt.com/ja-JP/docs/agent-approvals-security)
- [OpenAI Docs: Permissions](https://learn.chatgpt.com/docs/permissions)
