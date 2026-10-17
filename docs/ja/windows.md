# Windows（ネイティブ）対応の調査と試作

Issue #37 の第 1 段。コードと node-pty 1.1.0 の同梱物を読んでまとめた調査結果と、Linux で検証できる範囲の試作の説明です。
**Windows ホストでの実行はまだしていません。** 「要実機確認」と書いた箇所はすべて仮説で、末尾の確認項目で確かめます。

工数の目安: S = 半日まで、M = 1〜2 日、L = 3 日以上。

## 1. 要約

| 領域 | 現状 | Windows の方針 | 試作（この PR） | 工数 | 主なリスク |
| --- | --- | --- | --- | --- | --- |
| 接続口 | Unix ソケットのみ | 名前付きパイプ `\\.\pipe\misao-<user>-<hash>`。Node の `net` がそのまま扱える | 抽象層と解決を実装済み | S | パイプ名の先取り（squatting）。名前は 256 文字まで |
| 接続口の保護 | ソケット 600 + 親ディレクトリ 700 + uid 確認 | ACL で「起動したユーザーのみ」。**Node 単体では明示的に設定できない**（§2） | 既定の DACL に任せ、警告ログを出す | M〜L | 他ユーザーが接続できる可能性（要実機確認） |
| 前面プロセス | Linux は `/proc`、他は node-pty の `process` | ConPTY に前面プロセスグループは無い。コンソールのプロセス一覧からの推定が候補 | win32 は `undefined`（未対応を明示） | M | 現在の非 Linux 経路は Windows で端末名を返してしまう |
| ログインシェル | `os.userInfo().shell` + `-l` | `%COMSPEC%`（cmd.exe）を既定、`-l` なし。PowerShell は設定で選べるようにする | `%COMSPEC%` 既定を実装済み | S | `.cmd` / `.ps1` の shim は直接 spawn できない |
| 停止・再起動 | SIGTERM/SIGINT/SIGHUP、systemd / launchd | 優雅な停止は RPC で行い、常駐はタスクスケジューラ（ログオン時） | 未着手 | M | Windows の Node は SIGTERM を受け取れない |
| node-pty / ConPTY | Linux / macOS で使用 | 1.1.0 は win32 の prebuild を同梱。ConPTY を使う | CI でネイティブ導入を確認する | S（導入）+ M（挙動） | 子の終了処理、`encoding`、コマンド解決 |
| CRLF・再描画・稼働判定 | PTY の出力をそのまま扱う | ConPTY は出力を VT で再合成する。捕捉して回帰テスト化する | 未着手 | M〜L | 再描画が稼働判定・行ストリームを乱す |
| 設定・状態ファイル | POSIX 前提（700、rename） | `dataDir` を用意するだけにし、ACL は親（プロファイル）の継承に頼る | pid / state の置き場所を分離 | S | rename が AV に阻まれる（EPERM/EBUSY） |

## 2. 接続口と ACL

### 2.1 接続口の抽象（実装済み）

- `@misao/protocol`: `NAMED_PIPE_PREFIX`、`isNamedPipePath()`、パイプ名の上限（256）。
- `@misao/sdk`: `resolveSocketPath({ platform, userName })`。`platform === 'win32'` のときはパイプ名を返す。
  - 優先順位は Unix と同じ（`$MISAO_SOCKET` > `socket`（misao.json）> `$MISAO_DIR` > `~/.misao`）。
  - 既定の名前は `\\.\pipe\misao-<user>-<sha256(dir 小文字)[0:16]>`。ユーザー名は `[A-Za-z0-9._-]` 以外を `_` にして 32 文字で切る。
  - win32 で Unix のパスを指定したら `MisaoPathError`（黙って別の場所に作らない）。Unix 側で 107 バイト制限を適用するのは従来どおり。
- `@misao/daemon` の `endpoint.ts`: `prepareEndpoint` / `removeStaleEndpoint` / `listenEndpoint` / `endpointFiles`。`Daemon.start()` はここを通る。
  - Unix は従来の `socket.ts` に委譲するだけで、挙動は変えていない。
  - pipe は `listen({ path, readableAll: false, writableAll: false })`。`listenEndpoint` は保護の仕組み（`posix-mode` / `default-dacl`）を返し、`default-dacl` なら起動時に warn を出す。
  - pipe には親ディレクトリが無いので、pid と状態は `dataDir`（既定は misao のディレクトリ）へ置く。`misao serve` が境界で解決し、`buildDaemonOptions` は pipe で `dataDir` が無ければ失敗する。
  - pipe は作成したプロセスの終了で OS が消すため、stale の掃除もファイル削除も無い。接続できれば「別のデーモンが稼働中」として失敗する。

