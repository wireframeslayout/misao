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

## Phase 0 検証スクリプト (`verify/`)

先に `npm run build` を実行する (fixtures / cli は `dist/` を使う)。各スクリプトは専用の `.run/<name>` でデーモンを起動し、終了時に自分の PID だけを停止する。結果は `verify/results/vN.json`。

```bash
node --import tsx verify/v2.ts   # 複数 attach / replay 一致 / サイズ競合 (vim, Claude Code haiku を使用)
node --import tsx verify/v3.ts   # 行ストリームのマーカー検出 (V3_BURST_ROUNDS で --burst の回数を変更可、既定 1000)
node --import tsx verify/v4.ts   # systemd user unit で hub 再起動 / kill -9 (unit は実行中に作成し、終了時に削除)
node --import tsx verify/v5.ts   # hub 停止中の CLI 操作と追いつき / detach キー衝突 (vim, bash, claude, codex)
```

v2 / v5 は実際の `claude` / `codex` を起動する (プロンプトは v2 の claude 1 回のみ)。初回は cwd (このリポジトリ) の信頼ダイアログを自動で通過する。
