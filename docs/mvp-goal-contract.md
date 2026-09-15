# MCP Capability Boundary MVP goal contract

## 目的

本contractは、準備済みの自律実行環境でMCP Capability BoundaryのMVPを実装し、規範、test、および実装のdriftを解消した状態まで一つのgoalで到達するための条件を定める。

このgoalは、remoteへpushされたbaseline SHAにある[製品設計](design.md)、[実装アーキテクチャ](implementation-design.md)、[検証設計](verification-design.md)、[自律開発設計](autonomous-development.md)、およびループ外で固定した本contractを規範入力とする。clone内の改訂は承認前の提案であり、goalの受入条件を変更しない。

## 開始条件

goalは次が実測済みの場合だけ開始する。

- Rust 1.98.1、Cargo workspace、`Cargo.lock`、vendored dependency、および`verify`が準備されている。
- `cargo`のbuildとtestが`--locked --offline`で起動できる。
- fake CLIとfake MCPのskeletonが実credential、live service、およびnetworkを必要としない。
- remote repositoryを作成済みで、準備済みbaselineをpushし、remoteから読み返したcommit SHAをloop外で固定している。
- baselineから既存checkoutとGit directoryを共有しない独立cloneを作り、remoteとcredential helperを持たせていない。
- 独立cloneのRemote project設定からpermission profileと`approval_policy = never`が適用された新しいsessionで、local commitを含むpreflightが成功している。
- command network、Unix socket、credential、およびworkspace外writeが利用できない。Remote projectの`.codex/config.toml`でCodex側MCPの初期許可集合を空にし、必要性を準備時に確認したtoolだけをserverとtool名で固定して例外とする。それ以外のWeb、MCP、app、connector、browser、Computer Use、およびcloud tool surfaceは利用できない。設定fileの検査に加え、新sessionに実際に提示されたtool一覧でこれを確認する。
- source、test、fixture、文書、`verify`、Cargo manifest／lockfile、およびclone内Git refはwrite可能で、実効permission、vendor、toolchain、およびloop外の終了判定はwrite不能である。
- global AGENTS.md、Codex設定、memory、および既存skillの限定pathを読めるが変更できず、Codex home内のcredentialを含む他の領域は読めない。
- loop外の判定主体がbaselineと候補を比較し、clean checkoutで全検証を再実行できることを準備時に試している。

開始条件の不足をgoal内の実装で迂回しない。remote作成とbaseline push、依存取得、profile作成、独立clone、およびsession再起動は準備フェーズで完了させる。

## 成果

次を一つのMVPとして提供する。

- `mcp-boundary check --config <absolute-path>`
- `mcp-boundary tools --config <absolute-path> --format json`
- `mcp-boundary serve --config <absolute-path>`
- 限定JSON Schema、binding、および固定値からなるcapability boundary
- shellを介さないCLI target実行
- STDIO upstream MCP target実行
- MCP 2026-07-28とlegacy 2025-11-25の公開・upstream互換
- resource、deadline、同時実行、およびoutput上限
- CLIとupstream process groupの終了・回収
- 安定したerror codeと機密値を含まないbroker生成診断
- unit、property、CLI E2E、MCP E2E、requirement mapping、および検証report

## 非目標

goalはHTTP、認証、複数tenant、output property成形、意味validator、optional input、conditional binding、Windows、live target、release、publish、push、merge、およびvendorにないdependencyの取得を実装しない。規範の再設計やMVP外の一般化をreflectionで追加しない。

## 変更権限

goal内では独立cloneのsource、test、fixture、文書、`verify`、Cargo manifest／lockfile、およびlocal Git metadataを変更できる。意味のあるcheckpointとしてlocal commitを積める。testとfixtureは初期状態を正しいと仮定せず、要件の翻訳として修正できる。

clone内の規範、`verify`、manifest、およびlockfileへの改訂は最終差分で検出する。変更済み規範や弱めた検証入口をgoalの成功基準へ採用しない。実効permission、`rust-toolchain.toml`、vendor、CI/release設定、remote、既存checkout、およびloop外の終了判定は変更しない。vendor外依存や新しい権限が必要なら迂回せず停止条件として扱う。

## 実行順序

最初にCLI vertical sliceを完成させる。

```text
config bytes
  -> strict parse and normalization
  -> catalog and tools/list
  -> input validation
  -> resolved CLI invocation
  -> fake CLI process
  -> bounded result and error
```

このsliceでE01からE09、E15、E16と対応するP要件を検証する。成功後は利用者へ中間レビューを依頼せず、同じcore型とerror modelへupstream actorを追加する。