### 2.2 ACL を「起動したユーザーのみ」にする方法の比較

Node の `net.Server.listen` が名前付きパイプに対して渡せるのは `readableAll` / `writableAll` だけで、どちらも libuv の `uv_pipe_chmod` を呼んで**アクセスを広げる**方向にしか働きません。`SECURITY_ATTRIBUTES` を渡す手段は Node に無いので、**Node 単体で DACL を明示的に設定することはできません**（要実機確認: 以下は libuv のソースと Microsoft のドキュメントからの理解）。

| 案 | 内容 | 追加の依存 | 保護の強さ | 工数 | 備考 |
| --- | --- | --- | --- | --- | --- |
| A. 既定の DACL に任せる（現状の試作） | `NULL` の SECURITY_ATTRIBUTES で作られる DACL（Microsoft のドキュメントでは SYSTEM / 管理者 / 作成者に full、Everyone と匿名に read） | なし | 弱い。read だけでも接続時の挙動次第で漏れる可能性 | S | 実機で ACL を確認するまでは「保護済み」と言わない |
| B. ネイティブアドオンでパイプを自前で作る | `CreateNamedPipeW` に SDDL `D:P(A;;GA;;;<ユーザー SID>)` を渡し、Duplex として Node に橋渡し | **要承認**（N-API アドオン、または ffi 系。prebuild の配布も要る） | 強い（`FILE_FLAG_FIRST_PIPE_INSTANCE` と合わせて先取りにも強い） | L | libuv の pipe handle に既存のハンドルを渡せないため、サーバー全体をアドオンで実装する必要がある |
| C. listen 後に DACL を書き換える | `SetNamedSecurityInfo` 相当を外部から | PowerShell / .NET では他プロセスが作ったパイプの DACL を変更する手段が見当たらない。`icacls` はパイプを扱えない（要実機確認） | - | - | 現実的ではない |
| D. アプリ層で認証する | `%LOCALAPPDATA%\misao` の token ファイル（プロファイルの ACL で本人のみ）を接続直後に送らせる | なし（プロトコルに認証の交換を足す） | 中。接続はできるが何も操作できない。同一ユーザー内は Unix と同じく境界にならない | M | プロトコル 0.x のうちに入れるなら後方互換の問題が小さい |
| E. AF_UNIX を使う | Windows 10 1803 以降は OS が AF_UNIX を持ち、Unix と同じ「親ディレクトリの ACL」モデルにできる | なし | 強い | - | libuv が Windows で AF_UNIX を扱わない見込み（要実機確認: `net.createServer().listen('C:\\x\\a.sock')` の結果） |
| F. ローカル TCP + token | loopback で待ち受け、token で認証 | なし | 中（他ユーザーも接続は可能） | M | 接続口が TCP になり、SDK / AZITO の前提が変わる |

**推奨**: まず実機で A の ACL を確認する（確認項目 V1）。次に D（Node のみ）を第一候補、B を「完全な ACL が要件になった場合の第二候補」とする。B は依存の追加になるので承認が要ります。

そのほかに必要な対策:

- **クライアント側の先取り対策**: 他ユーザーが同じ名前のパイプを先に作る（squatting）と、SDK はそちらに接続してしまう。接続先サーバーのプロセス ID（`GetNamedPipeServerProcessId`）を確かめる手段が Node に無い。D の token はこれにも効く（token を持たないサーバーは応答できない）。サーバー側は libuv が `FILE_FLAG_FIRST_PIPE_INSTANCE` を付ける見込みで、二重起動は `EADDRINUSE` になる（要実機確認）。
- **リモートからの接続**: libuv は `PIPE_REJECT_REMOTE_CLIENTS` を付ける見込み（要実機確認）。
- **dataDir**: `%USERPROFILE%` 配下はプロファイルの既定 ACL（本人 / SYSTEM / 管理者）を継承する。Unix の 700 + uid 確認に当たる検証は、ACL を読む手段が Node に無いため未実装（シンボリックリンクとディレクトリであることだけ確認）。

