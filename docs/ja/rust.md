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
  misao-daemon/         デーモンの本体: ソケット、pid ファイル、ログ、接続、RPC、作業場所 / 窓 / ペイン、PTY、リング、イベント、
                        画面（端末の解釈）、稼働判定（state-tracker・profile）、前面プロセス
    tests/fixtures/node-diff/  Node 版との差分テストの入力と期待値（`scripts/gen-rust-diff-fixtures.mjs` の生成物）
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

| Node（`packages/`） | Rust（`crates/`） | 状況（第 3 段まで） |
|---|---|---|
| `protocol`（Zod スキーマ、`ErrorCode`、framing） | `misao-protocol` | `server.info`・JSON-RPC・エラー・NDJSON の型。スキーマと照合済み |
| `daemon/src/socket.ts` | `misao-daemon/src/socket.rs` | 完了（dir 700 / socket 600、所有者とシンボリックリンクの確認、古いソケット） |
| `daemon/src/pid-file.ts` | `misao-daemon/src/pid_file.rs` | 完了 |
| `daemon/src/daemon.ts`（`start` / `shutdown`） | `misao-daemon/src/daemon.rs` | 完了。起動の lock を終了まで保持する（下記） |
| `daemon/src/daemon.ts`（ディスパッチ）、`rpc-error.ts`、`params.ts` | `core.rs`、`rpc.rs`、`rpc_error.rs`、`params.rs` | 完了（`pane.attach` / `pane.detach` / `pane.send_keys` / `window.focus` / `pane.respawn` は `NotImplemented`） |
| `daemon/src/connection.ts`、`stream.ts` | `connection.rs`、`stream.rs` | 完了（送信キューの高水位、pull 型の購読、追い越された購読者の切断） |
| `daemon/src/layout.ts`、`layout-handlers.ts` | `layout.rs`、`layout_handlers.rs` | 完了（保存は第 4 段） |
| `daemon/src/pane-registry.ts`、`pane-lifecycle.ts`、`pane-io.ts` | `registry.rs`、`pane_handlers.rs` | 完了（attach・保存を除く） |
| `daemon/src/pane.ts` | `pane.rs`、`pty.rs`、`pane_view.rs`、`pane_observer.rs` | PTY（portable-pty）、入力、resize、終了検知、close、画面・タイトル・稼働判定との接続（`pane_view` が部品をつなぎ、`pane_observer` が lock・判定のスレッド・イベントを持つ） |
| `@xterm/headless`、`daemon/src/screen.ts`（`viewportText`） | `screen.rs`、`screen_{emulator,perform,cursor,edit,buffer,line,reflow,charset}.rs`、`unicode_width{,_table}.rs` | 完了（`serializeSnapshot` は attach と一緒に第 4 段）。vte のパーサー + xterm.js と同じ意味の自前の画面（後述） |
| `daemon/src/state-tracker.ts`、`byte-activity.ts`、`title-activity.ts` | `state_tracker.rs`、`byte_activity.rs`、`title_activity.rs` | 完了。閾値・窓・猶予は Node 版と同じ。タイマーは持たず、時刻は引数（後述） |
| `daemon/src/profile.ts`（差し込み口だけ。規則は Node 版に無い） | `profile.rs`、`profiles/{claude,codex}.json` | 完了。規則をデータとして持ち、AZITO の画面規則から Claude / Codex を移植（後述） |
| `daemon/src/foreground.ts` | `foreground.rs` | 完了（Linux は `/proc`、macOS は `ps`、Windows は未対応）|
| `daemon/src/size-arbiter.ts`、`ring.ts`、`ansi.ts`、`child-env.ts`、`event-log.ts`、`limits.ts`、`ulid.ts` | `size_arbiter.rs`、`ring.rs`、`ansi.rs`、`child_env.rs`、`event_log.rs`、`limits.rs`、`ids.rs` | 完了 |
| `daemon/src/log.ts` | `misao-daemon/src/log.rs` | 完了（stderr に `[misao <ISO 時刻>] msg`） |
| `sdk-node/src/socket-path.ts` | `misao/src/socket_path.rs` | 完了（優先順位も 107 バイト制限も同じ） |
| `cli/src/config/*` | `misao/src/config.rs` | `socket` / `logLevel` / `scrollback` / `rings` を使用。`keys` は型の検証のみ |
| `cli` の `serve`、`--version` | `misao/src/{cli,serve}.rs` | 完了。他のコマンドは後続の段 |
| 永続化・attach・`serializeSnapshot` | - | 未着手（第 4 段） |

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
- **入力の待ち行列に上限がある**: `pane.write` の入力は専用スレッドで pty へ書くため、子が読まないと溜まります。Rust 版は溜まったバイト数が 4 MiB を超えると、待たずに `-32603`（`pane input queue is full ...`）で断ります（待ち行列が空のときの 1 回の入力は、これより大きくても受けます）。Node 版（node-pty）は無制限に溜め、対応するエラーはありません。
- **`pane.close` は子を残さない**: Unix ではプロセスグループへ SIGHUP → 3 秒 → SIGKILL を送ります（Node 版と同じ）。さらに、
  リーダーが SIGHUP で終わっても、グループに残ったプロセス（SIGHUP を無視する子など）がいれば、猶予の後に SIGKILL してから戻ります。
  終了済みの pane でも、グループに残ったプロセスがいれば（`pane.close` とデーモンの終了で）同じ手順で止めます。
  **pgid の再利用対策**: Linux ではリーダーの終了を `waitid(WNOWAIT)` で「回収せずに」検知し、終了コードとシグナルをそこで記録します。
  リーダーはゾンビのまま残り、`pane.close` / 終了でグループを片付けた後に初めて回収するので、その pid（= pgid）は close まで再利用されず、
  close の時点で `/proc` を走査してグループの生存（ゾンビのリーダーは数えない）をその場で判定できます。終了済みの pane は close まで
  ゾンビを 1 つ残します。他の Unix（macOS）は `waitid` の `WNOWAIT` を使えないため終了時に回収し、終了時にグループに残りがいた pane
  だけを close で扱います（残りが後で終わって pgid が再利用された場合の誤送信は、この環境では防げません）。
  Windows は `taskkill /T /F` でツリーごと止めます（`spawn_blocking` で実行。SIGHUP 相当の猶予は無く、最初から強制終了で、終了済みの pane の残りは追いません。Job Object による封じ込めは第 5 段）。
