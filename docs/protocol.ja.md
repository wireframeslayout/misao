# プロトコル

[English](protocol.md) | 日本語

英語版（[protocol.md](protocol.md)）が正本です。食い違いがあれば英語版に従ってください。

misao デーモンは、ローカルの Unix ソケット上で JSON-RPC 2.0 API を提供します。このドキュメントはワイヤプロトコル（トランスポート、バージョン、メソッド、通知、シーケンス、購読、稼働判定、エラー）を説明します。パラメータと結果の形については、機械可読な完全スキーマが正本です（[メソッド](#メソッド)を参照）。

関連文書: [設定](config.ja.md)、[CLI](cli.ja.md)、[組み込み（Node SDK）](embedding.ja.md)。

## トランスポート

- Unix ドメインソケット（既定は `~/.misao/misao.sock`。[ソケットの場所](config.ja.md#ソケットの場所)を参照）。
- NDJSON: 1 行に JSON-RPC 2.0 メッセージを 1 つ置き、`\n` で終端します。空行は無視されます。
- 1 行の上限は 8 MiB です。これを超える行は `id: null` の `-32700` エラーになり、接続は閉じられます。
- メッセージは 3 種類です。リクエスト（`id` + `method`）、レスポンス（`id` + `result` または `error`）、サーバー通知（`id` なしの `method`）。クライアントが送った通知とレスポンスは、デーモンが無視します。
- レスポンスは `result` か `error` のどちらか一方だけを持ちます。返すものがないメソッドは `null` または `{ "ok": true }` を返します。

```json
{"jsonrpc":"2.0","id":1,"method":"server.info","params":{}}
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"0.2.0","pid":4242,"epoch":"01M3RCFVKNN4GWG2FBNS6BVTW2","uptimeSec":12,"paneCount":3,"eventHead":57}}
```

ソケットのディレクトリはモード `700`、ソケットはモード `600` で作成されます。アクセス制御は、現在のユーザーのファイルシステム権限によります。

## バージョンと互換性

`PROTOCOL_VERSION`（現在は `0.2.0`）は semver で、`server.info` と `daemon.started` イベントで報告されます。

- **互換**とは、**メジャー**バージョンが等しいことです（`isCompatibleProtocolVersion`）。SDK は接続の直後にこれを確認し、満たされない場合は `MisaoProtocolVersionError` で reject します。
- フィールド、メソッド、イベント種別、enum 値の追加はマイナー変更で、後方互換です。削除や意味の変更はメジャー変更です。

前方互換性のルール（相手側が許容しなければならないこと）:

| 方向 | ルール |
|---|---|
| リクエストの `params`（クライアントからデーモン） | スキーマで検証されます。**未知のキーは取り除かれます**（無視）。値は有効でなければならず、未知の enum 値は `-32602` で拒否されます。唯一の例外が `pane.list` の `filter` で、未知の条件を拒否します。古いデーモンがフィルタを無視して全ペインを返すことがないようにするためです。 |
| `result` と通知の `params`（デーモンからクライアント） | **寛容**: 未知のフィールドは無視しなければなりません。 |
| `result` / 通知 / イベント `data` 内の enum 値 | 受信側は未知の値を許容します。スキーマは `"unknown"` にフォールバックします（`processState`、`agentState`、イベント `input.source`、`pane.output` の `replay`）。デーモン自身は `processState`、`input.source`、`replay` に `"unknown"` を送りません。 |
| イベントの `type` | 未知の `type` はエラーではありません。`parseKnownEvent(event)` は既知の種別の `data` を検証し、未知の種別には `undefined` を返します。 |

注: `agentState` はデーモンが計算する状態で、`unknown` も実際の値です。どのルールもまだ判断していないペインの初期状態です（[稼働判定](#稼働判定)を参照）。

予約済みで利用できないもの:

- `mode: "cells"` の `pane.attach` は `1004`（Unsupported）を返します。動作するのは `mode: "raw"` だけです。
- `pane.respawn` はスキーマにのみ存在し、`1005`（NotImplemented）を返します。
- `window.focus` はスキーマにのみ存在し、`1005`（NotImplemented）を返します。`focus` イベントはスキーマに定義されていますが、まだ送られません。

## メソッド

パラメータと結果はここで要約しており、完全には繰り返しません。完全な JSON スキーマ（メソッド、通知、イベント、エラーコード）はデーモンから取得できます。

```bash
misao schema          # prints the result of server.schema as JSON
```

または `server.schema` メソッドで取得します。特に記載がない限り、`paneId` を取るメソッドは、ペインが存在しないと `1001` を返します。例外は `pane.detach` で、ペインや attach が存在しなくても `ok` を返します。

| メソッド | 目的 | 主なパラメータ | 結果 |
|---|---|---|---|
| `server.info` | デーモンの識別情報とストリームの head | なし | `protocolVersion`, `pid`, `epoch`, `uptimeSec`, `paneCount`, `eventHead` |
| `server.schema` | プロトコル全体の JSON スキーマ | なし | `{ protocolVersion, jsonrpc, methods, notifications, events, errors }` |
| `workspace.list` / `workspace.create` / `workspace.close` / `workspace.rename` | ワークスペースを管理する | `name`（rename は `newName`） | ワークスペース情報（`name`, `windows`）または `ok` |
| `window.create` / `window.close` / `window.rename` | ワークスペース内のウィンドウを管理する | `workspace`, `name`, `windowId` | ウィンドウ情報または `ok` |
| `window.focus` | スキーマのみ | `windowId`, `clientId` | 常に `1005` |
| `pane.open` | 新しい PTY でコマンドを開始する | `cmd`（必須）, `cwd`, `env`, `ephemeralEnv`, `cols`, `rows`, `labels`, `windowId` | `{ paneId }` |
| `pane.info` / `pane.list` | ペインの状態を読む | `paneId`; `filter`（`state`, `labels`, `workspace`。AND 条件） | ペイン情報（単体または配列） |
| `pane.write` | PTY にバイト列を書き込む | `paneId`, `data`（UTF-8）**または** `dataB64`, `clientId`, `source`（`hub` / `terminal`） | `ok` |
| `pane.send_keys` | スキーマのみ | `paneId`, `keys` | 常に `1005`。`pane.write` を使ってください |
| `pane.resize` | PTY をリサイズする | `paneId`, `cols`, `rows`, `clientId` | `ok` |
| `pane.screen` | 現在の画面テキスト | `paneId` | `text`, `cursor`, `altScreen`, `title`, `activity` |
| `pane.set_label` | ラベルを設定 / 解除する | `paneId`, `set`, `unset`（少なくとも 1 つ） | 変更後の `{ labels }` |
| `pane.attach` / `pane.detach` | 生の出力をこの接続へストリームする | `paneId`, `clientId`, `mode`（既定は `raw`）, `replay`（`raw` / `snapshot` / `none`。既定は `none`）, `cols`, `rows` | attach: `head`, `oldest`, `truncated` |
| `pane.subscribe_lines` | 行ストリームを購読する | `paneId`, `since`, `epoch` | `gap`, `head`, `epoch` |
| `events.subscribe` | デーモン全体のイベントを購読する | `since`, `epoch` | `gap`, `head`, `epoch` |
| `pane.close` | ペインを閉じる（SIGHUP、終了を待つ） | `paneId` | `ok` |
| `pane.respawn` | 予約済み | `paneId`, `cmd` | 常に `1005` |

補足:

- `pane.open` はスキーマ上 `preplace` を受け付けますが、指定されるとデーモンは `1005` を返します。
- `pane.write` が記録する `input` イベントは、ソースとバイト数のみです。内容は記録されません。
- `pane.list` の `filter.labels` は、指定したキーと値がすべて等しいときに一致します。
- `replay: "raw"` の `pane.attach` は、保持している生のリングを再生します（すでにバイトが破棄されていれば `truncated: true`）。`replay: "snapshot"` は最初に画面スナップショットを 1 つ送ります（`replay: "snapshot"` の `pane.output` 通知）。`none` はライブのみです。再生の後、ストリームは `head` の位置で gap なくライブ出力に切り替わります。
- ペイン ID は `p_` + ULID、ウィンドウ ID は `w_` + ULID です（デーモン側の ID）。

## 通知

通知はすべてサーバーからクライアントへ送られ、`seq` と `ts`（ISO 8601 UTC、デーモンが記録）を持ちます。

| 通知 | 配信される契機 | パラメータ |
|---|---|---|
| `pane.output` | `pane.attach` の後 | `seq`, `ts`, `paneId`, `dataB64`（生の PTY バイト列）, 任意の `replay`（`"snapshot"`） |
| `pane.line` | `pane.subscribe_lines` の後 | `seq`, `ts`, `paneId`, `text`（1 行。ANSI シーケンスは除去済み） |
| `event` | `events.subscribe` の後 | `seq`, `ts`, `type`, 任意の `paneId`, `data` |

既知のイベント種別（それぞれの `data` のスキーマは `server.schema` にあります）:

| グループ | 種別 |
|---|---|
| daemon | `daemon.started` |
| pane | `pane.opened`, `pane.title`, `pane.exited`, `pane.resized`, `pane.closed`, `pane.state`, `pane.label` |
| input / clients | `input`（ソースとバイト数のみ）, `client.attached`, `client.detached` |
| layout | `workspace.created`, `workspace.closed`, `workspace.renamed`, `window.created`, `window.closed`, `window.renamed`, `focus`（予約済み。まだ送られません） |

ペインに紐づくイベントでは、`paneId` は `data` の中ではなく `data` の隣に置かれます。

## シーケンス: seq / epoch / since / gap / head

互いに独立したストリームが 3 つあります。

1. 生の出力（ペインごと。`pane.output`。`seq` はチャンクを数えます）
2. 行（ペインごと。`pane.line`）
3. デーモン全体のイベント（`event`）

| 用語 | 意味 |
|---|---|
| `seq` | ストリームごとのカウンタで、1 から 1 ずつ増えます。空のストリームは `head = 0` です。 |
| `head` | 購読時点の最新の `seq`。再生は `seq <= head`、ライブ配信は `seq > head` を対象にします。 |
| `oldest` | リングにまだ保持されている最も古い `seq`（空のときは `head + 1`）。 |
| `since` | クライアントがすでに持っている最後の `seq`。デーモンは `seq > since` を再生し、その後ライブに移ります。省略するとライブのみです。 |
| `epoch` | デーモンの起動ごとに生成される ULID。`server.info` と、すべての購読結果で報告されます。 |
| `gap` | クライアントが取りこぼしたとき、購読結果で `true` になります。 |

購読は、`since` を、それが属する `epoch` と**一緒に**渡して再開します。

- `gap` が `true` になるのは、`since < oldest - 1`（クライアントが一度も見ていない項目をリングが破棄した）、`since > head`（デーモンが再起動して `seq` が巻き戻った）、または `since` を渡していて、渡した `epoch` が現在のものと異なる場合です。`epoch` が比較されるのは `since` を渡したときだけです。`since` なしで `epoch` だけを渡すと、ライブのみの購読になり `gap: false` です。
- `since` を渡していて、渡した `epoch` が現在の epoch と異なるとき、デーモンは `since` を `0` として扱います。保持している最も古い項目から再生し、`gap: true` を返します。クライアントは保存していた `since` を破棄しなければなりません。デーモンの再起動を確実に検出できるのは `epoch` の比較だけで、`since > head` は二次的なシグナルです。
- `since` がリングより古いとき、デーモンは `oldest` から再生し、`gap: true` を返します。
- `epoch` を省略すると、デーモンは世代を比較できず、上記の `since` のチェックにフォールバックします。必ずペアで送ってください（SDK の型がこれを強制します）。
- 同じ接続で同じ `(stream, pane)` に新しく購読すると、古い購読が置き換えられるため、クライアントが重複を受け取ることはありません。
- デーモンは、同じ接続で、再生の通知より先に購読のレスポンスを送ります。クライアントはレスポンスを先に処理しなければなりません（`head` と `gap` が設定されます）。

リングの容量（デーモンの起動ごと。[設定](config.ja.md#スキーマ)を参照）:

| リング | 容量の単位 | 既定値 |
|---|---|---|
| 生の出力（ペインごと） | バイト | 1 MiB |
| 行（ペインごと） | バイト（文字数 + 1 行あたり 64） | 4 MiB |
| イベント（デーモン全体） | 項目数 | 1000 |

リングは、その 1 項目だけで容量を超える場合でも、常に最新の項目を少なくとも 1 つ保持します。

## 購読: pull 型とバックプレッシャー

行とイベントの購読は **pull 型**です。各購読は自分のカーソル（最後に送った `seq`）を持ち、接続の送信キューが 1 MiB を下回っている間だけリングから読みます。新しい項目が到着したときと、ソケットが空いたときに続行します。そのため、遅い購読者は送信キューのメモリではなくリングが吸収します。

- 購読者が切断されるのは、**リングに追い越された場合だけ**です。つまり、カーソルの次の項目がすでに破棄されたときです。この判定はソケットが書き込み可能でない間も行われるため、まったく読まないクライアントも検出されます。
- 16 MiB の送信キュー上限が適用されるのは、**`pane.attach` の生の出力とレスポンスだけ**です。キューがこれを超えた接続は閉じられます。行とイベントの購読はこの上限を使いません。
- 購読者が追い越されて切断されると、SDK は再接続し、最後の `since` と `epoch` で再購読します。デーモンは `gap: true`（epoch の変更なし）を返し、SDK は理由 `truncated` で `gap` を報告します。[組み込み](embedding.ja.md#ストリームを追う)を参照してください。
- 同じ接続で `pane.attach` と購読を併用する場合: 生の出力によってキューが高水位を超え続けている間は、購読も一時停止します。

実測した挙動（目安にすぎません。マシンと Node のバージョンに依存します）:

| 計測 | 結果 |
|---|---|
| pull 型の導入前、100 ラウンドのバーストテスト | 切断 6 回。マーカーの検出は 100 件中 57 件 |
| pull 型、既定の 4 MiB の行リング、1000 ラウンドのバーストテスト | 切断 0 回、gap 0 回、マーカー 1000 件中 1000 件を検出、毎秒約 57,000 行 |
| Phase 0 スパイク、1000 ラウンドのバースト | 165 秒で 10,005,000 行、`seq` は連続、マーカー 1000 件中 1000 件を検出 |
| Phase 0 スパイク、長時間読み取りを止めた購読者 | 再開時に `gap: true` を通知された。リング内で再開したクライアントは連続した `seq` を見た |

## ペインのライフサイクルと永続化

- `pane.open` は PTY 上で `cmd` を起動します。子プロセスの環境は、デーモンの環境から `TMUX`、`TMUX_PANE`、`STY`、`ZELLIJ*` を取り除き、`MISAO_SOCKET`、`MISAO_PANE_ID`、`TERM=xterm-256color`、`COLORTERM=truecolor` を加え、続いて `env`、さらに `ephemeralEnv` を重ねたものです。
- 終了したペインは、閉じられるまで `processState: "exited"`、`exitCode`、`signal` とともに一覧に残ります。`pane.close` で削除されます。
- **`ephemeralEnv`**: `env` と同様に子プロセスへ注入される環境変数ですが、`persistence.json` には保存されず、`pane.info`、`pane.list`、イベントにも表示されません。トークンなどの秘密情報に使ってください。`env` は永続化されます。同じキーでは `env` を上書きします。デーモンの再起動後、ペインは `stopped` になり値は失われるため、将来の respawn では再度渡す必要があります。
- **`persistence.json`**: デーモンは、ワークスペース、ウィンドウ、ペインの記録（`cmd`、`cwd`、`env`、`labels`、サイズ）を、データディレクトリ（既定ではソケットのディレクトリ）の `persistence.json` に、`fsync` を伴ってアトミックに保存します。サイズの変更は少し遅れて保存され、定義とラベルの変更はレスポンスの前に保存されます。ファイルが壊れている場合や `version` が未知の場合、デーモンは起動しません。
- **`stopped`**: デーモンが再起動すると PTY は失われ、メタデータだけが残ります。復元されたペインは `processState: "stopped"`、`pid: null`、`agentState: "unknown"`、`decidedBy: "none"` で現れます。`stopped` のペインに対する入力、リサイズ、screen、attach、行の購読は `1002` を返します。`pane.close` で記録は削除されます。記録からのペインの再起動（`pane.respawn`）はまだ実装されていません（`1005`）。
- デーモンの `epoch` は起動のたびに変わるため、クライアントは再起動を検出できます（上記を参照）。

## 稼働判定

デーモンはペインごとに `agentState` を計算し、`pane.info` / `pane.list` と、`pane.state` イベント（`state`、`decidedBy`、`prev`）で報告します。ルールは次の優先順位で評価され、判断を持った最初のルールが決定します。

| 優先度 | ルール（`decidedBy`） | 決定内容 |
|---|---|---|
| 1 | exit（`exit`） | プロセスが終了したとき `exited`。終端状態で、以降は何も評価されません。 |
| 2 | profile（プロファイルの `name`） | エージェント固有の画面ルール。`blocked` と判断できる唯一のルールです。 |
| 3 | title（`title`） | OSC ウィンドウタイトル: スピナーのタイトル（点字、`◐◑◒◓`、`✻✶✽✢∗` の後にスペース）は、更新され続けている間は `working`（3 秒で古くなり、その後は判断なし）。一度スピナーを見た後は、空でないスピナー以外のタイトルは `idle`。 |
| 4 | bytes（`bytes`） | 出力量: 直近 3 秒に 200 バイト以上ある 1 秒ティックが 2 回連続したら `working`、5 秒間アクティビティがなければ `idle`。入力の直後（500 ms）やリサイズの直後（800 ms）の出力は数えません。 |

- コアが出すのは `working`、`idle`、`exited` だけです。`blocked` はプロファイルからのみ出ます。
- 初期状態は `agentState: "unknown"`、`decidedBy: "none"` です。いずれかのルールが判断を持つまでこのままです（bytes ルールでは、早くてもオープンの 5 秒後か、アクティブなティック 2 回後です）。
- どのルールも判断を持たない場合、状態はそのままです。
- プロファイルは、最初に `matches(cmd)` が真になったものが選ばれる、純粋な `classify(screen)` 関数です。`DaemonOptions.profiles` を通じてデーモンに組み込みます。[エージェントプロファイル](embedding.ja.md#エージェントプロファイル)を参照してください。3 回連続で throw したプロファイルは、そのペインでは無効になります（警告がログに出ます）。1 回の失敗は判断なしとして扱われます。
- プロファイルは出力が落ち着いた後に評価されます（120 ms のデバウンス、最初の出力から最大 300 ms 後）。

## エラー

エラーレスポンスの形式は `{ "code": <int>, "message": <string>, "data"?: ... }` です。メッセージは人間向けで、スタックトレースを含みません。判別には `code` を使ってください。

| コード | 名前 | 意味 |
|---|---|---|
| -32700 | Parse | 行が有効な JSON でないか、8 MiB の行上限を超えている（その場合、接続は閉じられます）。 |
| -32600 | InvalidRequest | JSON オブジェクトでないか、有効な JSON-RPC リクエストでない。 |
| -32601 | MethodNotFound | 未知のメソッド。 |
| -32602 | InvalidParams | `params` の検証に失敗した（メッセージは `path: reason` を列挙します）。プロセスを起動できないときの `pane.open` も返します。 |
| -32603 | Internal | 想定外のデーモンエラー。 |
| 1001 | PaneNotFound | その ID のペインがない。 |
| 1002 | PaneExited | ペインが終了している（write）、または `stopped` である（生きたプロセスを必要とする操作すべて）。 |
| 1003 | Ambiguous | 予約済み。デーモンは返しません。CLI が対象の解決をクライアント側で行います。 |
| 1004 | Unsupported | `mode: "cells"` の `pane.attach`。 |
| 1005 | NotImplemented | `pane.respawn`、`pane.send_keys`、`window.focus`、`pane.open` の `preplace`。 |
| 1006 | WorkspaceNotFound | ワークスペースが存在しない（または閉じている途中）。 |
| 1007 | WindowNotFound | ウィンドウが存在しない（または閉じている途中）。 |
| 1008 | AlreadyExists | その名前のワークスペースがすでに存在する。 |

SDK はこれらを `code` を持つ `MisaoRpcError` として表面化します。[組み込み](embedding.ja.md)を参照してください。