## 3. 前面プロセス

- 現状の問題（実装を読んで判明）: `foreground.ts` は Linux 以外で node-pty の `process` を使うが、node-pty 1.1.0 の `WindowsTerminal.process` は `opt.name`（misao は `xterm-256color`）を返す。Windows ではシェルやコマンドの名前ではなく**端末名が前面コマンドとして報告される**ところだった。
- 試作: `readForegroundCommand(pid, ptyProcess, platform)` は win32 で常に `undefined`（`pane.info` の前面コマンドを省略）。稼働判定のプロファイル選択がこの値に依存するかは Phase 2 で確認する。
- 実装案:
  1. node-pty の `conpty_console_list`（`WindowsPtyAgent._getConsoleProcessList`）が返す、コンソールに接続されたプロセス ID 一覧の末尾（最後に接続したもの）を前面とみなす。内部 API なので、同梱の `conpty_console_list_agent.js` の呼び出し方を真似るか、公開を上流へ提案する。
  2. 名前の取得は `QueryFullProcessImageNameW` が必要だが Node に無い。`Get-CimInstance Win32_Process` や `tasklist` の起動は 1 回 100 ms 以上かかるため、周期的な取得には向かない。呼び出しは `pane.info` の要求時だけにし、結果をキャッシュする。
  3. 精度を求めるなら B と同じネイティブ依存になる（要承認）。
- 工数: M。取れない間は「省略」で足りる（表示用）。

## 4. ログインシェル

- 試作: `readLoginShell` は win32 で `os.userInfo()` を見ず `%COMSPEC%`（通常 `C:\Windows\System32\cmd.exe`）を返し、無ければ `null`（従来どおり usage エラーで `-- cmd` を促す）。`loginShell()` は win32 で `-l` を付けない（cmd.exe は受け付けない）。
- 候補: `pwsh.exe`（PowerShell 7）> `powershell.exe`（5.1）> `cmd.exe`。PATH 探索の結果が環境ごとに変わると再現性が落ちるため、既定は `%COMSPEC%` に固定し、変更は設定（`misao.json` の `shell` を後で追加）で選ばせる案を推す。
- 注意: npm の shim（`claude.cmd` / `codex.cmd`）や `.ps1` は ConPTY の `CreateProcess` に直接渡せない（拡張子付きの実行ファイルか、`cmd.exe /c` 経由が要る）。AZITO が `cmd: ["claude"]` を送ると失敗する可能性がある（要実機確認）。解決は AZITO 側の起動コマンドで `cmd.exe /c` を挟むか、misao 側で `PATHEXT` を解決する。
- 工数: S。

## 5. 停止と再起動

- シグナル: Windows の Node は `process.on('SIGTERM')` を受けられない（`process.kill(pid, 'SIGTERM')` は無条件に強制終了する）。受けられるのは Ctrl+C（SIGINT）、Ctrl+Break（SIGBREAK）、コンソールを閉じたとき（SIGHUP、約 10 秒で強制終了）。`serve.ts` の `SIGINT/SIGTERM/SIGHUP` 登録は Windows でも害は無いが、**外部からの優雅な停止手段にはならない**。
  - 方針: 優雅な停止用の RPC（例: `server.shutdown`、プロトコルの追加）と `misao stop` を用意する。強制終了されると、`finally` の `shutdown()`（pane を閉じて record を保存し、pid を消す）が走らない。pid ファイルは次回起動で生存確認により置き換えられる。
  - `attach` の `SIGWINCH` は Windows に無い。リサイズは `process.stdout.on('resize')` で取る。
- 常駐:
  | 方式 | 長所 | 短所 |
  | --- | --- | --- |
  | タスクスケジューラ（ログオン時、本人権限） | ユーザーセッションで動く。依存なし。`schtasks` / XML で定義でき、失敗時の再起動設定もある | 画面を持たないウィンドウ（`windowsHide`）の扱い、ログオンしていない間は動かない |
  | Windows サービス | ログオン不要 | Node 製のサービスには WinSW / NSSM などの外部バイナリか依存が要る。Session 0 でユーザー環境（PATH、プロファイル）が違う |
  推奨はタスクスケジューラ。`deploy/` に XML と導入手順を足す（systemd の `Restart=on-failure` に当たるのは `RestartOnFailure`）。