- **`pane.write` の `dataB64`**: 不正な base64 は `InvalidParams` にします（Node の `Buffer.from` は黙って読み捨てる）。
- **サイズ**: cols / rows は Zod と同じ範囲（安全な整数）をそのまま保持し、`pane.info` とイベントには元の値を返します。pty へ渡すときだけ 65535 に丸めます。
- **未対応（後続の段）**: 状態の保存（再起動後の `stopped` の復元）、`preplace`（Node 版も未実装）。
  画面・タイトル・稼働判定・`fgCommand` は第 3 段で入った（次の節）。
- 子に `SHELL` が付く場合がある（portable-pty が `SHELL` を補う）。

## 第 3 段の振る舞い（画面と稼働判定）

`pane.screen`、`pane.info` の `title` / `agentState` / `decidedBy` / `fgCommand`、イベント `pane.title` / `pane.state` が入りました。
画面の文字・カーソル・タイトル、稼働判定の遷移と時刻は、記録した出力で Node 版と突き合わせて一致を確かめています（次の節）。

### 画面モデル

端末の解釈（`@xterm/headless` 相当）は、`vte` のパーサーの上に、xterm.js 6 の `InputHandler` / `Buffer` と同じ意味で作った画面です
（カーソル移動、消去、行・文字の挿入と削除、スクロール領域、代替画面、タブ、文字集合、全角・結合文字、折り返しの再配置を含む）。
色・属性、マウス、問い合わせへの応答は扱いません（Node 版も xterm の応答を子へ返しません）。

`alacritty_terminal` を先に試しましたが採用しませんでした。ランダムな制御列で xterm.js と突き合わせると、`IL` / `DL` の後のカーソル桁、
「空のセル」と「空白が書かれたセル」の区別（行末の空白を残すか）、行末で折り返しを待つ間のカーソル桁、文字幅の表（xterm.js の既定は
Unicode 6 で、絵文字は 1 桁）が違い、`Handler` を包んでも直せない内部状態の差でした。文字幅の表は
`scripts/gen-unicode-width.mjs` が `@xterm/headless` から書き出します（`unicode_width_table.rs`。`npm test` が古さを検出）。

