# 組み込み

[English](embedding.md) | 日本語

英語版（[embedding.md](embedding.md)）が正本です。食い違いがあれば英語版に従ってください。

SDK（`@misao/sdk`）を使って自分の Node.js プログラムから misao を操作する方法と、組み込むアプリが従うべき規約（ラベル、シークレット、エージェントプロファイル）を説明します。ワイヤ上の意味（`seq`、`epoch`、`gap`、pull 型の購読、稼働判定）は [プロトコル](protocol.ja.md) に一箇所だけ定義しており、ここからはリンクするだけにしています。

関連文書: [プロトコル](protocol.ja.md)、[設定](config.ja.md)、[CLI](cli.ja.md)。

パッケージ（`@misao/sdk`、`@misao/protocol`、`@misao/daemon`）は現時点ではこのリポジトリのワークスペースパッケージであり、npm には公開していません。ESM で、Node.js 24 以降が必要です。

## 接続

```ts
import os from 'node:os';
import { MisaoClient, resolveSocketPath } from '@misao/sdk';

const socketPath = resolveSocketPath({ env: process.env, homeDir: os.homedir() });
const client = new MisaoClient({ socketPath });
await client.connect();

const panes = await client.request('pane.list', {});
client.close();
```

- `resolveSocketPath` は CLI と同じ順序で解決します。`$MISAO_SOCKET`、次に `explicitPath`（たとえば `misao.json` の `socket`）、次に `$MISAO_DIR/misao.sock`、次に `~/.misao/misao.sock` です。パスが `MAX_SOCKET_PATH_BYTES`（107 バイト）より長い場合は `MisaoPathError` を投げます。[ソケットの場所](config.ja.md#ソケットの場所) を参照してください。
- `MisaoClientOptions`: `socketPath`（必須）、`backoff`（部分指定可、下記参照）、`connectTimeoutMs`（既定値 `5000`）。
- `connect()` は接続し、`server.info` を呼び、プロトコルのメジャーバージョンが一致することを確認し、ストリームを復元します。デーモンに到達できない場合（`MisaoConnectionError`）や互換性がない場合（`MisaoProtocolVersionError`、[バージョン](protocol.ja.md#バージョンと互換性) を参照）は reject されます。呼び出せるのはクライアントがアイドルのときだけで、最初の試行の前か、失敗した試行の後です。
- `connectTimeoutMs` は、ソケット接続後のセットアップ（`server.info` の確認とストリームの復元）に時間制限を設けます。タイムアウトすると接続を閉じ、`connect()`（または現在の再接続の試行）は `MisaoConnectionError` で失敗します。これによりデーモンのハングから保護されます。
- `request(method, params)` は `@misao/protocol` によって型付けされています。デーモンのエラーは `MisaoRpcError`（`code`、`message`。[エラー](protocol.ja.md#エラー) を参照）で reject されます。未接続の間に呼び出すと `MisaoConnectionError` で reject されます。
- `close()` はクライアントを終了し、再接続を止めます。

## 再接続とバックオフ

`connect()` が一度成功した後は、接続が切れても `close()` を呼ぶまで再試行します。

| オプション（`backoff`） | 既定値 | 意味 |
|---|---|---|
| `initialDelayMs` | `100` | 最初の再試行までの遅延 |
| `maxDelayMs` | `5000` | 遅延の上限 |
| `factor` | `2` | 試行ごとの倍率 |

試行 `n`（1 から始まる）の前の遅延は `min(maxDelayMs, initialDelayMs * factor^(n-1))`（`computeBackoffDelay`）です。試行回数の上限はありません。クライアントが諦めるのは、`close()` が呼ばれた場合と、再起動後のデーモンがプロトコル非互換だと分かった場合だけで、その場合は状態が `closed` になり `cause` が付きます。切断時に処理中だったリクエストは `MisaoConnectionError` で reject されます。

```ts
client.onStateChange((state) => {
  // { status: 'connected' }
  // { status: 'reconnecting', attempt, delayMs, cause }
  // { status: 'closed', cause? }
});
```

`connected` は、最初の `connect()` が解決した後にも一度発行され、再接続が成功するたびにも発行されます。

## ストリームを追う

```ts
import { parseKnownEvent } from '@misao/protocol';

const lines = await client.subscribeLines(paneId, (line) => {
  console.log(line.seq, line.text);
});

const events = await client.subscribeEvents((event) => {
  const known = parseKnownEvent(event);      // このバージョンが知らないイベント種別では undefined
  if (known?.type === 'pane.state') console.log(known.paneId, known.data.state);
});

// 後で、たとえばプロセスを再起動する前に:
const saved = lines.cursor;                  // { seq, epoch }
lines.unsubscribe();
```

- オプションなしの購読はライブのみです。再開するには **`since` と `epoch` を一緒に** 渡します: `{ since: saved.seq, epoch: saved.epoch }`。片方だけ渡すことは型で禁止されています。`epoch` がないと、再起動で `seq` が巻き戻ったかどうかをデーモンが判断できないためです。`Subscription.cursor` は常に最新の位置を返し、そのまま保存できます。
- 接続中、SDK は各ストリームの最後の `seq` を保持して重複を捨て、再接続後は、その `since` と見ていた `epoch` ですべてのストリームを再購読します。手動で再購読する必要はありません。
- デーモンの `epoch` が変わっていた場合（デーモンが再起動した場合）、SDK は位置を `0` にリセットします。デーモンは保持している最も古い項目から再送し、`onGap` が理由 `epoch` で発火します。
- 位置がリングから外れた場合（またはリングに追い越されたために SDK が切断された場合。[pull 型](protocol.ja.md#購読-pull-型とバックプレッシャー) を参照）、デーモンは `gap: true` を返し、`onGap` が理由 `truncated` で発火します。
- 購読は 1 ストリーム（events、または 1 つのペインの lines）につき 1 つしか作れません。2 つ目は例外を投げます。
- ハンドラーの例外は配信を止めません。`onError` に報告されます。

## コールバック

各 `on...` メソッドは、そのコールバックを削除する関数を返します。

| メソッド | 呼ばれるとき | 引数 |
|---|---|---|
| `onStateChange` | 接続、再接続中、または終了のとき | `ConnectionState` |
| `onGap` | ストリームで項目を取り逃したとき | `{ stream, reason }`。`stream` は `{ kind: 'events' }` または `{ kind: 'lines', paneId }`、`reason` は `'epoch'` または `'truncated'` |
| `onSubscriptionError` | デーモンが再購読を拒否したとき（たとえばペインが存在しない）。そのストリームは破棄されます。 | `{ stream, error: MisaoRpcError }` |
| `onNotification` | ストリーム購読ではない通知、つまり `pane.output` のとき | `{ method, params }` |
| `onError` | 登録したコールバックまたはハンドラーが例外を投げたとき | `unknown` |

`onGap` からの復旧方法: 取り逃した出力はストリームからは失われているので、スナップショット（たとえば `pane.screen` や `pane.info`）から状態を作り直し、新しい位置から続けます。コールバックの中では `client.request` がすでに使えます。ストリームが復元される前から接続は使用可能だからです。

`onError` は、自分のコールバックが投げた例外を受け取る唯一の場所です。例外が再接続、配信、gap 通知を止めることはありません。`onError` リスナーがない場合、SDK は `console.error` に出力します。

## 再接続後の attach のやり直し

`pane.attach`（生の出力）には `since` がないため、SDK は復元 **しません**。再接続のたびに、もう一度 attach する必要があります。出力ハンドラーは一度だけ登録します。`onNotification` のリスナーは再接続後も残ります。

```ts
const clientId = 'my-app-1';

client.onNotification((n) => {
  if (n.method === 'pane.output' && n.params.paneId === paneId) {
    term.write(Buffer.from(n.params.dataB64, 'base64'));
  }
});

const attach = (): Promise<unknown> =>
  client.request('pane.attach', { paneId, clientId, replay: 'snapshot', cols, rows });

await attach();
client.onStateChange((state) => {
  if (state.status === 'connected') attach().catch((error) => report(error));
});
```

- `replay: "snapshot"` は、最初に画面全体を 1 回送ります（`replay: "snapshot"` の `pane.output`）。描画する前にターミナルビューをリセットしてください。`replay: "raw"` は代わりに生のリングを再送します（すでにバイトを失っていれば `truncated` で分かります）。`"none"` はライブのみです。
- デーモンの再起動後、ペインは `stopped` になり、attach は `1002` を返します。先に `pane.info` を確認してください（[ペインのライフサイクル](protocol.ja.md#ペインのライフサイクルと永続化) を参照）。
- 同じ接続で同じペインを再度 attach すると、前の attach が置き換えられます。
- 入力は `pane.write` に `source: 'hub'`（自分のアプリ）または `'terminal'` を指定して送ります。デーモンが記録するのはバイト数だけです。

## シークレットの渡し方

`pane.open` の `env` は `persistence.json` に保存されます。トークンなどのシークレットは `ephemeralEnv` で渡してください。子プロセスには届きますが、永続化されることはなく、`pane.info`、`pane.list`、イベントにも現れません。

```ts
const { paneId } = await client.request('pane.open', {
  cmd: ['claude'],
  cwd: '/work/repo',
  env: { MY_APP_MODE: 'agent' },
  ephemeralEnv: { MY_APP_TOKEN: token },
  labels: { owner: 'my-app', task: '439', agent: 'claude', origin: 'hub' },
});
```

デーモンの再起動後、ペインは `stopped` になり、シークレットは失われます。再 spawn が実装されたとき（未実装、`1005`）は、呼び出し側が `ephemeralEnv` をもう一度渡す必要があります。同じキーでは `ephemeralEnv` が `env` を上書きします。

## ラベル規約

ラベルは自由形式の文字列ペアで、**デーモンはその意味を解釈しません**。アプリと CLI がペインの記述方法をそろえるためのものです。`pane.set_label` か `pane.open` の `labels` を使い、`pane.list` の `filter.labels`（すべてのキーと値が等しいときに一致）で絞り込みます。

| キー | 意味 | 例 |
|---|---|---|
| `owner` | ペインを所有するアプリまたはユーザー | `azito` |
| `task` | ペインが作業しているタスク番号（`#` は省略可。CLI は先頭の `#` を無視します） | `439` |
| `agent` | ペインで動いているエージェントの識別子 | `claude` |
| `origin` | ペインの作成方法: `hub`（制御するアプリによる）または `terminal`（人が `misao new` で作成） | `hub` |
| `windowId` | **ハブのウィンドウ番号**。例: `806`。CLI は `W-806` と表示し、`806` / `W-806` を [対象](cli.ja.md#対象の指定) として受け付けます。デーモンのウィンドウ id（`w_...`）とは無関係です。 | `806` |
| `name` | 人が読む表示名 | `misao plan` |

`misao new` で作ったペインには必ず `origin=terminal` が付くため、ハブはまだ登録していないペイン（`windowId` なし）を見つけて、登録を提案できます。

## エージェントプロファイル

[稼働判定](protocol.ja.md#稼働判定) には、エージェント固有の画面ルールを差し込む拡張点があります。プロファイルは見えている画面から `working`、`blocked`、`idle` のいずれかを判定します。`blocked` を得られるのはこの方法だけです。

```ts
interface AgentProfile {
  readonly name: string;                              // pane.state の decidedBy になる
  matches(cmd: readonly string[]): boolean;           // pane.open の cmd で選ばれる。最初に一致したものが優先
  classify(screen: ProfileScreen): 'working' | 'blocked' | 'idle' | null; // null = 意見なし
}
interface ProfileScreen { rows: readonly string[]; title: string; altScreen: boolean }
```

`classify` は画面だけに依存する純粋関数でなければなりません。プロファイルは `DaemonOptions.profiles` を通してデーモンに渡すため、デーモンを自分のプロセスに組み込む場合に使えます。

```ts
import { Daemon } from '@misao/daemon';
import type { AgentProfile } from '@misao/daemon';

const myAgent: AgentProfile = {
  name: 'my-agent',
  matches: (cmd) => cmd[0]?.endsWith('my-agent') === true,
  classify: ({ rows }) => (rows.some((r) => r.includes('Proceed? [y/n]')) ? 'blocked' : null),
};

const daemon = new Daemon({
  socketPath,
  pidPath,
  statePath,          // persistence.json
  profiles: [myAgent],
});
await daemon.start();
```

`misao serve` はプロファイルを受け取りません。汎用のタイトルとバイトのルールで動作します。Claude Code と Codex のプロファイルはまだ同梱されていません。