- pid ファイル: `process.kill(pid, 0)` は Windows でも存在確認として動く。
- 工数: M（RPC + `misao stop` + タスク定義）。

## 6. node-pty と ConPTY

node-pty 1.1.0（`packages/daemon` の依存）を同梱物から確認した結果:

- **ConPTY か winpty か**: `WindowsPtyAgent` は `useConpty` が未指定なら Windows のビルド番号が 18309 以上で ConPTY、それ未満で winpty（`winpty-agent.exe`）を使う。misao は指定していないので自動選択。Windows 10 1809 以降なら ConPTY。
- **ビルド**: `install` は `node scripts/prebuild.js || node-gyp rebuild`。`prebuilds/` には `win32-x64`・`win32-arm64`・`darwin-arm64`・`darwin-x64` があり（`conpty.node`、`conpty_console_list.node`、`pty.node`（winpty）、`winpty-agent.exe`、`winpty.dll`）、Windows では prebuild が使われてコンパイラは要らない見込み。Linux 向けの prebuild は無く、Linux では `node-gyp rebuild` で作られる。CI で確かめる（V6）。
- **spawn-helper**: Unix 専用（`UnixTerminal` が `fork` 時に使う補助バイナリで、macOS の prebuild に入っている）。Windows では使われず、何も要らない。
- **conpty.dll**: `useConptyDll`（既定 false）で同梱の OpenConsole 系 DLL（`third_party/conpty/1.23.251008001`）を使える。既定は OS 標準の ConPTY。OS が古い場合や挙動差（後述の再描画）が問題になれば切り替え候補。
- **API の差**（`lib/windowsTerminal.js` を読んで確認）:
  - `kill(signal)` は `signal` を渡すと `Signals not supported on windows.` を投げる。`Pane.close()` は `process.kill(-pid, 'SIGHUP')` → 失敗で `process.kill(pid)`（Windows では強制終了、ツリーは対象外）の経路になる。Windows では `proc.kill()`（ConPTY のコンソールにつながるプロセスを終了）を使う分岐が要る。**この PR では未実装**で、確認項目 V4 で挙動を見る。
  - `encoding: null` は `opt.encoding` が偽なので警告は出ない。データが Buffer で来るか文字列で来るかは要実機確認（misao は Buffer 前提）。
  - 終了後の `resize` は例外になる（`Cannot resize a pty that has already exited`）。`pane.resize` が exit 競合で投げないか確認する。
  - `proc.process` は端末名（§3）。
- 工数: 導入 S、挙動の吸収 M。

## 7. CRLF・再描画・稼働判定

ConPTY は、子プロセスの出力をそのまま通すのではなく、自分の画面バッファから VT シーケンスを再合成して出力する（要実機確認。Windows 11 22H2 以降の passthrough や OS の版で差が出る）。想定される影響:

- 起動直後の画面クリア（`ESC[2J` `ESC[H`）、カーソル表示切り替え（`ESC[?25l/h`）、ウィンドウタイトル（OSC 0）の出力。
- 改行は `\r\n`。行を追記する代わりに、行の消去と再書き込みで更新することがある。resize のたびに画面全体を再描画する。
- 影響を受ける misao の部分:
  - `AnsiLineAssembler`（行ストリーム）: 再描画で**同じ行が重複して流れる**可能性。
  - `byte-activity`: アイドル中の再描画（resize、カーソル点滅の同期）が「出力あり」と見え、**稼働の誤判定**になる可能性。
  - `title-activity`: Claude Code が設定するタイトルのグリフが ConPTY を通って残るか。ConPTY 自身が付けるタイトルと混ざらないか。
  - 画面（xterm-headless）の再構成: 再描画込みで描画した結果は同じ画面になるはずなので、スナップショットは影響が小さい見込み。
- 方針: Windows ホストで代表シナリオ（cmd、PowerShell、claude の起動・入力待ち・実行中、resize、終了）の生バイトを取り、フィクスチャにして `ansi` / `byte-activity` / `title-activity` / `screen` の回帰テストを足す。誤判定が出れば、Windows では byte-activity の閾値、または行ストリームの重複除去を調整する。
- 工数: M〜L（観察の結果次第）。

