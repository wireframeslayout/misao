# Windows（ネイティブ）対応の調査と試作

Issue #37 の第 1 段。コードと node-pty 1.1.0 の同梱物を読んでまとめた調査結果と、Linux で検証できる範囲の試作の説明です。
調査の時点では Windows ホストで実行していませんでした。「要実機確認」と書いた箇所は仮説で、§10 の確認項目で確かめ、結果を §10.1 に記録しました（実機で確かめた箇所は本文にも反映済み）。

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
| A. 既定の DACL に任せる（現状の試作） | `NULL` の SECURITY_ATTRIBUTES で作られる DACL。**実機で確認済み**: SYSTEM / Administrators / 作成者に full、Everyone と ANONYMOUS LOGON に read（§10.1 V1） | なし | 弱い。read だけの接続は何も受け取れず、書き込み権が無いので要求も送れない（実機で確認）が、誰でも接続でき、SMB 経由（別ホスト）でも届く（§10.1 V2） | S | 「起動したユーザーのみ」ではない。先取り（squatting）にも無力 |
| B. ネイティブアドオンでパイプを自前で作る | `CreateNamedPipeW` に SDDL `D:P(A;;GA;;;<ユーザー SID>)` を渡し、Duplex として Node に橋渡し | **要承認**（N-API アドオン、または ffi 系。prebuild の配布も要る） | 強い（`FILE_FLAG_FIRST_PIPE_INSTANCE` と合わせて先取りにも強い） | L | libuv の pipe handle に既存のハンドルを渡せないため、サーバー全体をアドオンで実装する必要がある |
| C. listen 後に DACL を書き換える | `SetNamedSecurityInfo` 相当を外部から | PowerShell / .NET では他プロセスが作ったパイプの DACL を変更する手段が見当たらない。`icacls` はパイプを扱えない（要実機確認） | - | - | 現実的ではない |
| D. アプリ層で認証する | `%LOCALAPPDATA%\misao` の token ファイル（プロファイルの ACL で本人のみ）を接続直後に送らせる | なし（プロトコルに認証の交換を足す） | 中。接続はできるが何も操作できない。同一ユーザー内は Unix と同じく境界にならない | M | プロトコル 0.x のうちに入れるなら後方互換の問題が小さい |
| E. AF_UNIX を使う | Windows 10 1803 以降は OS が AF_UNIX を持ち、Unix と同じ「親ディレクトリの ACL」モデルにできる | なし | 強い | - | libuv が Windows で AF_UNIX を扱わない見込み（要実機確認: `net.createServer().listen('C:\\x\\a.sock')` の結果） |
| F. ローカル TCP + token | loopback で待ち受け、token で認証 | なし | 中（他ユーザーも接続は可能） | M | 接続口が TCP になり、SDK / AZITO の前提が変わる |

**推奨**: まず実機で A の ACL を確認する（確認項目 V1）。次に D（Node のみ）を第一候補、B を「完全な ACL が要件になった場合の第二候補」とする。B は依存の追加になるので承認が要ります。

そのほかに必要な対策:

- **クライアント側の先取り対策**: 他ユーザーが同じ名前のパイプを先に作る（squatting）と、SDK はそちらに接続してしまう。接続先サーバーのプロセス ID（`GetNamedPipeServerProcessId`）を確かめる手段が Node に無い。D の token はこれにも効く（token を持たないサーバーは応答できない）。サーバー側は libuv が `FILE_FLAG_FIRST_PIPE_INSTANCE` を付けており、同名のパイプを二重に作ると `EADDRINUSE` になる（実機で確認、§10.1 V3）。
- **リモートからの接続**: libuv は `CreateNamedPipeW` に `PIPE_REJECT_REMOTE_CLIENTS` を付けない（付くのは `FILE_FLAG_FIRST_PIPE_INSTANCE` だけ）。**リモートからの接続が拒否されることを期待しない**。実機で、`\\127.0.0.1\pipe\<名前>` と `\\<ホスト名>\pipe\<名前>`（SMB 経由）から接続して JSON-RPC の応答まで得られることを確認した（§10.1 V2。別ホストからの実測は未実施）。防ぐには ACL（B）か認証（D）が要る。
- **dataDir**: `%USERPROFILE%` 配下はプロファイルの既定 ACL（本人 / SYSTEM / 管理者）を継承する。Unix の 700 + uid 確認に当たる検証は、ACL を読む手段が Node に無いため未実装（シンボリックリンクとディレクトリであることだけ確認）。

