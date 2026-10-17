# Rust 版

[English](../en/rust.md) | 日本語

misao のデーモンと CLI を Rust の単一バイナリ `misao` に書き直しています。配布は GitHub Releases です。
SDK（`@misao/sdk`）と protocol（`@misao/protocol`、`PROTOCOL_VERSION` 0.3.0）は TypeScript のまま維持し、プロトコルは変えません。
Rust 版は既存の e2e（`packages/e2e`）と CLI のテスト（`packages/cli/test`）を全部通します（オプトインの Claude / Codex / systemd を除く）。
バイナリの配布と AZITO の切り替え（#44）までは、Node 版が正式版です。

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
  misao/                バイナリ: CLI の全サブコマンド（引数の解釈・クライアント・attach の端末制御）、設定、ソケットパス
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

| Node（`packages/`） | Rust（`crates/`） | 状況（第 4 段まで） |
|---|---|---|
| `protocol`（Zod スキーマ、`ErrorCode`、framing） | `misao-protocol` | `server.info`・JSON-RPC・エラー・NDJSON の型。スキーマと照合済み |
| `daemon/src/socket.ts` | `misao-daemon/src/socket.rs` | 完了（dir 700 / socket 600、所有者とシンボリックリンクの確認、古いソケット） |
| `daemon/src/pid-file.ts` | `misao-daemon/src/pid_file.rs` | 完了 |
| `daemon/src/daemon.ts`（`start` / `shutdown`） | `misao-daemon/src/daemon.rs` | 完了。起動の lock を終了まで保持する（下記） |
| `daemon/src/daemon.ts`（ディスパッチ）、`rpc-error.ts`、`params.ts` | `core.rs`、`rpc.rs`、`rpc_error.rs`、`params.rs` | 完了（`pane.send_keys` / `window.focus` / `pane.respawn` は Node 版と同じく `NotImplemented`） |
| `daemon/src/connection.ts`、`stream.ts` | `connection.rs`、`stream.rs` | 完了（送信キューの高水位、pull 型の購読、追い越された購読者の切断） |
| `daemon/src/layout.ts`、`layout-handlers.ts` | `layout.rs`、`layout_handlers.rs` | 完了 |
| `daemon/src/pane-registry.ts`、`pane-lifecycle.ts`、`pane-io.ts` | `registry.rs`、`pane_handlers.rs`、`pane_lifecycle.rs`、`attach_handlers.rs`、`attachment.rs` | 完了 |
| `daemon/src/pane.ts` | `pane.rs`、`pty.rs`、`pane_view.rs`、`pane_observer.rs` | PTY（portable-pty）、入力、resize、終了検知、close、画面・タイトル・稼働判定との接続（`pane_view` が部品をつなぎ、`pane_observer` が lock・判定のスレッド・イベントを持つ） |
| `@xterm/headless`、`daemon/src/screen.ts`（`viewportText`） | `screen.rs`、`screen_{emulator,perform,cursor,edit,buffer,line,reflow,charset}.rs`、`unicode_width{,_table}.rs` | 完了（`serializeSnapshot` は `screen_snapshot.rs`、色・飾りは `screen_attr.rs`）。vte のパーサー + xterm.js と同じ意味の自前の画面（後述） |
| `daemon/src/state-tracker.ts`、`byte-activity.ts`、`title-activity.ts` | `state_tracker.rs`、`byte_activity.rs`、`title_activity.rs` | 完了。閾値・窓・猶予は Node 版と同じ。タイマーは持たず、時刻は引数（後述） |
| `daemon/src/profile.ts`（差し込み口だけ。規則は Node 版に無い） | `profile.rs`、`profiles/{claude,codex}.json` | 完了。規則をデータとして持ち、AZITO の画面規則から Claude / Codex を移植（後述） |
| `daemon/src/foreground.ts` | `foreground.rs` | 完了（Linux は `/proc`、macOS は `ps`、Windows は未対応）|
| `daemon/src/size-arbiter.ts`、`ring.ts`、`ansi.ts`、`child-env.ts`、`event-log.ts`、`limits.ts`、`ulid.ts` | `size_arbiter.rs`、`ring.rs`、`ansi.rs`、`child_env.rs`、`event_log.rs`、`limits.rs`、`ids.rs` | 完了 |
| `daemon/src/log.ts` | `misao-daemon/src/log.rs` | 完了（stderr に `[misao <ISO 時刻>] msg`） |
| `sdk-node/src/socket-path.ts` | `misao/src/socket_path.rs` | 完了（優先順位も 107 バイト制限も同じ） |
| `daemon/src/persistence.ts`、`state-persister.ts`、`attach.ts` | `persistence.rs`、`state_persister.rs`、`attach_handlers.rs`、`attachment.rs`、`screen_snapshot.rs`、`screen_attr.rs` | 完了（第 4 段。下記） |
| `cli/src/config/*` | `misao/src/config.rs`、`keys.rs` | 完了（`keys` も使う） |
| `cli/src/**`（全サブコマンド、attach、target、view） | `misao/src/{args,client,commands,attach,view,target,follow,connect,io,tty_sys}*` | 完了（第 4 段。下記） |

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
- **未対応**: `preplace`（Node 版も未実装）。状態の保存と attach は第 4 段で入った（次の節）。
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
- **OSC の長さの上限**: `vte` は `std` 機能のとき OSC の中身を上限なしに貯めます。`std` を外さず、パーサーに渡す前に OSC の長さを数えます（`screen_osc.rs`）。1 MiB に達したら CAN を差し込んで中止し、終端までの残りを捨てます。中止された OSC は何も起こしません（xterm.js が上限を超えた OSC を捨てるのと同じ）。切り詰めたタイトルにはなりません。OSC の途中の CAN / SUB で中止された OSC も、何も起こしません（`vte` は中止の前に dispatch するので、中止の 1 バイトを渡す間だけ dispatch を捨てます）。OSC の途中の 8 ビットの ST（U+009C = `C2 9C`）は `ESC \` として終端します（`C2` に別のバイトが続けば、中身の一部のままです）。xterm.js の上限は 10 MB ですが、実際の OSC（タイトルなど）は数 KB です。
- **行リストの食い違いの補正**: 通常と違う操作の組み合わせで行リストが見えている行に足りなくなったとき、空行で補います（上の「再現しない xterm.js の内部状態の食い違い」の後始末。画面の操作が常に成り立つようにするため）。
- 画面の行は、xterm.js と同じく「明示的に書かれた空白は残し、消去されたセルは行末から除く」です。

### 稼働判定

exit > profile > title > bytes の順は同じです。bytes（3 秒窓・200 バイト閾値・連続 2 tick・5 秒静穏で idle・入力 500ms / resize 800ms の猶予）、
title（スピナー + 空白で始まるタイトル、3 秒更新が無ければ意見なし、スピナーを見た後の空でない通常のタイトルは idle）、
profile の画面判定（出力が落ち着くまで 120ms 待ち、最初の更新から 300ms 以内には判定）の定数は Node 版と同じです。違う点:

- **タイマーを持たない**: `StateTracker` は時刻（pane の起動からのミリ秒）を引数で受け取り、次の期限を `next_deadline` で返します。pane ごとの判定のスレッド（`pane_observer`）が期限まで眠って `advance` を呼びます。同じ入力列から必ず同じ判定列が出るので、差分テストは仮想の時間で動かせます。同じ時刻の期限は、登録した順に実行します。
- **profile の失敗の扱いが無い**: Node 版の `classify` は外から渡された関数なので、3 回続けて例外を投げた pane では無効にしていました。Rust 版の規則はデータで、評価は失敗しません。読み込みの時に正規表現などを検証し、壊れていれば起動に失敗します。
- **出力と resize の順序**: 端末の解析はリングの更新の lock の外で行うので、pane ごとの順序づけの lock（order -> inner -> observer）で、出力の取り込み（格納と解析）と resize を受け取った順に画面へ反映します。
- **`pane.state` の `prev`**: 終了時も、直前の実際の状態になりました（第 2 段は `unknown` 固定）。`pane.exited` → `pane.state(exited)` の順は同じです。

### profile（Claude / Codex の規則）

Node 版の `AgentProfile` は差し込み口だけで、Claude Code / Codex の規則は同梱していません（`misao serve` は汎用の title / bytes だけで判定）。
Rust 版は規則をデータ（`crates/misao-daemon/src/profiles/*.json`）として持ち、AZITO の画面規則（`agentScreenRules`）を移植して同梱します。
**`misao serve` は同梱の profile を既定で使います**（Node 版との意図的な違い）。profile は pane の起動コマンドと、前面のプログラムで選びます。
`decidedBy` は `claude` / `codex` になり、`blocked` を出せます。

- **起動コマンド**: 実行ファイル名（先頭が `env` ならそのオプションと `NAME=VALUE` を飛ばした次の語。`.exe` は大文字小文字を問わず除く）が `claude` / `codex`。`sh -lc "cd d && claude --x"` のようにシェルへ文字列で渡したときは、その文字列の各コマンド（`&&` `||` `;` `|` `&` で区切った先頭の語。`exec` や `NAME=VALUE` は飛ばす）。
- **前面のプログラム（`fgCommand`）**: デーモンに 1 つのサンプラー（`foreground_sampler.rs`）が一定間隔（Linux は 1 秒、`ps` を使う他の Unix は 5 秒）で、起動コマンドだけでは profile が決まらない pane（シェルなど。`claude` を直接起動した pane は対象外）をまとめて調べて各 pane に配り（macOS は全 pane で `ps` を 1 回）、使う profile が**変わったときだけ**切り替えます。対話シェルを起動してから `claude` を始めると、`claude` が前面に来た時点で claude の profile が付き、終わってシェルに戻ると外れます。前面が profile のプログラムならそれを使い、そうでなければ起動コマンドで選びます。ただし、起動コマンドがシェルで前面が別のプログラム（`claude` が終わって `git` が前面、など）なら profile なしです。
- **切り替えたときの state-tracker**: profile が付くと、いまの状態のまま画面の判定を予約します（結果は 120ms 以内）。title / bytes 段の履歴は残ります。profile が外れると、profile の意見を捨てて title / bytes 段だけで決め直し、どの段も意見を持たなければ `unknown` / `none` にします。最終の判定を 1 度だけ出すので、`pane.state` イベントは 1 件です（`unknown` を挟みません）。

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

## 第 4 段の振る舞い（保存・attach・CLI）

### 状態の保存と復元

`persistence.json`（`--data` または socket の親ディレクトリ）は **Node 版と同じ形式で、相互に読めます**（`packages/e2e/test/persistence.test.ts` が
Node で保存 -> Rust で復元、Rust で保存 -> Node で復元を確かめる）。保存するのは workspace / window の定義と pane のメタ（`paneId`・`windowId`・`cmd`・`cwd`・`env`・`labels`・`cols`・`rows`）で、
`env` は `pane.open` の `env` だけです（`ephemeralEnv` は保存しない）。書き方は、同じディレクトリにランダムな名前の一時ファイルを `O_EXCL`・`O_NOFOLLOW`・mode 600 で作り、書く -> ファイルを fsync -> rename -> 親ディレクトリを fsync、です
（Node 版は固定名の一時ファイルに書いて fsync と rename だけで、ディレクトリの fsync と O_EXCL / O_NOFOLLOW はありません）。
データディレクトリは、ソケットのディレクトリと同じ検証（シンボリックリンクでない・ディレクトリ・所有者が自分・group / other に権限が無い）を起動時に行い、違反は起動エラーにします（Node 版は検証しない）。
読み込みは `O_NOFOLLOW` で開き、通常のファイルであること・32 MiB 以下であることを確かめてから読みます。workspace・window・pane は各 1 万まで、`cmd` / `env` / `labels` は各 4096 要素まで、文字列は 1 MiB までで、
超えるファイルは起動エラーです。保存するときも同じ上限で断る（読めないファイルを作らない）ので、上限に達した `pane.open` は保存の失敗として元に戻ります（Node 版にこれらの上限は無い）。

- **再起動**: 保存してある pane は `stopped` で復元されます（`pid` は `null`、`agentState` は `unknown`）。ラベルの編集と `pane.close` はでき、
  `pane.write` / `resize` / `screen` / `attach` / `subscribe_lines` は `PaneExited`（`pane is stopped`）です。`paneCount` と `pane.list` に含まれます。
- **壊れたファイル**: `invalid persistence file <path>: <詳細>` で起動に失敗します（pid ファイルは残さない）。`version` が 1 でない、
  必須項目の欠落・余計な項目、`cols` / `rows` が正の整数でない、ID の形式違反、workspace 名・window ID・pane ID の重複、存在しない window への所属が対象です。詳細の文言は serde のもので Zod とは違います。
- **保存の時機**: `pane.open` / `pane.close` / `pane.set_label` / workspace・window の作成・rename・close は保存してからイベントを出します。
  作成・rename・ラベルは保存に失敗したら元に戻します。`pane.open` が保存に失敗したら、作った layout も登録も戻し、起動した子はイベントを出さずに止めます。
  `close` は子を止めてしまっているので戻さず、メモリは実態のまま `pane.closed` を出してから失敗を返します。サイズ（`cols` / `rows`）は 1 秒遅らせて保存し、失敗はログだけで、デーモンの終了時に残りを保存します。保存の最中に入ったサイズの予約は世代番号で見分け、取りこぼさず次の保存に回します。
- **排他（Node 版との違い）**: Node 版は単一スレッドなので「変更 -> 保存 -> 失敗なら戻す」が割り込まれません。Rust 版は保存する状態を変える操作を `gate`（非同期の mutex）で 1 つずつ行い、
  fsync は全体の状態の lock の外で行います（読み取り・入出力は待たない）。書き込みの順序づけの lock を取ってからスナップショットを作るので、古い状態が新しい状態を上書きしません。lock の順序は gate -> 書き込み -> 全体の状態です。

### attach と複数クライアント

`pane.attach`（`mode: raw`、`replay: raw | snapshot | none`）と `pane.detach` は Node 版と同じ契約です。再生は応答の後に送り、境界は `head` です
（出力の取り込みと同じ lock の中で届け先の登録と `head` の読み取りを行うので、再生とライブの間に隙間も重なりも出ません）。
`raw` は生出力リングの全体（`truncated` はリングが欠けているか）、`snapshot` は見えている行だけを描き直す 1 通知（`replay: "snapshot"`）で、`mode: cells` は `Unsupported` です。
pane のサイズは最後に操作した（`pane.write` の `clientId`、`pane.attach` / `pane.resize` のサイズ）クライアントのもので、所有者が抜けると、残ったクライアントのうち最後に操作したもののサイズに戻ります。
接続が切れたときも同じ後始末をします。attach・detach・close の後始末（届け先・clients・サイズの所有者・「close が始まった」印）は pane の `order` の下で 1 つの操作として行い、close が始まった後の attach は `PaneExited`（`pane is closing`）で断ります。
送信キューは、メッセージを足したときの合計が 16 MiB を超えるなら、投入せずに接続を切ります（1 通で上限を飛び越えない）。応答前の attach が溜める出力も 16 MiB までで、超えたら attach を諦めて接続を切ります。
snapshot は 1 通の通知です。画面は最大 1024 x 1024 セルで、文字だけなら約 1 MiB（base64 で約 1.4 MiB）ですが、ほぼ全セルで色や飾りが変わる最悪の画面（1 セルあたり最大 60 バイト程度の SGR）では 16 MiB を超ええます。
その場合は上の規則で接続が切れ、クライアントは `replay: none` か `raw` で入り直します。現実の画面（数百 x 数十セル）では 1 MiB を大きく下回ります。同じ接続・同じ `clientId` の再 attach は届け先だけを差し替え、`client.detached` を出しません。

Node 版と違う点: pane を `close` したとき、attach していたクライアントには `client.detached` を出しますが、所有者の交代による `pane.resized` は出しません（閉じる pane のサイズを変えても意味が無いため）。

#### snapshot（色と飾り）

画面モデルは第 3 段まで文字だけでしたが、snapshot が見た目を再現できるよう、セルに属性（太字・薄字・斜体・下線・点滅・反転・非表示・取り消し線、前景色・背景色: 標準 16 色 / 256 色 / RGB）を持たせました
（セルは 16 バイトから 24 バイトになる）。`CSI m`（`;` 区切りも `:` 区切りも）、消去が背景色を引き継ぐ BCE、挿入・削除・スクロール、代替画面の初期化、カーソルの保存・復元、リセットで xterm.js と同じ意味にしています。
入力モード（アプリケーションカーソル / キーパッド、括弧付き貼り付け、フォーカス通知、マウスの報告）も持ち、snapshot の末尾で復元します。カーソルの表示 / 非表示とスクロール領域を復元しないのは Node 版と同じです。
差分テストは snapshot の文字列も Node 版（`serializeSnapshot`）と**完全に**突き合わせます（`snapshot-look` シナリオと、色を混ぜた乱数シナリオ）。xterm.js の癖で合わせたもの:
`CSI T` だけは消去した行に背景色を引き継がない、代替画面の行は消去の属性で作る、`CSI 38:2:r:g:b` は r を色空間として読む、挿入モードの文字入力は現在の属性で空きを埋める。
4000 本の追加の乱数シナリオのうち、画面の文字まで食い違うのは 3 本（0.1%、第 3 段の「再現しない内部状態」）、属性だけが食い違うのは 1 本（折り返しの再配置で xterm.js が属性を取り違える例）です。

### CLI

`misao` の全サブコマンド（`ls` / `attach` / `new` / `kill` / `status` / `send` / `screen` / `tail` / `events` / `label` / `schema` / `serve`）の出力形式・終了コード・メッセージ（日本語）・`--json` の出力・`--help` の文面は Node 版と同じです。
正本は Node 版のソースとテストで、引数の解釈は `util.parseArgs` の strict と同じ振る舞いを**手書きで**実装しています（clap は使いません: エラーの文言・終了コード 2・`--json` のエラーを Node 版と一致させるため）。
CLI は tokio の接続を持つクライアント（1 つの actor が接続を持ち、購読の応答を再生の通知より先に適用する。Node の SDK と同じ指数バックオフで再接続し、`since` / `epoch` を復元する）で、
`attach` は端末を raw にして prefix キーの状態機械・SIGWINCH・終了シグナル・端末の復元（`TTY_RESET`）を扱います（Unix のみ。他の OS は後続の段）。
Node 版との違い: raw モード中の panic は端末を戻して `[misao] 予期しないエラー` を出し終了コード 1 にする（Node の `uncaughtException`）、stdin の EOF は「シグナル」と同じ終わり方、`misao.json` の `keys` / `rings` の未知のキーは警告（Node 版と同じ）。

`misao.json`（`socket` / `logLevel` / `scrollback` / `rings` / `keys`）、制限値の既定、ログ（stderr に `[misao <ISO 時刻>] msg`）は Node 版と同じ検証・既定です。

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
残りは上の「再現しない xterm.js の内部状態の食い違い」です（snapshot を足した後は 4000 本のうち 4 本）。

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

## Rust 版で e2e と CLI のテストを動かす

`packages/e2e/test/helpers/daemon-process.ts` は `misao serve --config ... --socket ... --data ...` でデーモンを起動します。
`MISAO_DAEMON_BIN` を指定すると、Node の CLI の代わりにその実行ファイルを使います。CLI を起動するテスト（`v5-catch-up` の attach など）と `packages/cli/test` は
`MISAO_CLI_BIN` を指定するとそのバイナリを使います。

```bash
cargo build --release -p misao
B=$PWD/target/release/misao
MISAO_DAEMON_BIN=$B MISAO_CLI_BIN=$B npm run test:e2e:rust -w @misao/e2e   # e2e の全部（CI）
MISAO_DAEMON_BIN=$B MISAO_CLI_BIN=$B npm run test:rust -w misao             # CLI のテスト（CI）
```

変数を指定しなければ何も変わりません（Node 版）。大量出力のテスト（`v3-line-markers`）は処理速度に左右されるので `release` ビルドで動かしてください。`vim` が要ります。

| テストファイル | Rust 版 |
|---|---|
| `server-info`、`core-rpc`、`screen-rpc`、`v3-line-markers`、`v4-hub-restart`、`v5-key-collisions` | 通る |
| `v2-attach-replay`、`v2-size-conflict`（`pane.attach`、raw / snapshot の再生、サイズの調停） | 通る |
| `v5-catch-up`（Rust の CLI の attach） | 通る |
| `persistence`（再起動後の `stopped` の復元。Node で保存 -> Rust、Rust で保存 -> Node の相互も） | 通る |
| `v2-claude`、`v4-systemd`、claude / codex のケース | オプトイン。スキップ |

`packages/cli/test` のうち、`run(argv, io)` でコマンドを動かすテスト（`commands.test.ts`、`commands/follow.test.ts`、`commands/serve.test.ts`、pty で attach する `attach/binary.test.ts`）は、`MISAO_CLI_BIN` があるとバイナリを子プロセスとして起動します
（`MISAO_DAEMON_BIN` もあると、相手のデーモンも Rust 版）。Node の内部関数を呼ぶテスト（`args`、`target`、`send-keys`、`cli-io`、`config/*`、`view/*`、`attach/{session,loop,picker,prefix-filter,tty}`）は
同じ観点を `cargo test -p misao` に移してあります。

## AZITO の統合テストを Rust 版に向ける

AZITO（`mission/misao-driver`）の `e2e/specs/misao.spec.ts`（`npm run e2e` は一時ハブ・一時ポート・一時 tmux で動く）と `packages/server` の `*.misaoIntegration.test.ts` は、
`MISAO_CLI` に指した JS ファイルを `node <MISAO_CLI> serve ...` で起動します（`MisaoAgentInstaller` のテストは `import '<MISAO_CLI>'` する）。Rust 版を指す
1 行のラッパー（`process.execve` で自分を Rust のバイナリに置き換えるので pid も保たれる。環境を引き継がないテストがあるので、パスはファイルに直接書く）を置けば、AZITO 側を変えずに流せます:

```js
const RUST = '/path/to/target/release/misao';
process.execve(RUST, [RUST, ...process.argv.slice(2)], process.env);
```

結果（デーモンと CLI の両方が Rust 版）: `misao.spec.ts` 7 本が 3 回とも通過、`vitest --project misao-integration` の 10 ファイル 69 本が通過、misao 関連の単体テスト（541 本）も通過。
デーモンだけ Rust 版にした 4 回の実行のうち 1 回、稼働検知のテストが完了行の表示で失敗しました（再現せず。同じイベント列を Node 版と Rust 版で比べると一致するので、
ハブ側のタイミングと見ています）。

## メモリと CPU の比較計測（#32 の条件）

`scripts/bench-daemon.mjs` が、同じペイン数・同じ scrollback・同じ出力でデーモンのメモリ（RSS）と CPU を測ります（Linux 専用。一時ディレクトリのデーモンで、`~/.misao` には触れない）。

```bash
npm run build && cargo build --release -p misao
node scripts/bench-daemon.mjs --panes 10,30,60 --patterns quiet,bulk,tui           # 既定: scrollback 5000
node scripts/bench-daemon.mjs --panes 30 --patterns bulk --scrollback 20000         # 感度（--raw-bytes / --lines-bytes も）
```

RSS は `/proc/<pid>/status` の VmRSS、CPU は `/proc/<pid>/stat` の utime + stime（デーモン 1 プロセスだけ。pane の子は含まない）。出力は quiet（出力なし）/
bulk（各 pane が約 6.9 MB = 10 万行を一気に出す）/ tui（Claude Code 風の 12 行を 10 回/秒で再描画、20 秒）。pane は 120 x 40。ヒープ（`heapUsed`）は外から測れないので RSS で比べます。
測定環境: WSL2、22 論理コア、他のプロセスが動いている開発機（絶対値は目安）。

| 出力 | pane 数 | Node 起動直後 | Node RSS | Rust 起動直後 | Rust RSS | Node CPU 時間 | Rust CPU 時間 |
|---|---|---|---|---|---|---|---|
| quiet | 10 | 72 MiB | 74 MiB | 6 MiB | 8 MiB | 0.1 s | 0.0 s |
| quiet | 60 | 72 MiB | 96 MiB | 6 MiB | 16 MiB | 0.5 s | 0.8 s |
| tui | 10 | 73 MiB | 87 MiB | 6 MiB | 13 MiB | 1.2 s / 20 s | 0.2 s / 21 s |
| tui | 30 | 72 MiB | 119 MiB | 6 MiB | 28 MiB | 2.2 s / 21 s | 0.7 s / 21 s |
| tui | 60 | 72 MiB | 167 MiB | 6 MiB | 51 MiB | 3.7 s / 21 s | 1.9 s / 22 s |
| bulk | 10 | 72 MiB | 444 MiB | 6 MiB | 222 MiB | 6.8 s（6.7 s で完了） | 5.8 s（2.1 s で完了） |
| bulk | 30 | 73 MiB | 1020 MiB | 6 MiB | 651 MiB | 38.8 s（33.5 s） | 39.9 s（6.3 s） |
| bulk | 60 | - | -（メモリ不足を避け未計測） | 6 MiB | 1293 MiB | - | 99.3 s（13.6 s） |

読み方:
- **固定コスト**: Node 約 72 MiB（この版の依存を読み込んだ後。#32 の 35 MiB より増えている）に対し、Rust は約 6 MiB。
- **ペインあたりのメモリ**: 静穏はどちらも小さく（Node 0.4 MiB、Rust 0.2 MiB）、TUI は Node 1.5 MiB、Rust 0.7 MiB。大量出力は scrollback 5000 行 + 行リング 4 MiB + 生リング 1 MiB が埋まった状態で、
  Node 約 32-37 MiB、Rust 約 21.5 MiB（セルが 24 バイト: 文字 + 属性 + 幅 + 結合文字の箱）。60 ペインの大量出力でも Rust は 1.3 GiB で、ペイン数に比例します。
- **CPU**: 同じ出力量を処理する CPU 時間の合計は大量出力で Node とほぼ同じ（Rust は端末の解釈と行の組み立てが pane ごとのスレッドに並列で乗るため、**完了までの時間は 5 倍短い**）。
  静穏と TUI では Rust が 2-5 倍少ない。静穏の 60 ペインは pane の起動（pty と子の fork）の CPU が主で、Rust が多く見える。
  Node は 1 スレッドなので大量出力中は 1 コア（約 110 %）で頭打ちになり、その間 RPC の応答も遅れます。

scrollback と rings の感度（bulk、30 ペイン）:

| 設定 | Node RSS | Rust RSS |
|---|---|---|
| scrollback 1000 | 696 MiB | 313 MiB |
| scrollback 5000（既定） | 1020 MiB | 651 MiB |
| scrollback 20000 | 1874 MiB | 1919 MiB |
| scrollback 5000、`rawBytes` 256 KiB、`linesBytes` 1 MiB | 865 MiB | 498 MiB |

scrollback が大きいと Rust のセル（24 バイト）が Node の xterm.js（セル 12 バイト）を上回ります（20000 行で逆転）。セルを小さくする余地（結合文字を行の外に出す、属性を表に入れる）は残してあります。

## 移行計画

issue で管理しています。#39（workspace・プロトコル適合・最小デーモン・CI）、#40 中核（RPC・作業場所/窓/ペイン・PTY・出力の記録・イベント）、
#41 画面と稼働判定、#42 状態の保存・attach・CLI 全体（既存 e2e を全部通す。この段）、#43 macOS と Windows、#44 バイナリ配布と AZITO の切り替え。
背景は #32（部品の検討）と #37（Windows）。
