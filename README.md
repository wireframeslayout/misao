# misao (Phase 0 spike)

ヘッドレスのターミナルペインサーバー。PTY ホスト + 画面モデル (@xterm/headless) + Unix ソケット API。端末 UI は持たない。

## ビルドと実行

```bash
npm install
npm run build        # tsc -> dist/
npm test             # node:test (tsx)

# デーモン (既定の状態ディレクトリは $MISAO_DIR か ~/workspace/misao/.run)
MISAO_SPIKE_TAG=dev node dist/cli.js serve --dir .run &

node dist/cli.js --dir .run open -- bash
node dist/cli.js --dir .run ls
node dist/cli.js --dir .run send <pane> 'echo hi' --enter
node dist/cli.js --dir .run screen <pane>
node dist/cli.js --dir .run tail <pane> --follow
node dist/cli.js --dir .run attach <pane>      # Ctrl-] d で detach
```

pane は完全な ID か、一意に決まる前方/後方一致で指定できる。停止は起動した PID に SIGTERM (`.run/daemon.pid`)。

## プロトコル

NDJSON / JSON-RPC 2.0 (`src/protocol.ts`)。通知の `params` には stream ごとに単調増加する `seq` と `ts` が入る。
seq の stream は pane ごとの raw 出力、pane ごとの行、デーモン全体のイベントの 3 種。
`server.info` と subscribe 系の結果に載る `epoch` はデーモン起動ごとに変わる (再起動で seq が巻き戻るため、クライアントは epoch 変化で since を捨てる)。

## 構成

- `src/daemon/` デーモン (Pane, Daemon, screen)
- `src/client/MisaoClient.ts` 再利用可能なクライアント
- `src/fixtures/` fake-agent / fake-hub (検証用)
- `deploy/` systemd ユーザー unit のテンプレート (未インストール)