## 3. 前面プロセス

- 現状の問題（実装を読んで判明）: `foreground.ts` は Linux 以外で node-pty の `process` を使うが、node-pty 1.1.0 の `WindowsTerminal.process` は `opt.name`（misao は `xterm-256color`）を返す。Windows ではシェルやコマンドの名前ではなく**端末名が前面コマンドとして報告される**ところだった（実機で `proc.process === "xterm-256color"` を確認、§10.1 V9）。
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

- **ConPTY か winpty か**: `WindowsPtyAgent` は `useConpty` が未指定なら Windows のビルド番号が 18309 以上で ConPTY、それ未満で winpty（`winpty-agent.exe`）を使う。misao は指定していないので自動選択。Windows 10 1903（ビルド 18362）以降は ConPTY、1809（ビルド 17763）は winpty になる。
- **ビルド**: `install` は `node scripts/prebuild.js || node-gyp rebuild`。`prebuilds/` には `win32-x64`・`win32-arm64`・`darwin-arm64`・`darwin-x64` があり（`conpty.node`、`conpty_console_list.node`、`pty.node`（winpty）、`winpty-agent.exe`、`winpty.dll`）、Windows では prebuild が使われてコンパイラは要らない見込み。Linux 向けの prebuild は無く、Linux では `node-gyp rebuild` で作られる。CI で確かめる（V6）。
- **spawn-helper**: Unix 専用（`UnixTerminal` が `fork` 時に使う補助バイナリで、macOS の prebuild に入っている）。Windows では使われず、何も要らない。
- **conpty.dll**: `useConptyDll`（既定 false）で同梱の OpenConsole 系 DLL（`third_party/conpty/1.23.251008001`）を使える。既定は OS 標準の ConPTY。OS が古い場合や挙動差（後述の再描画）が問題になれば切り替え候補。
- **API の差**（`lib/windowsTerminal.js` を読んで確認）:
  - `kill(signal)` は `signal` を渡すと `Signals not supported on windows.` を投げる。`Pane.close()` は `process.kill(-pid, 'SIGHUP')` → 失敗で `process.kill(pid)`（Windows では強制終了、ツリーは対象外）の経路になる。Windows では `proc.kill()`（ConPTY のコンソールにつながるプロセスを終了）を使う分岐が要る。**この PR では未実装**。実機では、現状の `close()` は閉じるまで約 4.7 秒かかり、シェルの子プロセス（孫）が残った。`proc.kill()` なら孫まで終了した（§10.1 V4）。
  - `encoding: null` は `opt.encoding` が偽なので警告は出ない。node-pty は Windows で出力ソケットに `setEncoding('utf8')` する（`windowsPtyAgent.js`）ので、出力は**常に文字列**で届く。`pane.ts` はその文字列を `Buffer.from(d)` で Buffer に変換して扱う。実機で確かめるのは、無効な UTF-8 や VT の再合成でバイト情報が失われないか（V11 で採取した生バイトと比べる）だけ。
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
| V1 | 既定の DACL に Everyone の read が付くか、および read だけで接続したときに何が読めるか（`server.info` やイベントが流れるか） | `node packages/cli/dist/main.js serve` を起動し、別のシェルで `$p=[IO.Pipes.NamedPipeClientStream]::new('.','misao-<解決された名前>','InOut'); $p.Connect(); $p.GetAccessControl().Access` | Everyone に read が付けば A は不十分（その場合、read だけで開いた接続で `server.info` の応答やイベントが読めるかも確かめる）。許可が本人・SYSTEM・管理者だけなら A で足りる |
| V2 | 別ユーザー・別ホストからの接続 | 標準ユーザーを別に作り、`runas` で同じ手順の接続を試す。別ホストからは `\\<ホスト名>\pipe\<パイプ名>` を開く（ファイアウォールと SMB の設定が要る） | 別ユーザーは拒否されること。別ホストからの接続は、拒否されることを期待せず結果を記録する |
| V3 | 二重起動と stale | `serve` を 2 つ起動し、片方を `Stop-Process -Force` して再起動 | 二重起動は失敗し、強制終了後は再起動できる |
| V4 | ConPTY のペイン | `misao new -- cmd.exe`、`misao send`、`misao screen`、`misao attach`、resize、`exit` | 入力・出力・resize・終了検知（`pane.state` が exited）が動く。`pane.close` で子が残らない |
| V5 | `server.info` / `pane.state` | `misao status`、`misao events` で idle/working の遷移を観察 | 何もしない間に working にならない（再描画の誤判定が無い） |
| V6 | CI のネイティブ導入 | PR の `Windows (experimental)` ジョブのログ | `npm ci` が成功し、node-pty の ConPTY スモークが `conpty-ok` を出す |
| V7 | 単体テスト | 同ジョブで `npm test` | 落ちたテストの一覧を Phase 2 の `skip` / 修正の対象にする |
| V8 | コマンド解決 | `misao new -- claude`（npm shim）、`-- powershell.exe`、`-- pwsh.exe` | shim が失敗するか確認。必要なら `cmd.exe /c` で包む |
| V9 | 前面コマンド | `pane.info` の前面コマンドが空になること、`proc.process` の値 | 端末名が漏れていない |
| V10 | AF_UNIX | `node -e "require('net').createServer().listen('C:\\\\Temp\\\\a.sock')"` | 失敗すれば E 案は不可 |
| V11 | 生バイトの採取 | cmd / PowerShell / claude の出力をファイルへ保存 | §7 のフィクスチャにする |

