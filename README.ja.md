# misao（操）

**misao** は、AI コーディングエージェント向けのヘッドレスなペインサーバーです。

PTY を持ち、ペインごとに画面モデルを保持し、それらをローカルの Unix ソケット API
（NDJSON JSON-RPC 2.0）で提供します。端末 UI は持ちません。ブラウザ、デスクトップアプリ、
TUI、SSH 端末はすべてクライアントです。

> 状態: **pre-alpha**。Phase 0 の検証スパイクは [`spike/phase-0`](../../tree/spike/phase-0)
> ブランチにあります。Phase 1（デーモン・プロトコル・CLI・Node SDK）を `main` で進めています。

## できること

- 各ペインを実際の PTY（node-pty）で動かし、画面を `@xterm/headless` で保持する
- ペインを 3 つの形で提供する: attach 用の生バイト（途中参加の再生つき）、マーカー検出用の
  ANSI 除去済み行ストリーム、画面スナップショット
- 出力行とイベントに単調増加の `seq` を付け、切断していたクライアントが `since=<seq>` で
  続きから再開でき、取り逃しがあれば `gap` で分かる
- エージェントの稼働状態（working / blocked / idle / exited）をデーモン内で判定する
- 制御側のアプリが落ちていても、SSH 端末から `misao ls / attach / new / kill / send` で操作できる

## パッケージ（予定）

| パッケージ | 役割 |
|---|---|
| `@misao/protocol` | ソケット API の JSON schema と TypeScript 型 |
| `@misao/daemon` | PTY のホスト、画面モデル、リングバッファ、イベント |
| `misao`（cli） | `serve`、`ls`、`attach`、`new`、`kill`、`send`、`screen`、`tail`、`events` |
| `@misao/sdk` | 再接続と `since` 追従を備えた Node クライアント |
| `@misao/bridge`、`@misao/web`、`@misao/profile-*` | 後のフェーズ |

## 開発

Node.js 24 以上が必要です。

```bash
npm ci          # インストール（node-pty をネイティブビルドする）
npm run build   # 全パッケージを tsc -b でビルド
npm run typecheck
npm test        # ワークスペースごとに tsx 経由の node:test（src に対して実行するのでビルド不要）
npm run smoke   # ビルドした CLI を直接実行する
npm run test:e2e  # 実際のデーモンプロセスに対する e2e テスト（後述）
```

e2e テスト（`packages/e2e`）は、一時ディレクトリで `misao serve` を起動し、偽エージェント・偽 hub・
vim・bash で通して確かめます。`npm test` には含まれません。外部ツールが要るケースは環境変数で
有効にします: `MISAO_E2E_CLAUDE=1`（Claude Code）、`MISAO_E2E_CODEX=1`（Codex）、
`MISAO_E2E_SYSTEMD=1`（一時的な systemd ユーザー unit を作る）。`MISAO_E2E_FULL=1` で Phase 0 の
検証と同じ規模に戻します。

systemd でデーモンを動かす手順は [deploy/README.md](deploy/README.md) を参照してください。

開発中は CLI を `node packages/cli/dist/main.js` か `npx misao` で実行します（`npm run build` の後、
bin のリンクを作るために一度だけ `npm rebuild misao` を実行してください）。

## ドキュメント

英語版が正本です。各文書の隣に日本語版（`*.ja.md`）があります。

| 文書 | 内容 |
|---|---|
| [docs/protocol.ja.md](docs/protocol.ja.md) | ソケットプロトコル: メソッド・通知・`seq` / `epoch` / `gap`・エラー |
| [docs/cli.ja.md](docs/cli.ja.md) | `misao` コマンド: 対象指定・コマンド・attach のキー操作・終了コード |
| [docs/config.ja.md](docs/config.ja.md) | `misao.json`・ソケットの場所・メモリの目安・サービスとして動かす |
| [docs/embedding.ja.md](docs/embedding.ja.md) | Node SDK・ラベル規約・シークレット・エージェントプロファイル |

## ライセンス

[Apache License 2.0](LICENSE)。コントリビューションには [CLA](CLA.md) への同意が必要です。

English: [README.md](README.md)
