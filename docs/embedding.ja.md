# 組み込み

[English](embedding.md) | 日本語

英語版（[embedding.md](embedding.md)）が正本です。食い違いがあれば英語版に従ってください。

SDK（`@misao/sdk`）を使って自分の Node.js プログラムから misao を操作する方法と、組み込むアプリが従うべき規約（ラベル、シークレット、エージェントプロファイル）を説明します。ワイヤ上の意味（`seq`、`epoch`、`gap`、pull 型の購読、稼働判定）は [プロトコル](protocol.ja.md) に一箇所だけ定義しており、ここからはリンクするだけにしています。

関連文書: [プロトコル](protocol.ja.md)、[設定](config.ja.md)、[CLI](cli.ja.md)。

パッケージは ESM で、Node.js 24 以降が必要です（リポジトリのルートの `package.json` の `engines` で宣言しています）。

## インストール

`@misao/sdk` と `@misao/protocol` は npm には公開していません。リリースごとに、パッケージ別の tarball を [GitHub Releases](https://github.com/wireframeslayout/misao/releases) に添付しており、利用側は tarball の URL に依存します。SDK は `@misao/protocol` の型を参照するため、両方を書きます。利用側の `package.json` では次のようにします（`0.1.0` は使いたいリリースに置き換えます。タグとファイル名の両方です）。

```json
{
  "dependencies": {
    "@misao/sdk": "https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-sdk-0.1.0.tgz",
    "@misao/protocol": "https://github.com/wireframeslayout/misao/releases/download/v0.1.0/misao-protocol-0.1.0.tgz"
  }
}
```

- `@misao/sdk` の tarball は、`@misao/protocol` を同じリリースの protocol の tarball に向けています。SDK だけを書いてもインストールできます。両方を書くと 2 つの version が揃っていることが見え、まとめて更新できます。
- tarball に含まれるのはコンパイル済み JavaScript（`dist`）と型定義だけです。型は Node.js の型を参照するため、TypeScript のプロジェクトでは `@types/node` を devDependencies に入れてください。
- ハイフンを含むタグ（例: `v0.2.0-rc.1`）は pre-release です。
- 公開済みのタグの tarball は差し替えません。修正は新しいタグで出します。
- 同じ tarball を手元で作るには、`npm run clean && npm run build` のあと `node scripts/set-version.mjs <version>` と `npm run release:pack -- --tag v<version>`（出力は `release/`）を実行し、`node scripts/verify-release.mjs --tag v<version>` で確かめます。verify は SDK の tarball を一時プロジェクトにインストールし、実行時の import と型の解決を検証します。終わったら `git checkout -- package.json package-lock.json packages/*/package.json` で version の変更を戻してください。

## デーモンと CLI の同梱

リリースには `misao-<version>.mjs` も添付します。CLI とデーモンを 1 つの ESM ファイルにまとめたもので、misao を自分で同梱するアプリ（例: AZITO）向けです。同じリリースの `SHA256SUMS` に、すべてのアセット（2 つの tarball とこのバンドル）の sha256 が載っています。`sha256sum -c --ignore-missing SHA256SUMS` で確かめられます。

```bash
node misao-0.1.0.mjs --version
node misao-0.1.0.mjs serve
```

- `misao-<version>.LICENSES.txt`（`SHA256SUMS` にも載ります）には、misao 本体（Apache-2.0、`NOTICE` を含む）と、バンドルに入っているサードパーティパッケージ（`zod`、`@xterm/headless`。どちらも MIT）のライセンス全文が入っています。バンドルと一緒に配布してください。バンドルの先頭の行もこのファイルを指しています。`@xterm/headless` は公開物にライセンスファイルを含めていないため、宣言されているライセンスとリポジトリだけを記しています。
- ネイティブモジュールの `node-pty` はバンドルに**含まれません**。通常の Node.js の探索で解決されるので、ファイルの隣（または親ディレクトリ）の `node_modules/node-pty` に置いてください。すでに `node-pty ^1.1.0` を同梱しているアプリはそれを共有できます。misao 自身はプリビルドを配りません。`@xterm/headless` と `zod` はバンドルの中に入っています。
- バンドルは ESM で、Node.js 24 以上が必要です。先頭は `#!/usr/bin/env node` の shebang なので、実行権限を付けて直接実行することもできます。
- CLI とデーモンは同じファイルなので、`--version` と `server.info` の `version` は、そのファイルに埋め込まれたリリースの version（`misao 0.1.0` と `"version": "0.1.0"`）です。`protocolVersion` と違い、同じプロトコルを話す別ビルドのデーモンを見分けられます。`version` を返さない古いデーモンは、その欠落で区別できます。デーモンをサービスとして動かす手順は [deploy/README.md](../deploy/README.md)（Linux は systemd ユニット、macOS は launchd の plist）を参照してください。
- 互換ルール: バンドルのデーモンは `server.info` で `PROTOCOL_VERSION` を報告し、クライアント（`@misao/sdk`）は**メジャー**バージョンが等しければ互換です（[プロトコル](protocol.ja.md#バージョンと互換性)）。SDK とバンドルは同じリリースのものを使うか、少なくともプロトコルのメジャーを揃えてください。マイナーの差は許容されます（受信側は未知のフィールドと enum 値を許容します）。
- 手元で作るには `node scripts/bundle-cli.mjs --out dist-release` のあと、`node scripts/smoke-bundle.mjs dist-release/misao-<version>.mjs` を実行します。スモークテストは、バンドルと `node-pty` を一時ディレクトリに置き、`--version` と `server.info` の `version` がファイル名の version と一致すること、一時ソケットでの `serve`、pane の作成・出力確認・kill（外部の `node-pty` で pty が作れること）、正常な停止を確かめます。稼働中のデーモンには触れません。

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
- `MisaoClientOptions`: `socketPath` と `connect` のどちらか一方（[接続方法の差し込み](#接続方法の差し込みconnect) を参照）、`backoff`（部分指定可、下記参照）、`connectTimeoutMs`（既定値 `5000`）。どちらも指定しない、または両方指定した場合は、コンストラクターが `TypeError` を投げます。
- `connect()` は接続し、`server.info` を呼び、プロトコルのメジャーバージョンが一致することを確認し、ストリームを復元します。デーモンに到達できない場合（`MisaoConnectionError`）や互換性がない場合（`MisaoProtocolVersionError`、[バージョン](protocol.ja.md#バージョンと互換性) を参照）は reject されます。呼び出せるのはクライアントがアイドルのときだけで、最初の試行の前か、失敗した試行の後です。
- `connectTimeoutMs` は、接続確立後のセットアップ（`server.info` の確認とストリームの復元）に時間制限を設けます。タイムアウトすると接続を閉じ、`connect()`（または現在の再接続の試行）は `MisaoConnectionError` で失敗します。これによりデーモンのハングから保護されます。カスタム `connect` では、`connect` 関数の待機にも別枠で同じ時間制限が掛かります（[接続方法の差し込み](#接続方法の差し込みconnect) を参照）。`socketPath` ではセットアップだけが対象です。
- `request(method, params)` は `@misao/protocol` によって型付けされています。デーモンのエラーは `MisaoRpcError`（`code`、`message`。[エラー](protocol.ja.md#エラー) を参照）で reject されます。未接続の間に呼び出すと `MisaoConnectionError` で reject されます。`params` は送信前にスキーマで検証され、不正な場合は Zod の `ZodError` で reject されます（何も送信されません）。
- `close()` はクライアントを終了し、再接続を止めます。

## 接続方法の差し込み（`connect`）

Unix ソケット以外（たとえば別プロセスが中継する WebSocket）でデーモンに接続するには、`socketPath` の代わりに `connect` を渡します。接続済みの Node.js `Duplex` を返す関数です。プロトコルは改行区切りの JSON-RPC なので、トランスポートは双方向でバイト列をそのまま運べば足ります。

```ts
import WebSocket, { createWebSocketStream } from 'ws';   // 例。WebSocket ライブラリは何でも構いません
import { MisaoClient } from '@misao/sdk';

const client = new MisaoClient({
  connect: async ({ signal }) => {
    const ws = new WebSocket('wss://hub.example/misao-relay', {
      headers: { authorization: `Bearer ${token}` },
      handshakeTimeout: 5000,   // upgrade の待ちは自分でも制限する
      signal,                   // タイムアウトや close() で abort される。ws は接続途中のソケットを閉じる
    });
    await new Promise<void>((resolve, reject) => {
      ws.once('open', resolve);
      ws.once('error', reject);
    });
    return createWebSocketStream(ws);                    // 生のバイト列を運ぶ Duplex
  },
});
await client.connect();
```

- `connect` は `{ signal: AbortSignal }` を受け取って呼ばれます（引数を使わない関数でも構いません）。各試行は `connectTimeoutMs` で打ち切られます。期限を過ぎるか、試行中に `close()` すると、SDK は `signal` を abort し、その試行は `MisaoConnectionError` で失敗し（再接続中なら通常のバックオフで再試行）、あとから解決した `Duplex` は破棄されます。実際の接続試行が取り消されるよう、`signal` を尊重し、上のように独自のハンドシェイクタイムアウトも設定してください。タイムアウトは `connect` の呼び出しと、その後のセットアップにそれぞれ適用されます。
- `connect` は最初の `connect()` と、**再接続の試行ごと**に呼ばれます。毎回新しい `Duplex` を返してください。reject は失敗した試行として扱われ（元のエラーを `cause` に持つ `MisaoConnectionError`）、通常のバックオフが適用されます。
- それ以外は `socketPath` のときと同じ動作です。`server.info` の確認、`connectTimeoutMs`、再接続、ストリームの復元、`onGap`、`onStateChange` が同様に働きます。
- Promise が解決した時点で接続済みであること、接続が切れたら `close` を発火することが必要です（相手が end した場合は SDK 側でも破棄します）。SDK はこのストリームへ書き込み、不要になったら破棄します。
- `Error` でない値で reject した場合は `String(reason)` をメッセージに使います。
- `MisaoClientOptions` は interface から型エイリアス（`socketPath` 形式と `connect` 形式のユニオン）になったため、`extends` できません。交差型（`& { ... }`）を使ってください。2 つの形式は `MisaoClientTarget` として export されます。
- `@misao/sdk` は WebSocket ライブラリに依存しません。上の `ws` の import は例です。

## 再接続とバックオフ

`connect()` が一度成功した後は、接続が切れても `close()` を呼ぶまで再試行します。

| オプション（`backoff`） | 既定値 | 意味 |
|---|---|---|
| `initialDelayMs` | `100` | 最初の再試行までの遅延 |
| `maxDelayMs` | `5000` | 遅延の上限 |
| `factor` | `2` | 試行ごとの倍率 |

試行 `n`（1 から始まる）の前の遅延は `min(maxDelayMs, initialDelayMs * factor^(n-1))`（`computeBackoffDelay`）です。試行回数の上限はありません。クライアントが諦めるのは、`close()` が呼ばれた場合と、再起動後のデーモンがプロトコル非互換だと分かった場合だけで、どちらの場合も状態は `closed` になり、`cause` が付くのはプロトコル非互換のときだけです（再接続処理での想定外の内部エラーも `onError` に報告したうえで `closed` になります）。切断時に処理中だったリクエストは `MisaoConnectionError` で reject されます。

```ts
client.onStateChange((state) => {
  // { status: 'connected' }
  // { status: 'reconnecting', attempt, delayMs, cause }
  // { status: 'closed', cause? }
});
```

`connected` は、最初の `connect()` の終わり（Promise が解決する前）にも一度発行され、再接続が成功するたびにも発行されます。最初の通知を受け取るには、`connect()` を呼ぶ前にリスナーを登録してください。

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
- 購読は 1 ストリーム（events、または 1 つのペインの lines）につき 1 つしか作れません。2 つ目は素の `Error`（`stream already registered: <key>`）で reject されます。
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