### 10.1 実機確認の結果（2026-10-17、Windows 11 25H2 ビルド 10.0.26200.9168、Node v24.22.0）

- 環境: Windows 11（ロケールは日本語、コンソールのコードページは 932）、Windows PowerShell 5.1.26100.9168、ユーザー `ServerUser` のプロファイル配下（管理者権限は使っていない）。PowerShell 7（`pwsh.exe`）、`claude`、`git` は無い。
- 手順: Node は公式のポータブル版 `node-v24.22.0-win-x64.zip`（`SHASUMS256.txt` の SHA-256 `f9683a19…f80c3` と一致を確認）を `%USERPROFILE%\misao-win-test\node` に展開し、PATH はプロセス内でのみ変更した。このブランチの作業ツリー（node_modules を除く）を `…\misao-win-test\misao` に置いて `npm ci`（npm 11.19.1）を実行した。インストーラ・レジストリ・サービス・管理者権限は使っていない。
- `npm ci` は成功したが、npm 11 は `install-scripts`（esbuild の postinstall、node-pty の install）を「未承認」として実行しなかった。それでも node-pty は同梱の `prebuilds/win32-x64` で動いた（コンパイラ不要）。

| ID | 結果 | 実測 |
| --- | --- | --- |
| V1 | 成功（A は不十分） | パイプ `\\.\pipe\misao-ServerUser-72bd07d30c339cae` の DACL は下記。`In` だけ・`Out` だけで開いた接続も成功した。read だけで接続して待ち、その間にペインを作ったが、5 秒間 1 バイトも届かなかった（イベントは購読した接続にだけ流れる）。書き込み権（`FILE_WRITE_DATA`）が Everyone には無いので、他ユーザーは要求を送れない見込み（他ユーザーでの実測は V2 の手作業） |
| V2 | 一部のみ | 別ユーザー・別ホストは未実行（ユーザーの手作業）。代わりに同じユーザーで SMB 経由の UNC パスを試したところ、`\\.\pipe\…` と同じく `\\127.0.0.1\pipe\…` と `\\WIN-SERVER-001\pipe\…` から接続でき、`server.info` の応答まで得られた。パイプは SMB（LanmanServer）経由で到達できる |
| V3 | 成功 | 同じ `MISAO_DIR` で 2 つ目の `serve`: 終了コード 1、`another daemon (pid 22920) holds …\daemon.pid`。別の `MISAO_DIR` で同じ `MISAO_SOCKET`: 終了コード 1、`another daemon is already listening on \\.\pipe\…`（`EADDRINUSE` を検出）。素の `net` で同名を listen しても `EADDRINUSE`。`Stop-Process -Force` で落とすとパイプは消え、pid ファイルは残り、再起動すると pid が置き換わった。ペインは `persistence.json` から `stopped` で復元された。強制終了ではペインの子（cmd.exe、powershell.exe）も孫（ping.exe）も一緒に終了した（ConPTY の拠点が閉じるため） |
| V4 | 一部失敗 | `new`・`send`・`screen`・resize・終了検知は動いた。`attach` は ConPTY 内で起動した CLI から入力・resize（100x30 → 120x40）・`Ctrl-^ d` での detach まで動いた。`exit /b 7` は `pane.exited` の `exitCode` が 7、`signal` が `null`。日本語の入出力（コードページ 932）も正しい。**`kill --force`（`pane.close`）は閉じるまで約 4.7 秒かかり、cmd.exe は終了するが孫プロセス（`ping`）が残った**（powershell.exe でも同じ）。孫は記録した PID で止めた。`pty.kill()` を直接呼ぶと、cmd.exe と孫が両方終了し、`exit` イベントもすぐ届いた（`conpty_console_list_agent.js` が `AttachConsole failed` を stderr に出すが、終了は妨げない） |
| V5 | 成功（誤判定なし） | cmd.exe と powershell.exe のペインを 25 秒放置、続けて attach から 5 回 resize（100x30、120x40、80x24、132x50、90x28 を 2.5 秒間隔）、さらに 15 秒放置: `pane.state` は `idle`（`decidedBy: bytes`）のまま一度も変わらなかった。resize のたびに画面全体の再描画（80x24 で約 2KB、132x50 で数 KB）が入るが、`byte-activity` の resize 後 800 ms の猶予に収まった。採取したフィクスチャで猶予を外して再生しても、idle のまま working にはならなかった。なお、`ping -n 14` の可視出力（1 秒に 1 行）では working にならなかった（閾値は 3 秒で 200 バイト）。`dir /s` のような短い一気出力は猶予（入力後 500 ms）の内に終わるため working にならない。いずれも Windows 固有の問題ではない |
| V6 | 同等を確認 | CI のログは未確認。同じ内容のスモークを実機で実行: `conpty-ok` が出て終了コード 0（出力は下記） |
| V7 | 失敗あり | 下記の一覧。protocol は 56/56 成功。sdk-node は 125 件中 77 件、daemon は 217 件中 82 件（skip 3）、cli は 137 件中 43 件失敗・11 件キャンセル（`serve` のテストが SIGINT / SIGTERM で終了しない）。**失敗はすべてテスト側の前提**（実装の不具合ではない）。e2e は未実行。`daemon` の `connection.test.ts` と `stream.test.ts` は、`.sock` のパスで待ち受けて失敗したあとデーモンを閉じず、テストランナーが止まらない（`--test-timeout` を付けると完走する） |
| V8 | 一部失敗 | npm の shim `npm.cmd` で代用（この機に `claude.cmd` は無い）。**拡張子なし（`npm`、`node`）は失敗**（`spawn failed: Cannot create process, error code: 2` / `File not found: `。後者はファイル名が空で出る）。`npm.cmd`、`C:\…\npm.cmd`（フルパス）、`node.exe`、`powershell.exe`、`cmd.exe /c npm --version` は成功。`npm.ps1` は `error code: 193`（Win32 アプリケーションではない）。`pwsh.exe` は `File not found`。つまり AZITO が `cmd: ["claude"]` を送ると失敗し、`claude.cmd` と書くか `cmd.exe /c` で包む必要がある |
| V9 | 成功 | `proc.process` は `"xterm-256color"`（端末名）。`pane.info`（`ls --json`）には前面コマンドの項目が無く、端末名は漏れない。`ls` の AGENT 列は `cmd.exe`（`cmd` の先頭から作られる表示） |
| V10 | 成功（E 案は不可） | `net.createServer().listen('<profile>\\…\\a.sock')` は `EACCES: permission denied` |
| V11 | 成功 | 6 シナリオの生バイトを採取し、`packages/daemon/test/fixtures/windows/` に保存（下記） |