## 8. その他（POSIX 前提の箇所）

- `persistence.ts`: 一時ファイルを書いて `fsync` → `renameSync`。Windows では宛先が開かれていると `EPERM` / `EBUSY` になり得る（AV、エディタ）。リトライを検討（V7）。
- `scripts/fix-bin.mjs`: `chmodSync(…, 0o755)` は Windows では no-op で害は無い。bin は `main.js` で、Windows の shim は npm が作る。
- `scripts/smoke-bundle.mjs` は `/bin/sh` を使うので Windows では動かない（リリース配布物はスコープ外）。
- テスト: POSIX の権限（`chmod`、`getuid`、`/proc`）を前提にした単体テストは Windows で落ちる。CI の試験ジョブで洗い出し、Phase 2 で `skip`（理由付き）にするか、Windows 用の期待値を書く。

## 9. この PR で変えたもの / 変えていないもの

- 変えたもの: 接続口の解決と待ち受けの層、前面プロセスの win32 分岐、ログインシェルの win32 分岐、`serve` の dataDir 解決、CI の試験ジョブ。Linux / macOS の挙動は変えていない（既存のテストはすべて通る）。
- 変えていないもの（Phase 2 以降）: `Pane.close()` の win32 分岐、ACL の実装、停止 RPC、常駐定義、前面プロセスの推定、再描画の吸収。

## 10. Windows ホストで確かめる項目

前提: Node.js 24 以上（ポータブル版を使う場合はユーザーの承認の上で）。以下は PowerShell で、一時ディレクトリだけを使う。リポジトリを `npm ci && npm run build` しておく。

| ID | 目的 | 手順 | 期待 / 判断 |
| --- | --- | --- | --- |
| V1 | 既定の DACL | `node packages/cli/dist/main.js serve` を起動し、別のシェルで `$p=[IO.Pipes.NamedPipeClientStream]::new('.','misao-<解決された名前>','InOut'); $p.Connect(); $p.GetAccessControl().Access` | Everyone / Users に許可があれば §2.2 の A は不十分。許可が本人・SYSTEM・管理者だけなら A で足りる |
| V2 | 別ユーザーからの接続 | 標準ユーザーを別に作り、`runas` で同じ手順の接続を試す | 拒否されること |
| V3 | 二重起動と stale | `serve` を 2 つ起動し、片方を `Stop-Process -Force` して再起動 | 二重起動は失敗し、強制終了後は再起動できる |
| V4 | ConPTY のペイン | `misao new -- cmd.exe`、`misao send`、`misao screen`、`misao attach`、resize、`exit` | 入力・出力・resize・終了検知（`pane.state` が exited）が動く。`pane.close` で子が残らない |
| V5 | `server.info` / `pane.state` | `misao status`、`misao events` で idle/working の遷移を観察 | 何もしない間に working にならない（再描画の誤判定が無い） |
| V6 | CI のネイティブ導入 | PR の `Windows (experimental)` ジョブのログ | `npm ci` が成功し、node-pty の ConPTY スモークが `conpty-ok` を出す |
| V7 | 単体テスト | 同ジョブで `npm test` | 落ちたテストの一覧を Phase 2 の `skip` / 修正の対象にする |
| V8 | コマンド解決 | `misao new -- claude`（npm shim）、`-- powershell.exe`、`-- pwsh.exe` | shim が失敗するか確認。必要なら `cmd.exe /c` で包む |
| V9 | 前面コマンド | `pane.info` の前面コマンドが空になること、`proc.process` の値 | 端末名が漏れていない |
| V10 | AF_UNIX | `node -e "require('net').createServer().listen('C:\\\\Temp\\\\a.sock')"` | 失敗すれば E 案は不可 |
| V11 | 生バイトの採取 | cmd / PowerShell / claude の出力をファイルへ保存 | §7 のフィクスチャにする |

## 11. 次の段（提案）

1. V1〜V4 の結果で ACL の方針（D / B）を決める。
2. `Pane.close()` の win32 分岐、停止 RPC、`misao stop`。
3. 再描画のフィクスチャ化と稼働判定の調整。
4. 前面プロセスの推定、`shell` 設定、タスクスケジューラの定義。
