# Debug 観測性の設計

## 目的

MCP Capability Boundary は、target 呼び出しの成否を operator が追跡できる最小限の debug 観測性を提供する。これは、MCP client への診断開示、監査記録、または target 出力の収集機能ではない。

## 有効化と出力先

通常起動では、tool call ごとの運用イベントを出力しない。broker 起動時に `MCP_BOUNDARY_LOG=debug` を明示した場合だけ、debug event を stderr へ JSON Lines 形式で出力する。stdout は常に MCP JSON-RPC 専用であり、debug event を一byteも含めない。

環境変数は起動 context の一時的な選択であり、能力設定へ保存しない。無効値または未設定は debug を有効にしない。ログファイル、外部 sink、rotation、およびログレベルの動的変更は提供しない。

## event 契約

debug を有効にした broker は、公開 `tools/call` ごとに一つの完了 event を出す。event は少なくとも次を持つ。

- `event`: `tool_call_finished`
- `correlation_id`: broker lifetime 内で一意の呼び出し識別子
- `tool`: 公開 tool 名
- `target`: target を選択する前の拒否では省略し、それ以外では target ID
- `outcome`: `success`、`error`、または `rejected`
- `code`: error または rejected 時の stable error code

同じ `correlation_id` は一つの公開 call にだけ対応する。event は MCP wire response の前後順序を保証せず、client が解釈するプロトコル情報でもない。

## 情報境界

通常の stderr diagnostic と MCP response は、executable、cwd、argv、environment value、公開 input、upstream arguments、および target stdout/stderr を含めない。debug は operator が明示的に開く診断境界であり、target stderr を event に含める。したがって、debug の stderr は秘密を含み得る一時的な operator 出力として扱い、通常ログや MCP client へ転送してはならない。

target stderr は `target_stderr: {"encoding":"utf8|hex","value":"..."}` として表現する。UTF-8 として妥当な bytes は `utf8` の JSON string にし、妥当でない bytes は `hex` の小文字 hexadecimal にする。これにより JSONL を壊さず、任意の bytes を失わずに扱える。target の終了状態は `target_exit: {"code": N}` または `target_exit: {"signal": N}` とする。upstream MCP のように process が継続する場合は終了状態を省略し、call 間に収集した stderr だけを載せる。

target の非0 exit、timeout、起動不能、I/O failure、出力上限、出力不正、cancellation は stable code で識別する。stderr は configured limit 内だけを保持し、limit 超過時は raw bytes を debug event に出さない。

## 受入条件

- debug 無効時、正常・失敗・拒否の tool call は stdout に正しい MCP response だけを出し、call event を stderr に出さない。
- debug 有効時、各 tool call は parse 可能な一つの JSON event を出し、結果種別と stable code が MCP response と矛盾しない。target が stderr を出した場合、event は上記の encoding でそれを含める。
- debug 無効時、target が出す secret sentinel、入力、環境値、および生の stderr は debug event、通常 diagnostic、MCP response のいずれにも現れない。
- debug 有効時も、target stderr は stdout または MCP response に現れず、event の `target_stderr` にだけ現れる。任意の bytes は JSONL を壊さず復元可能である。
- 既存の設定失敗と致命的 shutdown の stderr diagnostic は維持する。

## 再検討条件

複数利用者の事後追跡、長期保存、検索、集計、または target stderr を含む調査が運用要件になった時点で、監査 sink と安全な詳細診断を独立した設計として再検討する。