**V1 の DACL（SDDL と展開）**

```
O:S-1-5-21-…-1001G:S-1-5-21-…-513D:(A;;FR;;;WD)(A;;FR;;;AN)(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;S-1-5-21-…-1001)
Everyone                      Allow  Read, Synchronize
NT AUTHORITY\ANONYMOUS LOGON  Allow  Read, Synchronize
NT AUTHORITY\SYSTEM           Allow  FullControl (2032127)
BUILTIN\Administrators        Allow  FullControl (2032127)
win-server-001\ServerUser     Allow  FullControl (2032127)   ← 起動したユーザー
```

**ACL の結論**: A（既定の DACL）は「起動したユーザーのみ」ではなく、不十分。ただし実害は想定より小さい（Everyone / 匿名は read だけで、要求を送れず、受け取れるものも無い）。それでも、(1) 誰でも接続して接続数を占有できる、(2) SMB 経由で別ホストから届く、(3) パイプ名の先取りを防げない、ので **D 案（`%LOCALAPPDATA%\misao` の token を接続直後に送らせる）を推奨**する。B 案（ネイティブアドオン）は、要件が「他ユーザーを接続させない」まで上がった場合のみ。E 案は不可。V2 の別ユーザー・別ホストの実測でこの判断を確認する。

**V6 の出力**

