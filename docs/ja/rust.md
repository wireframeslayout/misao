# Rust 版

[English](../en/rust.md) | 日本語

misao のデーモンと CLI を Rust の単一バイナリ `misao` に書き直しています。配布は GitHub Releases です。
SDK（`@misao/sdk`）と protocol（`@misao/protocol`、`PROTOCOL_VERSION` 0.3.0）は TypeScript のまま維持し、プロトコルは変えません。
Rust 版が既存の e2e（`packages/e2e`）を全部通すまでは、Node 版が正式版です。

関連文書: [プロトコル](../protocol.ja.md)、[CLI](../cli.ja.md)、[設定](../config.ja.md)。

## 構成

```text
Cargo.toml              workspace（resolver 3、edition 2024、MSRV 1.88、共通 lint）
rustfmt.toml  clippy.toml  deny.toml
crates/
  misao-protocol/       ワイヤの型: JSON-RPC、ErrorCode、ServerInfo、NDJSON の行分割
    schema/protocol.json  Zod から書き出した JSON Schema（生成物。コミットする）
    tests/schema_conformance.rs
  misao-daemon/         デーモンの本体: ソケット、pid ファイル、ログ、accept ループ、ディスパッチ
  misao/                バイナリ: clap の CLI（`serve`、`--version`）、設定、ソケットパス
```

依存は一方向です: `misao` -> `misao-daemon` -> `misao-protocol`。

## ビルドとテスト

```bash
cargo build --workspace            # target/debug/misao
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check                   # ライセンス・脆弱性・取得元（deny.toml）
```

方針: stable の Rust、MSRV は `1.88`（`workspace.package.rust-version`。CI の `Rust (MSRV)` で確認）、
`unsafe_code = "forbid"`、clippy の `all` を deny、テスト以外では `unwrap` / `expect` を deny。
`Cargo.lock` はコミットし、CI は `--locked` で動かします。

手元で Rust 版のデーモンを動かす:

```bash
target/debug/misao serve --socket /tmp/misao-try/misao.sock --data /tmp/misao-try
```

## Node 版との対応

| Node（`packages/`） | Rust（`crates/`） | 第 1 段の状況 |
|---|---|---|
| `protocol`（Zod スキーマ、`ErrorCode`、framing） | `misao-protocol` | `server.info`・JSON-RPC・エラー・NDJSON の型。スキーマと照合済み |
| `daemon/src/socket.ts` | `misao-daemon/src/socket.rs` | 完了（dir 700 / socket 600、所有者とシンボリックリンクの確認、古いソケット） |
| `daemon/src/pid-file.ts` | `misao-daemon/src/pid_file.rs` | 完了 |
| `daemon/src/daemon.ts`（`start` / `shutdown` / ディスパッチ） | `misao-daemon/src/daemon.rs` | `server.info` と `server.schema` のみ。他の既知メソッドは `NotImplemented` |
| `daemon/src/log.ts` | `misao-daemon/src/log.rs` | 完了（stderr に `[misao <ISO 時刻>] msg`） |
| `sdk-node/src/socket-path.ts` | `misao/src/socket_path.rs` | 完了（優先順位も 107 バイト制限も同じ） |
| `cli/src/config/*` | `misao/src/config.rs` | `socket` と `logLevel` を使用。`keys` / `scrollback` / `rings` は型の検証のみ |
| `cli` の `serve`、`--version` | `misao/src/{cli,serve}.rs` | 完了。他のコマンドは後続の段 |
| ペイン・画面・稼働判定・永続化 | - | 未着手（第 2〜4 段） |

第 1 段で意図的に違う点: `--help` は clap の生成（説明は日本語、フラグ名は英語）。`--version` は `misao <Cargo のバージョン>` で、
リリーススクリプトが設定するようになる（第 6 段）まで `0.0.0`。Windows では接続口が後続の段のため、
`misao serve` はエラーで終わる（`docs/*/windows.md` を参照）。

## プロトコル適合

`node scripts/export-protocol-schema.mjs`（事前に `npm run build`）が、`@misao/protocol` の
`buildProtocolJsonSchema()` を `crates/misao-protocol/schema/protocol.json` に書き出します。
ファイルはコミットしてあるので、`cargo test` に Node は要りません。

- `packages/protocol/test/rust-schema-sync.test.ts` は、コミット済みのファイルが古いと失敗します（`npm test` で動く）。
  Zod のスキーマを変えたら、再生成してコミットします。
- `crates/misao-protocol/tests/schema_conformance.rs` は、プロトコルバージョン、メソッド名と通知名、エラーコード、
  `server.info` の項目（名前と必須かどうか）、Rust の型が作る JSON-RPC 応答が、書き出したスキーマを満たすことを確かめます。
- Rust 版デーモンの `server.schema` は、埋め込んだ `schema/protocol.json` を返します。

## Rust 版デーモンで e2e を動かす

`packages/e2e/test/helpers/daemon-process.ts` は `misao serve --config ... --socket ... --data ...` でデーモンを起動します。
`MISAO_DAEMON_BIN` を指定すると、Node の CLI の代わりにその実行ファイルを使います。

```bash
cargo build -p misao
MISAO_DAEMON_BIN=$PWD/target/debug/misao npm run test:e2e -w @misao/e2e   # 全体
npm run test:e2e:rust -w @misao/e2e                                       # 通る範囲（CI）
```

変数を指定しなければ何も変わりません。第 1 段の状況:

| テストファイル | Rust 版 |
|---|---|
| `server-info.test.ts` | 通る |
| `v2-attach-replay`、`v2-size-conflict`、`v3-line-markers`、`v4-hub-restart`、`v5-catch-up`、`v5-key-collisions` | `not implemented: pane.open` で失敗（ペインは第 2 段） |
| `v2-claude`、`v4-systemd` | オプトイン。スキップ |

## 移行計画

issue で管理しています。#39（この段: workspace・プロトコル適合・最小デーモン・CI）、#40 中核（RPC・作業場所/窓/ペイン・PTY・出力の記録・イベント）、
#41 画面と稼働判定、#42 状態の保存・attach・CLI 全体（既存 e2e を全部通す）、#43 macOS と Windows、#44 バイナリ配布と AZITO の切り替え。
背景は #32（部品の検討）と #37（Windows）。