Node 版（xterm.js）と違う点:

- **大きさの上限**: プロトコルは安全な整数まで受けますが、画面モデルは 1024 x 1024 に丸めます（セルごとにメモリを使うため）。`pane.info` とイベントには元の値を返します。下限は xterm.js と同じ 2 桁 x 1 行です。
- **扱わない列**: `CSI Ps SP @` / `A`（横スクロール）、`CSI Ps ' }` / `~`（桁の挿入・削除）、`CSI t`（既定で無効）、DCS、色・属性・マウス・ハイパーリンクの列。画面の文字には効かないか、xterm.js でも既定で無効です。8 ビットの C1 制御文字（U+0080-009F）は制御として扱わず、文字として扱います。OSC のパラメータは 16 個までです（vte の上限）。
- **再現しない xterm.js の内部状態の食い違い**: `ED 1` を最終行の行末で受けると xterm.js は例外を投げます（Rust 版は何もしません）。空の代替画面を `resize` してから使うと、xterm.js は行リストと `ybase` が食い違った状態になり、画面の外の古い行を読みます。スクロールバックが満杯のとき、狭くする再配置が行リストの上限を超えると、結果が行リストの内部の回転位置に依存します（Rust 版は古い行から捨てます）。いずれも、差分テストの乱数入力の 0.1% 程度で見つかる、小さなスクロールバックと幅 5 桁前後の端の動きです。
- **結合文字と REP の上限**: 1 つのセルに付けられる結合文字は 32 個までです（超えたぶんは捨てる。xterm.js は無制限ですが、長い結合列で行の更新が遅くなるのを防ぎます）。`CSI Ps b`（REP）の繰り返しは、画面のセル数までです（それ以上は同じ画面になります）。
- **OSC の長さの上限**: `vte` は `std` 機能のとき OSC の中身を上限なしに貯めます。`std` を外さず、パーサーに渡す前に OSC の長さを数えます（`screen_osc.rs`）。1 MiB に達したら CAN を差し込んで中止し、終端までの残りを捨てます。中止された OSC は何も起こしません（xterm.js が上限を超えた OSC を捨てるのと同じ）。切り詰めたタイトルにはなりません。xterm.js の上限は 10 MB ですが、実際の OSC（タイトルなど）は数 KB です。
- **行リストの食い違いの補正**: 通常と違う操作の組み合わせで行リストが見えている行に足りなくなったとき、空行で補います（上の「再現しない xterm.js の内部状態の食い違い」の後始末。画面の操作が常に成り立つようにするため）。
- 画面の行は、xterm.js と同じく「明示的に書かれた空白は残し、消去されたセルは行末から除く」です。

### 稼働判定

exit > profile > title > bytes の順は同じです。bytes（3 秒窓・200 バイト閾値・連続 2 tick・5 秒静穏で idle・入力 500ms / resize 800ms の猶予）、
title（スピナー + 空白で始まるタイトル、3 秒更新が無ければ意見なし、スピナーを見た後の空でない通常のタイトルは idle）、
profile の画面判定（出力が落ち着くまで 120ms 待ち、最初の更新から 300ms 以内には判定）の定数は Node 版と同じです。違う点:

- **タイマーを持たない**: `StateTracker` は時刻（pane の起動からのミリ秒）を引数で受け取り、次の期限を `next_deadline` で返します。pane ごとの判定のスレッド（`pane_observer`）が期限まで眠って `advance` を呼びます。同じ入力列から必ず同じ判定列が出るので、差分テストは仮想の時間で動かせます。同じ時刻の期限は、登録した順に実行します。
- **profile の失敗の扱いが無い**: Node 版の `classify` は外から渡された関数なので、3 回続けて例外を投げた pane では無効にしていました。Rust 版の規則はデータで、評価は失敗しません。読み込みの時に正規表現などを検証し、壊れていれば起動に失敗します。
- **`pane.state` の `prev`**: 終了時も、直前の実際の状態になりました（第 2 段は `unknown` 固定）。`pane.exited` → `pane.state(exited)` の順は同じです。

### profile（Claude / Codex の規則）