```
"\u001b[?9001h\u001b[?1004h\u001b[?25l\u001b[2J\u001b[m\u001b[Hconpty-ok\r\n\u001b]0;C:\\WINDOWS\\SYSTEM32\\cmd.exe\u0007\u001b[?25h"
```

**V7 の失敗の内訳（すべてテスト側）**

| 原因 | 該当 |
| --- | --- |
| テストのヘルパーが一時ディレクトリの `*.sock` で待ち受ける（Windows ではパイプ名が必要）。`Daemon.start()` が `on Windows the daemon endpoint must be a named pipe` で失敗 | daemon: `daemon.test.ts`（42）、`daemon-limits.test.ts`（6）、`connection.test.ts`（7）、`stream.test.ts`（11）。cli: `test/helpers/daemon.ts` 経由の attach / loop / コマンド系（約 29）。sdk-node: `listen EACCES …fake.sock` の `client*.test.ts`、`rpc-connection.test.ts` |
| POSIX の権限・所有者の検査（`not owned by the current user`、`mode 0600` が 438 = 0666、`ENOTSOCK` の判定、`EACCES` の期待） | daemon: `socket.test.ts`（10）、`endpoint.test.ts` の Unix 側、`persistence.test.ts`（1） |
| シンボリックリンクの作成が `EPERM`（開発者モード / 権限が無い標準ユーザー） | `socket.test.ts`、`endpoint.test.ts` |
| パスの前提（`/bin/bash`、`~/`、`/` 区切り）、`readLoginShell` の期待値 | cli: `cli-io.test.ts`、`config` 系（約 8）、`serve` の `buildDaemonOptions` |
| SIGINT / SIGTERM で優雅に終了する前提（Windows の Node は受けられない、§5） | cli: `serve` のテスト 2 件（5 秒でタイムアウト） |
| sdk-node の `socket-path.test.ts`（9） | 失敗の理由は未精査（パス形式の期待値と見られる） |

**V11 のフィクスチャ**（`packages/daemon/test/fixtures/windows/`、1 行 1 JSON。先頭行が `meta`、以降は `{t, d}`（node-pty の `onData` が渡した文字列と、spawn からの経過 ms）、`{t, input}`、`{t, resize}`、`{t, exit}`）

| ファイル | 内容 |
| --- | --- |
| `conpty-cmd-basic.jsonl` | cmd.exe の起動・`echo`・`dir`・日本語・`cls`・resize・`exit /b 5` |
| `conpty-cmd-idle-resize.jsonl` | cmd.exe を放置したまま resize を 4 回 |
| `conpty-ps-basic.jsonl` | powershell.exe の起動・`Get-Date`・複数行出力・日本語・resize・`exit` |
| `conpty-ps-idle-resize.jsonl` | powershell.exe を放置したまま resize を 4 回 |
| `conpty-cmd-flood.jsonl` | `dir /s` の大量出力（約 105 KB）と Ctrl-C |
| `conpty-fake-agent.jsonl` | **合成**（`claude` はこの機に無い）。タイトルのグリフ（`✳` / `⠂`）を OSC 0 で更新しスピナーを再描画するスクリプト。実物の claude の採取ではない |

観察できたこと（§7 の仮説の確認）:

