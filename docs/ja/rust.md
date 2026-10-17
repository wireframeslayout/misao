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
  misao-daemon/         デーモンの本体: ソケット、pid ファイル、ログ、接続、RPC、作業場所 / 窓 / ペイン、PTY、リング、イベント
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

| Node（`packages/`） | Rust（`crates/`） | 状況（第 2 段まで） |
|---|---|---|
| `protocol`（Zod スキーマ、`ErrorCode`、framing） | `misao-protocol` | `server.info`・JSON-RPC・エラー・NDJSON の型。スキーマと照合済み |
| `daemon/src/socket.ts` | `misao-daemon/src/socket.rs` | 完了（dir 700 / socket 600、所有者とシンボリックリンクの確認、古いソケット） |
| `daemon/src/pid-file.ts` | `misao-daemon/src/pid_file.rs` | 完了 |
| `daemon/src/daemon.ts`（`start` / `shutdown`） | `misao-daemon/src/daemon.rs` | 完了。起動の lock を終了まで保持する（下記） |
| `daemon/src/daemon.ts`（ディスパッチ）、`rpc-error.ts`、`params.ts` | `core.rs`、`rpc.rs`、`rpc_error.rs`、`params.rs` | 完了（`pane.screen` / `pane.attach` / `pane.detach` / `pane.send_keys` / `window.focus` / `pane.respawn` は `NotImplemented`） |
| `daemon/src/connection.ts`、`stream.ts` | `connection.rs`、`stream.rs` | 完了（送信キューの高水位、pull 型の購読、追い越された購読者の切断） |
| `daemon/src/layout.ts`、`layout-handlers.ts` | `layout.rs`、`layout_handlers.rs` | 完了（保存は第 4 段） |
| `daemon/src/pane-registry.ts`、`pane-lifecycle.ts`、`pane-io.ts` | `registry.rs`、`pane_handlers.rs` | 完了（attach・screen・保存を除く） |
| `daemon/src/pane.ts` | `pane.rs`、`pty.rs` | PTY（portable-pty）、入力、resize、終了検知、close。画面・タイトル・稼働判定は第 3 段 |
| `daemon/src/size-arbiter.ts`、`ring.ts`、`ansi.ts`、`child-env.ts`、`event-log.ts`、`limits.ts`、`ulid.ts` | `size_arbiter.rs`、`ring.rs`、`ansi.rs`、`child_env.rs`、`event_log.rs`、`limits.rs`、`ids.rs` | 完了 |
| `daemon/src/log.ts` | `misao-daemon/src/log.rs` | 完了（stderr に `[misao <ISO 時刻>] msg`） |
| `sdk-node/src/socket-path.ts` | `misao/src/socket_path.rs` | 完了（優先順位も 107 バイト制限も同じ） |
| `cli/src/config/*` | `misao/src/config.rs` | `socket` / `logLevel` / `scrollback` / `rings` を使用。`keys` は型の検証のみ |
| `cli` の `serve`、`--version` | `misao/src/{cli,serve}.rs` | 完了。他のコマンドは後続の段 |
| 画面（`screen.ts`）・稼働判定（`state-tracker.ts` ほか）・永続化・attach | - | 未着手（第 3〜4 段） |

第 1 段で意図的に違う点: `--help` は clap の生成（説明は日本語、フラグ名は英語）。`--version` は `misao <Cargo のバージョン>` で、
リリーススクリプトが設定するようになる（第 6 段）まで `0.0.0`。Windows では接続口が後続の段のため、
`misao serve` はエラーで終わる（`docs/*/windows.md` を参照）。

## 第 2 段の振る舞い（Node 版と違う点）

エラーコードと番号、序数の振り直し、`ephemeralEnv`（保存しない）、子の env の継承（デーモンの env から `TMUX*` / `STY` / `ZELLIJ*`
を除き、`MISAO_SOCKET` / `MISAO_PANE_ID` / `TERM` / `COLORTERM` を足す）、リングの既定値、gap / epoch / since の意味、
1 接続 1 events ストリーム、`daemon.started` が 1 件目、は Node 版と同じです。違うのは次の点です。

- **起動に失敗した pane は作らない**: コマンドが無い・`cwd` がディレクトリでないとき、Node 版（node-pty）は pane を作って
  終了コード 1 で終わらせますが、Rust 版は `pane.open` が `InvalidParams`（`spawn failed: ...`）で失敗し、作業場所も pane も残しません
  （Node 版も `Pane` の生成が例外を投げれば同じ扱いにする設計）。
- **動いている行の購読者を待つ**: Rust 版は出力の処理が速く、読んではいるが子より遅い購読者（SDK など）が、4 MiB のリングに追い越されて
  切断されます。購読者の遅れがリング容量の半分を超えている間は、pty を読まずに子の書き込みを詰まらせます。1 秒以上カーソルが進まない
  購読者は待たず、従来どおりリングに追い越されたら切断して gap で知らせます。閉じている途中と子の終了後は待ちません。
- **`pane.close` は子を残さない**: Unix ではプロセスグループへ SIGHUP → 3 秒 → SIGKILL を送ります（Node 版と同じ）。さらに、
  リーダーが SIGHUP で終わっても、グループに残ったプロセス（SIGHUP を無視する子など）がいれば、猶予の後に SIGKILL してから戻ります。
  Windows は `taskkill /T /F` でツリーごと止めます（Job Object による封じ込めは第 5 段）。
