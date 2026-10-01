# CLI

[English](cli.md) | 日本語

英語版（[cli.md](cli.md)）が正本です。食い違いがあれば英語版に従ってください。

`misao` はデーモンのコマンドラインクライアントです。制御用のアプリが動いていなくても、任意の SSH セッションから使えます。CLI が表示するメッセージは日本語です。この文書は動作・オプション・終了コードを説明します。

関連文書: [プロトコル](protocol.ja.md)、[設定](config.ja.md)、[組み込み（Node SDK）](embedding.ja.md)。

```text
misao [options] <command> [args]
```

## グローバルオプション

すべてのコマンドで、コマンド名の前でも後ろでも指定できます。

| Option | Description |
|---|---|
| `--config PATH` | この `misao.json` を使います（存在しなければエラー）。[設定ファイルの探索](config.ja.md#設定ファイルの探索)を参照してください。 |
| `--json` | 機械可読な出力にします。エラーも stderr に JSON で出力されます（[終了コード](#終了コード)を参照）。 |
| `--help` | ヘルプを表示します。コマンド名の後ろに付けると、そのコマンドの使い方を表示します。 |
| `--version` | バージョンを表示します。 |

環境変数: `MISAO_CONFIG`、`MISAO_DIR`、`MISAO_SOCKET` で設定ファイルとソケットを選びます（[設定](config.ja.md)を参照）。設定の警告（未知のキー）は stderr に出力されます。

## 対象の指定

1 つのペインを操作するコマンドは `<target>` を取ります。`<target>` は `pane.list` に対して**段階的に**解決されます。少なくとも 1 つのペインに一致した最初の段階が結果を決めます。その段階で一致したペインが 1 つならそれが対象です。複数なら、候補を列挙した「ambiguous」エラーで失敗します。どの段階にも一致しなければ「not found」で失敗します。

| Stage | Form | Matches |
|---|---|---|
| 1 | `p_01M3...`（完全な id） | ペイン id の完全一致 |
| 2 | `task:N` | `task` ラベル（先頭の `#` は無視されます） |
| 3 | `agent:ID` | `agent` ラベル |
| 4 | `806` または `W-806` | `windowId` ラベル（hub のウィンドウ番号。`W-` は大文字小文字を区別しません） |
| 5 | `7Q`、`p_01M3`、... | ペイン id の前方一致または後方一致（大文字小文字を区別せず、`p_` は省略可）、**2 文字以上** |
| 6 | 任意のテキスト | `name` ラベルまたはウィンドウ名の部分一致（大文字小文字を区別しません） |

知っておくべきルール:

- **数字だけ**のクエリ（先頭に `W-` を付けてもよい）はウィンドウ番号として扱われ、段階 4 だけで決まります。id や名前には進みません。数字だけのペイン id の断片はウィンドウ番号として解釈されるため、**`p_` を付けて**書いてください（例: `p_12`）。
- `misao ls` は id を `p_01M3…7Q` の形で表示します（先頭 4 文字、`…`、2 文字以上の一意な最短の末尾）。末尾だけ（`7Q`）でも対象として使えます。
- 表の NAME 列には、hub が登録したペイン（`windowId` ラベルを持つ）は `W-806 · display name` と表示され、未登録のペイン（`misao new` で作ったペインなど）は `(未登録) ...` と表示されます。[ラベル規約](embedding.ja.md#ラベル規約)を参照してください。

## コマンド

| Command | Purpose |
|---|---|
| `ls` | ペインを一覧表示します（blocked を先頭に） |
| `attach` | ペインに入ります |
| `new` | ペインを作成します |
| `kill` | ペイン、ウィンドウ、ワークスペースを閉じます |
| `send` | ペインにテキストやキーを送ります |
| `screen` | ペインの現在の画面テキストを出力します |
| `tail` | ペインの出力を行単位で追跡します |
| `events` | デーモンのイベントを追跡します |
| `label` | ラベルを設定または削除します |
| `status` | デーモンの状態を表示します |
| `schema` | プロトコルの JSON スキーマを出力します |
| `serve` | デーモンをフォアグラウンドで実行します |

### ls

```text
misao ls [--state blocked|working|idle|exited] [--task N] [--workspace W] [--json]
```

列: `STATE PANE NAME TASK AGENT CWD LAST`。順序は blocked、working、idle、exited、stopped、unknown で、同じ state の中では出力が新しいものが先です。`--state` はエージェントの state で、`--task` は `task` ラベル（`#` は省略可）で、`--workspace` はワークスペース名で絞り込みます。`--json` はペイン情報オブジェクトの配列を出力します（[プロトコル](protocol.ja.md#メソッド)を参照）。

```bash
misao ls --state blocked
misao ls --task 439 --json
```

### attach

```text
misao attach <target> [--readonly] [--no-replay]
```

端末をペインに接続します。TTY が必要です（なければ、接続の前に終了コード `2` で終了します）。最初に画面のスナップショットを再生します。`--no-replay` を付けるとライブのみになります。`--readonly` は入力を送らず、リサイズもしません。[attach 中のキー操作](#attach-中のキー操作)を参照してください。入力バイトは `source: "terminal"` として送られます。デーモンが記録するのはバイト数だけで、内容は記録しません。

### new

```text
misao new [--cwd DIR] [--label k=v ...] [--env K=V ...] [--workspace W] [--window NAME] [--attach | --json] [-- cmd ...]
```

ペインを作成します。`cmd` を省略するとログインシェルを `-l` 付きで実行します。シェルは `$SHELL` ではなく OS のユーザー情報から取得し、特定できない場合（シェルの登録が無い、またはユーザー情報を読めない場合）は使い方のエラー（終了コード `2`）になります。`--label` と `--env` は繰り返し指定できます。ペインには常に `origin=terminal` が付き、未登録であることを示します。`--workspace` / `--window` を省略するとデーモンのデフォルトのウィンドウが使われます。どちらか一方を指定すると、もう一方は `default` になり、存在しないワークスペースとウィンドウは作成されます。`--attach` はすぐにペインに入ります。`--json` とは併用できません（終了コード `2`）。

`--env` の値は `persistence.json` に保存されます。この方法でシークレットを渡さないでください。プロトコルまたは SDK 経由で `ephemeralEnv` を使ってください（[組み込み](embedding.ja.md#シークレットの渡し方)を参照）。

```bash
misao new --label task=439 --label agent=claude -- claude
misao new --cwd ~/work --attach
```

### kill

```text
misao kill <target> [--window | --workspace] [--force] [--json]
```

対象のペインを閉じます。`--window` はそのペインのウィンドウ全体を、`--workspace` はそのワークスペース全体（すべてのペインを含む）を閉じます。`--force` がなければ `[y/N]` を確認します。TTY がなく `--force` もない場合は拒否されます（終了コード `2`）。no と答えると終了コード `1` で終了します。

### send

```text
misao send <target> [text] [--enter] [--keys Enter,Escape,C-c] [--stdin] [--json]
```

ペインにバイトを書き込みます（`pane.write`）。`text` は入力したとおりに送られます。`-` で始まるテキストは `--` の後ろに置いてください。`--keys` はカンマ区切りの名前を取ります: `Enter`、`Escape`、`Tab`、`Space`、`Backspace`、`Delete`、`Up`、`Down`、`Left`、`Right`、`Home`、`End`、`PageUp`、`PageDown`、および `C-<char>`（コントロールキー）。`Return` と `Esc` はそれぞれ `Enter` と `Escape` の別名で、名前の大文字小文字は区別しません。`--enter` は末尾に Enter を追加します。`--stdin` は標準入力からテキストを読み、`text` とは併用できません。順序は、テキスト、`--keys`、`--enter` の順です。送るものが必要です。

```bash
misao send W-806 'run the tests' --enter
misao send 7Q --keys Escape,C-c
echo 'hello' | misao send task:439 --stdin --enter
```

### screen

```text
misao screen <target> [--lines N] [--json]
```

末尾の空行を除いた現在の画面テキストを出力します。`--lines N` は最後の N 行だけを残します。`--json` は `text`、`cursor`、`altScreen`、`title`、`activity` を出力します。

### tail

```text
misao tail <target> [--since SEQ [--epoch EPOCH]] [--json]
```

Ctrl-C まで行ストリームを追跡します（終了コード `0`）。デフォルトでは、まず保持されている行を再生し、その後は追跡します。`--since` は指定した `seq` の後から再開します（[シーケンス](protocol.ja.md#シーケンス-seq--epoch--since--gap--head)を参照）。`--epoch` がない場合、その seq は現在の epoch のものとみなされ、その旨の警告が出ます。`--epoch` には `--since` が必要です。`--json` は 1 行に 1 つの JSON オブジェクトとして、`seq`、`ts`、`paneId`、`text`、`epoch` を出力します。

追跡は切断を乗り越えます。SDK が再接続して再開し、進捗とギャップは stderr に報告されます。

### events

```text
misao events [--since SEQ [--epoch EPOCH]] [--pane <target>] [--json]
```

Ctrl-C までデーモンのイベントを追跡します。`--since` を指定しない限りライブのみです。`--pane` は 1 つのペインのイベントだけに絞ります。テキスト出力: `seq  ts  type  paneId  data`。`--json` では `epoch` が加わります。

### label

```text
misao label <target> [k=v ...] [--unset k ...] [--json]
```

ラベルを設定し（`k=v`。値は空でもかまいません）、`--unset` でキーを削除したうえで、変更後のラベルを出力します（`k=v` の行、または `{"labels": {...}}`）。

### status

```text
misao status [--json]
```

プロトコルのバージョン、pid、epoch、稼働時間、ペイン数、ソケットを出力します。デーモンが動いていれば終了コード `0`、到達できなければ `1` です（`--json`: `{"running": false, "socket": ...}`）。

### schema

```text
misao schema
```

`server.schema` の結果を出力します。すべてのメソッド、通知、イベント、エラーコードの JSON スキーマです。

### serve

```text
misao serve [--socket PATH] [--data DIR]
```

`SIGINT`、`SIGTERM`、`SIGHUP` を受けるまでデーモンをフォアグラウンドで実行し、その後ペインを閉じて終了します。設定は `misao.json` から読み込まれます。`--socket` はソケットのパスを上書きし、`--data` は `daemon.pid` と `persistence.json` のディレクトリを上書きします（デフォルト: ソケットのあるディレクトリ）。サービスとして実行する方法は [deploy/README.md](../deploy/README.md) を参照してください。

## attach 中のキー操作

attach 中は、**prefix**（デフォルトは `Ctrl-^`）とそれに続くキーを除き、すべてのキーがペインに送られます。これらのキーは [`keys`](config.ja.md#スキーマ) で変更できます。

| Keys | Action |
|---|---|
| `Ctrl-^` `d` | ペインから離れ、ペイン一覧を表示します |
| `Ctrl-^` `n` | 次のペインへ移ります（`misao ls` の順序で、末尾から先頭へ戻ります） |
| `Ctrl-^` `p` | 前のペインへ移ります |
| `Ctrl-^` `l` | ペイン一覧を表示します |
| `Ctrl-^` `Ctrl-^` | `Ctrl-^` のバイトを 1 つペインに送ります |
| `Ctrl-^` と他の任意のキー | そのキーは破棄されます |

入ると、stderr に表示名（`misao ls` の NAME 列）、タスク、フォアグラウンドのコマンド、state、`--readonly` なら `READONLY`、キーのヒントを示すバナーが表示されます。ペイン一覧では、番号でペインに入り、`n` で新しいシェルのペインを作り（`--readonly` では不可）、`q` で終了します（終了コード `0`）。exited と stopped のペインは一覧に表示されません。入っているペインが終了すると、CLI は終了コードを報告して一覧に戻ります。ペインが残っていなければ終了コード `0` で終了します。コマンドラインで指定したペインがすでに終了している場合、そのペインに入ろうとするとエラーになります。接続が失われた場合、またはペインの中か一覧で CLI が終了シグナル（`SIGTERM`、`SIGHUP`、`SIGINT`、`SIGQUIT`）を受けた場合は、端末を復元して終了コード `1` で終了します。`attach` は自分では再接続しません。

## 終了コード

| Code | Meaning |
|---|---|
| `0` | 成功（ペイン一覧から終了した `attach`、Ctrl-C で終了した `tail` / `events` を含む） |
| `1` | 失敗: not found、対象が ambiguous、デーモンに到達できない、RPC エラー、設定エラー、aborted、実行時エラー。デーモンが動いていないときの `status` も含みます |
| `2` | 使い方のエラー: 未知のコマンド、不正または不足した引数、`attach` に TTY がない、併用できないオプション |

`--json` を付けると、エラーは stderr に 1 つの JSON オブジェクトとして出力されます。

```json
{"error":{"code":"ambiguous","message":"...","candidates":[{"paneId":"p_01M3...","name":"W-806"}]}}
```

`code` は `usage`、`not_found`、`ambiguous`、`aborted`、`daemon_unreachable`、`rpc`、`config`、`runtime` のいずれかです。`candidates` は `ambiguous` のときだけ現れます。
