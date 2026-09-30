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

## ライセンス

[Apache License 2.0](LICENSE)。コントリビューションには [CLA](CLA.md) への同意が必要です。