- 起動直後: `ESC[?9001h ESC[?1004h`、`ESC[?25l ESC[2J ESC[m ESC[H`、画面の内容、`OSC 0;<exe のパス> BEL`、`ESC[?25h`。cmd は実行中のコマンドに合わせて `OSC 0;…cmd.exe - ping …` のようにタイトルを更新し、終わると戻す。
- 改行は `\r\n`。表示が進むときは `ESC[<行>;<桁>H` で位置を指定して書く（例: `\x1b[7;1HC:\…>`）。
- **resize のたびに、画面全体を `ESC[8;<行>;<桁>t ESC[H` から `ESC[K\r\n` の連なりで再描画する**。`AnsiLineAssembler` に通すと、cmd の放置 + resize 4 回で、同じ行（バナー 2 行とプロンプト）が 4 回ずつ現れ、空行が約 110 行流れた（powershell は プロンプトが 3 回）。**行ストリームの重複は仮説どおり起きる**。
- 日本語は文字列としてそのまま届く。ただし cmd は、タイトルの更新 OSC やエコーを文字ごとの細かい chunk に分けて送ることがある（`echo 日本語` が数 chunk）。
- 終了時、powershell は `ESC[?25l`、`ESC[92mexit\r\n`、`ESC[?25h` を出して `exit` イベントが続く。

**この結果で食い違っていた本文**: §2.2 の A（「不十分の見込み」→ 確定）、E（「見込み」→ 不可）、二重起動（「見込み」→ 確認済み）、§3 と §6 の `proc.process`・`close()` の挙動を上のとおり直した。そのほか、CI の「node-pty が `npm ci` で導入できる」は、npm 11 の `install-scripts` が未承認で実行されなくても prebuild で動くことを実機で確認した。

**実機確認で見つかった、次の段で直すコード**

1. `Pane.close()` の win32 分岐: `process.kill(-pid, 'SIGHUP')` ではなく `proc.kill()` を使う（孫が残り、閉じるまで約 4.7 秒かかる）。
2. テストのヘルパー（`daemon/test/helpers`、`cli/test/helpers/daemon.ts`、sdk-node の `fake.sock` など）が一時ディレクトリの `*.sock` ではなく、`resolveSocketPath` 由来のパイプ名を使う。
3. POSIX 前提のテストの `skip`（理由付き）または Windows 用の期待値: `socket.test.ts`、`endpoint.test.ts`（Unix 側）、`persistence.test.ts`（mode）、`cli-io.test.ts`、`config` 系、`serve` の SIGINT / SIGTERM、シンボリックリンク（`EPERM`）。
4. `connection.test.ts` / `stream.test.ts` が `Daemon.start()` 失敗後に止まらない（`after` で `daemon` が `undefined` のときの後始末、または `--test-timeout`）。
5. コマンド解決: 拡張子なしの名前（`claude`）を `PATHEXT` で解決する、または `cmd.exe /c` で包む。`spawn failed: File not found: ` はファイル名が空なので、名前を含めたメッセージにする。`.ps1` は `powershell.exe -File` 経由にする。
6. 停止 RPC / `misao stop`（`serve` のテストが示すとおり、Windows では SIGINT / SIGTERM で優雅に止まらない）。
7. アプリ層の token 認証（D 案、V1 の結論）。
8. 再描画への対処: resize の再描画で行ストリームに重複・空行が入る。猶予が効くので `byte-activity` は変えなくてよいが、`AnsiLineAssembler` 以降（`tail` / `pane.line`）に Windows 用の重複除去が要る。`scrollback` / `screen` の挙動は別途確認する。
9. `CI` の `Windows (experimental)` ジョブ: `--test-timeout` を付ける（止まるテストで上限の 12 分まで待たされる）。

**ユーザーの手作業が必要な項目**: V2 の「別の標準ユーザーからの接続」と「別ホストからの接続」。手順: (1) `serve` を起動したまま、標準ユーザーを別に作り、`runas /user:<別ユーザー>` で `MISAO_SOCKET=\\.\pipe\<名前>` を指定して `node packages\cli\dist\main.js status` を実行し、接続できても応答が得られないこと（書き込み拒否）を確認する。(2) 別ホストから `\\<ホスト名>\pipe\<名前>` を開く（ファイアウォールと SMB の許可が要る）。結果が想定どおりなら D 案を採る。

## 11. 次の段（提案）

1. V1〜V4 の結果で ACL の方針（D / B）を決める。
2. `Pane.close()` の win32 分岐、停止 RPC、`misao stop`。
3. 再描画のフィクスチャ化と稼働判定の調整。
4. 前面プロセスの推定、`shell` 設定、タスクスケジューラの定義。