Node 版の `AgentProfile` は差し込み口だけで、Claude Code / Codex の規則は同梱していません（`misao serve` は汎用の title / bytes だけで判定）。
Rust 版は規則をデータ（`crates/misao-daemon/src/profiles/*.json`）として持ち、AZITO の画面規則（`agentScreenRules`）を移植して同梱します。
**`misao serve` は同梱の profile を既定で使います**（Node 版との意図的な違い）。profile は pane の起動コマンドと、前面のプログラムで選びます。
`decidedBy` は `claude` / `codex` になり、`blocked` を出せます。

- **起動コマンド**: 実行ファイル名（先頭が `env` ならそのオプションと `NAME=VALUE` を飛ばした次の語。`.exe` は大文字小文字を問わず除く）が `claude` / `codex`。`sh -lc "cd d && claude --x"` のようにシェルへ文字列で渡したときは、その文字列の各コマンド（`&&` `||` `;` `|` `&` で区切った先頭の語。`exec` や `NAME=VALUE` は飛ばす）。
- **前面のプログラム（`fgCommand`）**: 判定のスレッドが一定間隔（Linux は 1 秒、`ps` を使う他の Unix は 5 秒）で前面のプログラムを調べ、使う profile が**変わったときだけ**切り替えます。対話シェルを起動してから `claude` を始めると、`claude` が前面に来た時点で claude の profile が付き、終わってシェルに戻ると外れます。前面が profile のプログラムならそれを使い、そうでなければ起動コマンドで選びます。ただし、起動コマンドがシェルで前面が別のプログラム（`claude` が終わって `git` が前面、など）なら profile なしです。
- **切り替えたときの state-tracker**: profile が付くと、いまの状態のまま画面の判定を予約します（結果は 120ms 以内）。title / bytes 段の履歴は残ります。profile が外れると、profile の意見を捨てて title / bytes 段だけで決め直し、どの段も意見を持たなければ `unknown` / `none` に戻します（`pane.state` イベントが出ます）。

規則は `{id, verdict, priority, region, when}` です。画面は Claude Code の入力欄（`─` の罫線 2 本とその間）で「上 / 入力欄 / 下」に分け、
`region`（`above` / `prompt_box` / `below` / `all` / `title`）で読む範囲を指定し、`when` は `all` / `any` / `not` / `contains`（小文字の部分文字列）/
`matches`（正規表現）/ `line_matches`（行ごとの正規表現）/ `last_lines`（末尾 n 行）の組み合わせです。優先順位が最も高く一致した規則が勝ちます。
`verdict` の `skip`（別の表示に切り替わっている）と `error`（API エラーで止まっている）は意見なし（title / bytes 段に任せる）にしますが、
下位の規則を黙らせます。AZITO の `skip` は「直前の状態を保つ」ですが、misao の profile の「意見なし」は次の段へ落ちる意味なので、そこは違います。
Codex のタイトル `Action Required` は `region: title` の規則です。

### 前面プロセス（`fgCommand`）

走行中の pane だけに付きます。Linux は pty の前面プロセスグループのリーダーの `/proc/<pgid>/cmdline` から（インタプリタ経由ならスクリプト名かモジュール名。
解釈は Node 版と同じ）。macOS は前面プロセスグループのリーダーの実行ファイル名を `ps -o comm=` で得ます（Node 版の node-pty の `process` と同じで、スクリプト名にはしません）。
Windows は ConPTY に前面プロセスグループが無く、誤った名前を報告するより報告しないことを選ぶので、付きません（Node 版と同じ）。

`pane.info` / `pane.list` の組み立て（前面プロセスの取得を含む）は、全体の lock を離してから行います。lock の中では所属・ラベル・`Arc<Pane>` のスナップショットだけを取ります。`pane.list` の前面プロセスは、macOS では一覧ぶんをまとめて `ps -o pid=,comm= -p a,b,c` を 1 回だけ起動して引きます。

## Node 版との差分テスト

`scripts/gen-rust-diff-fixtures.mjs` が、記録した出力の列を Node 版の部品（`@xterm/headless` + `StateTracker` + `foreground`）に流し、
`crates/misao-daemon/tests/fixtures/node-diff/*.json` に書き出します。`cargo test` はこれを `PaneView`（Rust 版の画面・タイトル・判定の接続）に
同じ時刻・同じ順序で流し、次を**完全に**突き合わせます（Node は不要）。