- **`pane.write` の `dataB64`**: 不正な base64 は `InvalidParams` にします（Node の `Buffer.from` は黙って読み捨てる）。
- **サイズ**: cols / rows の上限は設けません（Zod と同じ）。pty へは 65535 に丸めて渡します。
- **未対応（後続の段）**: `pane.info` の `fgCommand`、稼働判定（`agentState` は走行中は `unknown`、終了後は `exited` / `exit` だけ）、
  タイトル、状態の保存（再起動後の `stopped` の復元）、`preplace`（Node 版も未実装）。
- 子に `SHELL` が付く場合がある（portable-pty が `SHELL` を補う）。

## プロトコル適合

`node scripts/export-protocol-schema.mjs`（事前に `npm run build`）が、`@misao/protocol` の
`buildProtocolJsonSchema()` を `crates/misao-protocol/schema/protocol.json` に書き出します。
ファイルはコミットしてあるので、`cargo test` に Node は要りません。

- `packages/protocol/test/rust-schema-sync.test.ts` は、コミット済みのファイルが古いと失敗します（`npm test` で動く）。
  Zod のスキーマを変えたら、再生成してコミットします。
- `crates/misao-protocol/tests/schema_conformance.rs` は、プロトコルバージョン、メソッド名と通知名、エラーコード、
  `server.info` の項目（名前と必須かどうか）、Rust の型が作る JSON-RPC 応答が、書き出したスキーマを満たすことを確かめます。
- Rust 版デーモンの `server.schema` は、埋め込んだ `schema/protocol.json` を返します。

## 起動の排他（Node 版との違い）

Rust 版デーモンは、古いソケットの判定から終了時のソケット削除まで、ソケットの隣の `<socket>.lock`（`--data` に依存しない）を
`flock` で排他します。同時に起動した 2 つが互いのソケットを消すことも、終了中の旧デーモンが、後から（`--data` が違うために
pid ファイルでは排他できない形で）起動した新デーモンのソケットを消すこともありません。lock は `Daemon` が保持し、
ソケットと pid ファイルを消した後に解放します。稼働中のデーモンがいる間は他の起動が lock を取れないので、待ち受けていれば
「another daemon is already listening」、そうでなければ（起動中・終了中）「another daemon is starting」で失敗します。
`bind` に失敗してもパスは消さず、自分の `bind` の後に `chmod` が失敗したソケットだけを消します。lock ファイルは意図的に残します。
Node 版にはこの lock が無く、pid ファイルの場所が違う 2 つのデーモンを同時に起動すると、判定・削除・listen の間で競合する
可能性があります。Node 版は直さず（置き換え予定のため）、ここに記録するだけにします。

## Rust 版デーモンで e2e を動かす

`packages/e2e/test/helpers/daemon-process.ts` は `misao serve --config ... --socket ... --data ...` でデーモンを起動します。
`MISAO_DAEMON_BIN` を指定すると、Node の CLI の代わりにその実行ファイルを使います。

```bash
cargo build --release -p misao
MISAO_DAEMON_BIN=$PWD/target/release/misao npm run test:e2e -w @misao/e2e   # 全体
MISAO_DAEMON_BIN=$PWD/target/release/misao npm run test:e2e:rust -w @misao/e2e   # 通る範囲（CI）
```

変数を指定しなければ何も変わりません。大量出力のテスト（`v3-line-markers`）は処理速度に左右されるので、`release` ビルドで動かしてください
（debug ビルドでも通りますが、通常出力のテストに約 70 秒かかります）。第 2 段の状況:

| テストファイル | Rust 版 | 理由 |
|---|---|---|
| `server-info.test.ts` | 通る | |
| `core-rpc.test.ts`（第 2 段で追加。Node 版でも通る） | 通る | 作業場所 / 窓、ペインの起動・env・入力・resize・close、行の記録、イベントと再接続 |
| `v3-line-markers.test.ts` | 通る | 行ストリームのマーカー、遅い購読者、再購読 |
| `v2-attach-replay`、`v2-size-conflict` | 通らない | `pane.attach`（第 4 段）と `pane.screen`（第 3 段）が `not implemented` |
| `v4-hub-restart` | 通らない | `pane.screen`（第 3 段）が `not implemented`。hub の再開そのもの（SDK の自動再接続と購読の復元）は `core-rpc` と `v3` で確かめている |
| `v5-catch-up` | 通らない | CLI の attach（第 4 段）が必要 |
| `v5-key-collisions` | 通らない | `pane.screen`（第 3 段）が `not implemented` |
| `v2-claude`、`v4-systemd` | オプトイン。スキップ | |

## 移行計画

issue で管理しています。#39（workspace・プロトコル適合・最小デーモン・CI）、#40 中核（RPC・作業場所/窓/ペイン・PTY・出力の記録・イベント。この段）、
#41 画面と稼働判定、#42 状態の保存・attach・CLI 全体（既存 e2e を全部通す）、#43 macOS と Windows、#44 バイナリ配布と AZITO の切り替え。
背景は #32（部品の検討）と #37（Windows）。