```text
resolved MCP invocation
  -> target actor admission
  -> modern probe
  -> legacy same-process or one-restart fallback
  -> single tool request
  -> bounded result
  -> reuse, cancellation, termination
```

このsliceでE10からE14、E17とP要件のupstream側を完成させる。その後に全検証、drift検査、および全体再読を行う。局所testの成功やmilestone完了をgoal完了としない。

## 反復規則

各反復は、合意した成果に対する不足、観測した証拠、および次に行う最小修正を一組として扱う。変更後は影響範囲の狭い検証を行い、成功したら`verify all`へ戻る。

同じ失敗に根拠のないretryを繰り返さない。test defectはtestを修正し、implementation defectはsourceを修正する。testを合格させるためだけに規範よりassertionを狭めない。新しい設計判断が必要に見えても、既存規範と局所的で可逆な実装判断で解決できる限り作業を継続する。

denialまたは環境障害を受けても、既存の権限と検証契約で成立する別の方法を調べる。利用者への通知は、必須要件を満たす代替経路がなく、新しい権限、Codex側MCP tool、依存、または規範判断が不可欠と具体的な証拠で示せる場合に限る。その場合は試した代替手段と最小の必要変更を添え、境界を自己緩和しない。

## 必須検証

完了候補のlocal commit SHAに対し、[独立した終了判定](autonomous-development.md#独立した終了判定)が次を満たす。

1. baseline SHAと候補SHAの全tracked path差分が機械的に列挙され、規範、permission、受入条件、vendor、toolchain、およびCI/release設定の破壊が残っていない。候補をclean checkoutへ取り出せる。
2. loop外の検証契約に沿う`verify all`が`--locked --offline`の経路で成功する。clone内の成功申告や改訂した`verify`だけに依存しない。
3. P01からP20とE01からE17が`tests/requirements.toml`で検証実体へ対応する。
4. baseline規範から各testを逆引きし、刺激、assertion、mock境界、およびplatform条件を意味的に再検査する。
5. testと実装が同じhelperを誤ったoracleとして共有していない。
6. malformed input、boundary value、failure、timeout、cancellation、およびprocess残存を含む反例が検証される。
7. broker生成の全error pathとstderr診断でsecret sentinelが検出されない。
8. final diffと主要I/Fを再読し、未検証の経路、重複policy、adapterからcoreへのprotocol漏出、およびdebug artifactがない。
9. 同じeffective permission profileでpreflightを再実行し、child processを含むsandbox境界が維持される。
10. macOS native test結果を記録し、未実行のLinuxを対応済みまたはrelease-readyと表示しない。

test変更後の成功は、そのtestが対応する規範を以前より狭くしていないことを説明できる場合だけ採用する。requirement IDの存在確認だけでdrift検査を代用しない。

## 完了条件

成果物、loop外の必須検証、drift検査、および全体再読がすべて成功し、未実行の必須項目がない場合だけgoalを完了する。過去の成功、部分的なtest成功、既存不具合、flaky retry、local commit、またはbudget到達を完成の根拠にしない。候補のremote pushは完成判定の前後いずれも自律loopの権限外とする。

完了報告は、実装した境界、`verify all`とpreflightの結果、requirement coverage、最終drift検査の結果、platform範囲、および残るMVP外項目を区別して示す。

## 停止と再準備

次の場合は変更を積み増さず、同じblocking conditionの証拠と最小の推奨変更をまとめる。

- 必須dependencyまたはtoolchainが準備されていない。
- vendorにないdependency、toolchain、実効permission、またはloop外の検証契約の変更が必要である。
- 必須検証にnetwork、credential、live service、Unix socket、またはworkspace外writeが必要である。
- effective sandboxが開始条件と異なる、またはchild processへ継承されない。
- 規範同士が矛盾し、実装だけでは同時に満たせない。
- remoteのbaseline SHA、独立clone、またはloop外の終了判定が再現せず、失敗をgoal差分へ帰属できない。
- 規範・受入条件・permissionの改訂案なしにはgoalを満たせない、または最終機械差分にそれらの破壊が残る。
- testがhostまたはlive resourceへ副作用を与えた可能性がある。
- 同じblocking conditionに対して新しい証拠または異なる解決仮説が残っていない。
- 利用者が明示した時間、token、反復回数、または費用budgetへ到達した。

停止報告には、失敗した要件ID、再現可能な検証入口、観測結果、必要と判明した新しい権限または設計判断、および再開条件を含める。