- 画面（`pane.screen` の `text` / `cursor` / `altScreen` / `title` / `activity`）、`pane.title` の列、`pane.state` の列（`state` / `decidedBy` / `prev` と**時刻**）、`agentState` / `decidedBy` の読み出し。
- 前面プロセス名の解釈、`/proc/<pid>/stat` の読み取り、表示名の整形（`foreground.json`）。

入力は `scripts/rust-diff/scenarios.mjs` です。Claude Code 風の UI（Ink 風の再描画、スピナーとタイトルの切り替え、許可プロンプト、質問、トランスクリプト表示、API エラー）、
Codex 風の画面、bytes / title だけの判定、終了、シェル出力（折り返し、タブ、スクロールバック）、全角・結合文字、代替画面、resize と、
種付きの乱数で混ぜた制御列（小さな画面・小さなスクロールバック・resize を含む）です。Node 版の参照実装の profile は `scripts/rust-diff/reference-profiles.mjs`（規則をコードで書いた別実装）で、
Rust 版のデータと同じ判定列になることを確かめます。仮想時間は `scripts/rust-diff/virtual-clock.mjs` です（実時間を使わないので、何度作っても同じ結果になる）。

```bash
npm run build
node scripts/gen-rust-diff-fixtures.mjs            # 再生成（Node 版の振る舞いを変えたら）
node scripts/gen-unicode-width.mjs                 # 文字幅の表
cargo test -p misao-daemon node_diff               # 突き合わせ
```

`packages/daemon/test/rust-fixtures-sync.test.ts`（`npm test`）が、コミット済みのファイルが Node 版の出力と一致するかを確かめます。
Rust 単独では、何を食わせても panic しないことを確かめるファズ（`src/panic_fuzz_tests.rs`: 固定の種で、極端な大きさの resize、代替画面、カーソルの保存と復元、極端な引数、壊れた UTF-8 を混ぜ、画面の行数などの不変条件も確かめる）も `cargo test` で動きます。

調査用に、コミットしない乱数シナリオを多めに作れます: `node scripts/gen-rust-diff-fixtures.mjs --out /tmp/fz --extra-fuzz 3000`、
`MISAO_NODE_DIFF_DIR=/tmp/fz cargo test -p misao-daemon node_diff`（`MISAO_FUZZ_EVERY=1` で全チャンクの後に画面を読む）。3000 本のうち 99.8% 以上が一致し、
残りは上の「再現しない xterm.js の内部状態の食い違い」です。

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
（debug ビルドでも通りますが、通常出力のテストに約 70 秒かかります）。`vim` が要ります。第 3 段の状況:

| テストファイル | Rust 版 | 理由 |
|---|---|---|
| `server-info.test.ts` | 通る | |
| `core-rpc.test.ts`（第 2 段で追加。Node 版でも通る） | 通る | 作業場所 / 窓、ペインの起動・env・入力・resize・close、行の記録、イベントと再接続 |
| `screen-rpc.test.ts`（第 3 段で追加。Node 版でも通る） | 通る | `pane.screen`・タイトル・`fgCommand`、vim の画面がサイズの所有者に追従すること（`v2-size-conflict` の画面の部分）、`pane.title` / `pane.state` の列 |
| `v3-line-markers.test.ts` | 通る | 行ストリームのマーカー、遅い購読者、再購読 |
| `v4-hub-restart.test.ts` | 通る | hub の再開（SDK の自動再接続と購読の復元）。画面の突き合わせに `pane.screen` を使う |
| `v5-key-collisions.test.ts` | 通る | vim / bash に制御キーを送っても落ちない（`pane.screen` で確かめる） |
| `v2-attach-replay`、`v2-size-conflict` | 通らない | `pane.attach`（第 4 段）が `not implemented` |
| `v5-catch-up` | 通らない | CLI の attach（第 4 段）が必要 |
| `v2-claude`、`v4-systemd` | オプトイン。スキップ | |

## 移行計画

issue で管理しています。#39（workspace・プロトコル適合・最小デーモン・CI）、#40 中核（RPC・作業場所/窓/ペイン・PTY・出力の記録・イベント）、
#41 画面と稼働判定（この段）、#42 状態の保存・attach・CLI 全体（既存 e2e を全部通す）、#43 macOS と Windows、#44 バイナリ配布と AZITO の切り替え。
背景は #32（部品の検討）と #37（Windows）。
