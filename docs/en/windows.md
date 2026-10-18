# Native Windows support: investigation and prototype

First stage of issue #37. It records what was found by reading the code and the node-pty 1.1.0 package, and describes the prototype that can be verified on Linux.
At the time of the investigation nothing had been run on a Windows host. Statements marked "verify on host" were hypotheses; they were checked with the list in section 10 and the results are recorded in section 10.1 (the body has been updated where the host showed otherwise).

Effort: S = up to half a day, M = 1-2 days, L = 3+ days.

## 1. Summary

| Area | Today | Windows approach | Prototype (this PR) | Effort | Main risk |
| --- | --- | --- | --- | --- | --- |
| Endpoint | Unix socket only | Named pipe `\\.\pipe\misao-<user>-<hash>`; Node's `net` handles it natively | Abstraction layer and resolution implemented | S | Pipe-name squatting; names are limited to 256 characters |
| Endpoint protection | Socket 600 + parent dir 700 + uid check | ACL limited to the launching user. **Node alone cannot set one explicitly** (section 2) | Relies on the default DACL and logs a warning | M-L | Other users may be able to connect (verify on host) |
| Foreground process | `/proc` on Linux, node-pty `process` elsewhere | ConPTY has no foreground process group; infer from the console process list | `undefined` on win32 (explicitly unsupported) | M | The existing non-Linux path would report the terminal name on Windows |
| Login shell | `os.userInfo().shell` + `-l` | Default to `%COMSPEC%` (cmd.exe) without `-l`; make PowerShell selectable by config | `%COMSPEC%` default implemented | S | `.cmd` / `.ps1` shims cannot be spawned directly |
| Stop / restart | SIGTERM/SIGINT/SIGHUP, systemd / launchd | Graceful stop via RPC; supervise with Task Scheduler (at logon) | Not started | M | Node on Windows cannot receive SIGTERM |
| node-pty / ConPTY | Used on Linux / macOS | 1.1.0 ships win32 prebuilds; use ConPTY | CI verifies the native install | S (install) + M (behavior) | Child termination, `encoding`, command resolution |
| CRLF, redraw, activity detection | PTY output is used as-is | ConPTY re-synthesizes output as VT; capture and turn into regression tests | Not started | M-L | Redraws disturb activity detection and the line stream |
| Config / state files | POSIX assumptions (700, rename) | Only ensure `dataDir`; rely on the profile's inherited ACL | pid / state location separated | S | rename blocked by AV (EPERM/EBUSY) |

## 2. Endpoint and ACL

### 2.1 Endpoint abstraction (implemented)

- `@misao/protocol`: `NAMED_PIPE_PREFIX`, `isNamedPipePath()`, the pipe-name limit (256).
- `@misao/sdk`: `resolveSocketPath({ platform, userName })`. For `platform === 'win32'` it returns a pipe name.
  - Precedence matches Unix (`$MISAO_SOCKET` > `socket` in misao.json > `$MISAO_DIR` > `~/.misao`).
  - The default name is `\\.\pipe\misao-<user>-<sha256(lowercased dir)[0:16]>`. Characters outside `[A-Za-z0-9._-]` in the user name become `_`, and it is cut to 32 characters.
  - A Unix path on win32 raises `MisaoPathError` (it never silently creates something elsewhere). The 107-byte limit still applies on Unix only.
- `@misao/daemon` `endpoint.ts`: `prepareEndpoint` / `removeStaleEndpoint` / `listenEndpoint` / `endpointFiles`. `Daemon.start()` goes through them.
  - On Unix they delegate to the existing `socket.ts`; behavior is unchanged.
  - A pipe listens with `listen({ path, readableAll: false, writableAll: false })`. `listenEndpoint` returns the protection mechanism (`posix-mode` / `default-dacl`) and the daemon logs a warning for `default-dacl`.
  - A pipe has no parent directory, so the pid and state files go to `dataDir` (default: the misao directory). `misao serve` resolves it at the boundary, and `buildDaemonOptions` fails when a pipe is used without `dataDir`.
  - The OS removes a pipe when its creating process exits, so there is no stale cleanup and no file to delete. A successful connect means "another daemon is running" and startup fails.

### 2.2 Restricting the ACL to the launching user: comparison

The only options `net.Server.listen` passes for a named pipe are `readableAll` / `writableAll`, and both call libuv's `uv_pipe_chmod`, which can only **widen** access. Node has no way to pass `SECURITY_ATTRIBUTES`, so **Node alone cannot set an explicit DACL** (verify on host: this is from reading libuv and the Microsoft documentation).

| Option | What | Extra dependency | Strength | Effort | Notes |
| --- | --- | --- | --- | --- | --- |
| A. Rely on the default DACL (current prototype) | The DACL from a `NULL` SECURITY_ATTRIBUTES. **Confirmed on a host**: full control for SYSTEM / Administrators / the creator, read for Everyone and ANONYMOUS LOGON (section 10.1 V1) | None | Weak. A read-only connection receives nothing and, lacking write access, cannot send requests (verified), but anyone can connect and the pipe is reachable over SMB (another host) (section 10.1 V2) | S | Not "only the launching user". Does nothing against squatting |
| B. Create the pipe in a native addon | `CreateNamedPipeW` with SDDL `D:P(A;;GA;;;<user SID>)`, bridged to Node as a Duplex | **Needs approval** (N-API addon or an ffi library, plus prebuild distribution) | Strong (with `FILE_FLAG_FIRST_PIPE_INSTANCE` also resists squatting) | L | An existing handle cannot be given to a libuv pipe handle, so the whole server has to live in the addon |
| C. Rewrite the DACL after listen | `SetNamedSecurityInfo`-style call from outside | No way found in PowerShell / .NET to change the DACL of a pipe another process created; `icacls` cannot address pipes (verify on host) | - | - | Not realistic |
| D. Authenticate at the application layer | A token file in `%LOCALAPPDATA%\misao` (user-only via the profile ACL) sent right after connecting | None (add an auth exchange to the protocol) | Medium: anyone can connect but cannot do anything. Within one user there is no boundary, as on Unix | M | Cheap to add while the protocol is 0.x |
| E. Use AF_UNIX | Windows 10 1803+ has AF_UNIX, giving the same "parent directory ACL" model as Unix | None | Strong | - | **Not possible (verified on a host)**: `listen` on a `.sock` path in Node 24 fails with `EACCES` (section 10.1 V10) |
| F. Loopback TCP + token | Listen on loopback, authenticate with a token | None | Medium (other users can still connect) | M | The endpoint becomes TCP, changing assumptions in the SDK and AZITO |

**Recommendation**: first check the real ACL of option A on a host (item V1). Then D is the first candidate (Node only), and B the second candidate if a full ACL becomes a requirement. B adds a dependency and needs approval.

Other measures:

- **Client-side squatting**: if another user creates a pipe with the same name first, the SDK connects to it. Node cannot check the server process ID (`GetNamedPipeServerProcessId`). The token in D also covers this (a server without the token cannot answer). On the server side libuv sets `FILE_FLAG_FIRST_PIPE_INSTANCE`, so creating a second pipe with the same name gets `EADDRINUSE` (verified on a host, section 10.1 V3).
- **Remote clients**: libuv does not pass `PIPE_REJECT_REMOTE_CLIENTS` to `CreateNamedPipeW` (only `FILE_FLAG_FIRST_PIPE_INSTANCE` is set). **Do not expect remote connections to be refused.** On a host we connected through `\\127.0.0.1\pipe\<name>` and `\\<hostname>\pipe\<name>` (over SMB) and got a JSON-RPC response (section 10.1 V2; a real connection from another host was not tried). Blocking it needs an ACL (B) or authentication (D).
- **dataDir**: under `%USERPROFILE%` it inherits the profile's default ACL (user / SYSTEM / administrators). The equivalent of the Unix "700 + uid" check is not implemented because Node cannot read ACLs; only symlink and directory checks are done.

## 3. Foreground process

- Existing problem found while reading the code: `foreground.ts` uses node-pty's `process` on non-Linux platforms, but `WindowsTerminal.process` in node-pty 1.1.0 returns `opt.name` (misao passes `xterm-256color`). On Windows the **terminal name would have been reported as the foreground command** instead of a program name (verified on a host: `proc.process === "xterm-256color"`, section 10.1 V9).
- Prototype: `readForegroundCommand(pid, ptyProcess, platform)` always returns `undefined` on win32 (the foreground command is omitted from `pane.info`). Whether the activity profile selection depends on it is to be checked in phase 2.
- Implementation ideas:
  1. Use the process-ID list that node-pty's `conpty_console_list` (`WindowsPtyAgent._getConsoleProcessList`) returns for the console, and treat the last entry (most recently attached) as the foreground. It is internal API: mimic the bundled `conpty_console_list_agent.js`, or propose exposing it upstream.
  2. Resolving the name needs `QueryFullProcessImageNameW`, which Node lacks. `Get-CimInstance Win32_Process` or `tasklist` takes 100 ms or more per call, which suits neither polling nor hot paths. Call it only when `pane.info` is requested and cache the result.
  3. Precision needs the same native dependency as B (needs approval).
- Effort: M. Omitting the value is acceptable meanwhile, since it is display-only.

## 4. Login shell

- Prototype: on win32 `readLoginShell` ignores `os.userInfo()` and returns `%COMSPEC%` (normally `C:\Windows\System32\cmd.exe`), or `null` when unset (the usual usage error asking for `-- cmd`). `loginShell()` does not add `-l` on win32 (cmd.exe rejects it).
- Candidates: `pwsh.exe` (PowerShell 7) > `powershell.exe` (5.1) > `cmd.exe`. PATH probing varies by machine and hurts reproducibility, so the default is pinned to `%COMSPEC%`, with a `shell` setting in `misao.json` added later to choose another.
- Caveat: npm shims (`claude.cmd`, `codex.cmd`) and `.ps1` cannot be handed to ConPTY's `CreateProcess` directly (an executable with extension or a `cmd.exe /c` wrapper is required). AZITO sending `cmd: ["claude"]` may fail (verify on host). Fix by wrapping with `cmd.exe /c` in AZITO's launch command, or by resolving `PATHEXT` in misao.
- Effort: S.

## 5. Stop and restart

- Signals: Node on Windows cannot handle `SIGTERM` (`process.kill(pid, 'SIGTERM')` terminates unconditionally). It can receive Ctrl+C (SIGINT), Ctrl+Break (SIGBREAK), and console close (SIGHUP, force-killed after about 10 s). The `SIGINT/SIGTERM/SIGHUP` registration in `serve.ts` is harmless on Windows but is **not an external graceful-stop mechanism**.
  - Plan: add a graceful-stop RPC (for example `server.shutdown`, a protocol addition) and `misao stop`. When the daemon is force-killed the `shutdown()` in `finally` (close panes, save records, delete the pid file) does not run; the stale pid file is replaced at the next start by the liveness check.
  - `SIGWINCH` in `attach` does not exist on Windows; use `process.stdout.on('resize')`.
- Supervision:
  | Method | Pros | Cons |
  | --- | --- | --- |
  | Task Scheduler (at logon, user rights) | Runs in the user session; no dependency; definable via `schtasks` / XML, with restart-on-failure | Hidden-window handling (`windowsHide`); does not run while logged off |
  | Windows service | No logon needed | Node services need an external binary or dependency (WinSW / NSSM); Session 0 has a different user environment (PATH, profile) |
  Task Scheduler is recommended. Add an XML and install steps under `deploy/` (`RestartOnFailure` corresponds to systemd's `Restart=on-failure`).
- pid file: `process.kill(pid, 0)` works as an existence check on Windows.
- Effort: M (RPC + `misao stop` + task definition).

## 6. node-pty and ConPTY

Checked from the bundled files of node-pty 1.1.0 (a dependency of `packages/daemon`):

- **ConPTY or winpty**: when `useConpty` is unset, `WindowsPtyAgent` uses ConPTY on Windows build 18309 or later and winpty (`winpty-agent.exe`) below that. misao does not set it, so it is auto-selected: ConPTY on Windows 10 1903 (build 18362) and later, winpty on 1809 (build 17763).
- **Build**: `install` runs `node scripts/prebuild.js || node-gyp rebuild`. `prebuilds/` contains `win32-x64`, `win32-arm64`, `darwin-arm64` and `darwin-x64` (`conpty.node`, `conpty_console_list.node`, `pty.node` (winpty), `winpty-agent.exe`, `winpty.dll`), so Windows is expected to use a prebuild without a compiler. There is no Linux prebuild; on Linux it is built with `node-gyp rebuild`. CI verifies this (V6).
- **spawn-helper**: Unix only (a helper binary used by `UnixTerminal` at `fork`, shipped in the macOS prebuilds). It is not used on Windows and needs nothing.
- **conpty.dll**: `useConptyDll` (default false) can switch to the bundled OpenConsole-based DLL (`third_party/conpty/1.23.251008001`). The default is the OS ConPTY. A candidate if old OS versions or the redraw differences below become a problem.
- **API differences** (read from `lib/windowsTerminal.js`):
  - `kill(signal)` throws `Signals not supported on windows.` when a signal is given. `Pane.close()` goes `process.kill(-pid, 'SIGHUP')` then, on failure, `process.kill(pid)` (a forced kill of that process only, not the tree). A win32 branch using `proc.kill()` (which ends the processes attached to the ConPTY console) is needed. **Not implemented in this PR**. On a host the current `close()` takes about 4.7 s and leaves the shell's child processes (grandchildren) running; `proc.kill()` ended the grandchildren too (section 10.1 V4).
  - `encoding: null` is falsy, so no warning is printed. node-pty calls `setEncoding('utf8')` on the output socket on Windows (`windowsPtyAgent.js`), so output **always arrives as a string**; `pane.ts` converts it with `Buffer.from(d)`. The only thing to verify on a host is whether invalid UTF-8 or VT re-synthesis loses byte information (compare with the raw bytes captured in V11).
  - `resize` after exit throws (`Cannot resize a pty that has already exited`). Check that `pane.resize` does not throw on an exit race.
  - `proc.process` is the terminal name (section 3).
- Effort: install S, absorbing behavior M.

## 7. CRLF, redraw, and activity detection

ConPTY does not pass the child's output through; it re-synthesizes VT sequences from its own screen buffer (verify on host; Windows 11 22H2+ passthrough and OS versions change this). Expected effects:

- A screen clear (`ESC[2J` `ESC[H`), cursor show/hide (`ESC[?25l/h`) and a window title (OSC 0) right after start.
- Newlines are `\r\n`. Lines may be updated by erase + rewrite instead of append, and the whole screen is repainted on every resize.
- Affected misao parts:
  - `AnsiLineAssembler` (line stream): repaints may **emit the same line twice**.
  - `byte-activity`: repaints while idle (resize, cursor-blink sync) look like output and may cause **false "working"**.
  - `title-activity`: whether the title glyph set by Claude Code survives ConPTY, and whether it mixes with ConPTY's own title.
  - Screen reconstruction (xterm-headless): the repainted result should converge to the same screen, so snapshots are expected to be little affected.
- Plan: capture raw bytes for representative scenarios on a Windows host (cmd, PowerShell, claude start / waiting / running, resize, exit), keep them as fixtures, and add regression tests for `ansi` / `byte-activity` / `title-activity` / `screen`. If misdetection appears, tune the byte-activity threshold on Windows or de-duplicate the line stream.
- Effort: M-L (depends on what is observed).

## 8. Other POSIX assumptions

- `persistence.ts`: writes a temp file, `fsync`, then `renameSync`. On Windows `EPERM` / `EBUSY` can happen when the destination is open (AV, editors). Consider a retry (V7).
- `scripts/fix-bin.mjs`: `chmodSync(..., 0o755)` is a no-op on Windows. The bin is `main.js`; npm creates the Windows shim.
- `scripts/smoke-bundle.mjs` uses `/bin/sh`, so it does not run on Windows (release artifacts are out of scope).
- Tests: unit tests that assume POSIX permissions (`chmod`, `getuid`, `/proc`) will fail on Windows. The experimental CI job lists them; phase 2 either skips them with a reason or writes Windows expectations.

## 9. What this PR changes and does not change

- Changed: the endpoint resolution and listening layer, the win32 branches for the foreground process and login shell, `serve` dataDir resolution, and an experimental CI job. Linux / macOS behavior is unchanged (all existing tests pass).
- Not changed (phase 2 and later): the win32 branch in `Pane.close()`, the ACL implementation, the stop RPC, the supervision definition, foreground inference, absorbing redraws.

## 10. Items to verify on a Windows host

Prerequisite: Node.js 24+ (with user approval if a portable build is used). Use PowerShell and only temporary directories. Run `npm ci && npm run build` in the repository first.

| ID | Purpose | Steps | Expected / decision |
| --- | --- | --- | --- |
| V1 | Whether the default DACL gives Everyone read access, and what a read-only connection can read (do `server.info` responses or events flow?) | Start `node packages/cli/dist/main.js serve`; in another shell run `$p=[IO.Pipes.NamedPipeClientStream]::new('.','misao-<resolved name>','InOut'); $p.Connect(); $p.GetAccessControl().Access` | If Everyone has read access, A is insufficient (then also check whether a connection opened read-only receives `server.info` responses or events). If only the user, SYSTEM and administrators have access, A is enough |
| V2 | Connection from another user and another host | Create a standard user and try the same connection via `runas`. From another host, open `\\<hostname>\pipe\<pipe name>` (needs firewall and SMB settings) | Another user must be refused. Do not expect another host to be refused; record the result |
| V3 | Double start and stale | Start two `serve` instances; `Stop-Process -Force` one and restart | The second start fails; restart works after a forced kill |
| V4 | ConPTY pane | `misao new -- cmd.exe`, `misao send`, `misao screen`, `misao attach`, resize, `exit` | Input, output, resize and exit detection (`pane.state` becomes exited) work; `pane.close` leaves no child |
| V5 | `server.info` / `pane.state` | Watch idle/working transitions with `misao status` and `misao events` | No "working" while idle (no redraw misdetection) |
| V6 | CI native install | Log of the `Windows (experimental)` job in the PR | `npm ci` succeeds and the node-pty ConPTY smoke prints `conpty-ok` |
| V7 | Unit tests | `npm test` in the same job | The failing tests become the phase-2 skip / fix list |
| V8 | Command resolution | `misao new -- claude` (npm shim), `-- powershell.exe`, `-- pwsh.exe` | Confirm that shims fail; wrap with `cmd.exe /c` if needed |
| V9 | Foreground command | The foreground command in `pane.info` is empty; the value of `proc.process` | The terminal name does not leak |
| V10 | AF_UNIX | `node -e "require('net').createServer().listen('C:\\\\Temp\\\\a.sock')"` | If it fails, option E is out |
| V11 | Raw bytes | Save cmd / PowerShell / claude output to files | Use as the fixtures for section 7 |

### 10.1 Results on a Windows host (2026-10-17, Windows 11 25H2 build 10.0.26200.9168, Node v24.22.0)

- Environment: Windows 11 (Japanese locale, console code page 932), Windows PowerShell 5.1.26100.9168, under the profile of user `ServerUser` (no elevation used). There is no PowerShell 7 (`pwsh.exe`), `claude` or `git`.
- Procedure: the official portable `node-v24.22.0-win-x64.zip` (SHA-256 `f9683a19...f80c3` matched `SHASUMS256.txt`) was unpacked to `%USERPROFILE%\misao-win-test\node`, with PATH changed only inside the process. This branch's working tree (without node_modules) was placed in `...\misao-win-test\misao` and `npm ci` (npm 11.19.1) was run. No installer, registry, service or administrator rights were used.
- `npm ci` succeeded, but npm 11 did not run the `install-scripts` (esbuild postinstall, node-pty install) because they were not approved. node-pty still worked from its bundled `prebuilds/win32-x64` (no compiler needed).

| ID | Result | Measured |
| --- | --- | --- |
| V1 | Done (A is insufficient) | The DACL of pipe `\\.\pipe\misao-ServerUser-72bd07d30c339cae` is shown below. Connections opened `In`-only and `Out`-only both succeeded. A read-only connection that waited while a pane was created received 0 bytes in 5 s (events only flow to subscribed connections). Everyone has no write access (`FILE_WRITE_DATA`), so another user should not be able to send requests (not measured with another user; manual step under V2) |
| V2 | Partial | Another user and another host were not run (manual steps for the user). Instead, with the same user over SMB, `\\127.0.0.1\pipe\...` and `\\WIN-SERVER-001\pipe\...` connected just like `\\.\pipe\...` and returned the `server.info` response. The pipe is reachable through SMB (LanmanServer) |
| V3 | Done | Second `serve` with the same `MISAO_DIR`: exit code 1, `another daemon (pid 22920) holds ...\daemon.pid`. Different `MISAO_DIR` but the same `MISAO_SOCKET`: exit code 1, `another daemon is already listening on \\.\pipe\...` (`EADDRINUSE` detected). A plain `net` server on the same name also gets `EADDRINUSE`. After `Stop-Process -Force` the pipe disappeared, the pid file stayed, and a restart replaced the pid. Panes were restored from `persistence.json` as `stopped`. Force-killing the daemon also ended the pane children (cmd.exe, powershell.exe) and grandchildren (ping.exe), because the ConPTY host closes |
| V4 | Partial failure | `new`, `send`, `screen`, resize and exit detection work. `attach` (run inside a ConPTY) handled input, resize (100x30 -> 120x40) and detach with `Ctrl-^ d`. `exit /b 7` gave `pane.exited` with `exitCode` 7 and `signal` `null`. Japanese input/output (code page 932) is correct. **`kill --force` (`pane.close`) takes about 4.7 s; cmd.exe ends but the grandchild (`ping`) stays alive** (also with powershell.exe). The grandchildren were stopped by their recorded PIDs. Calling `pty.kill()` directly ended both cmd.exe and the grandchild, and the `exit` event arrived at once (`conpty_console_list_agent.js` prints `AttachConsole failed` to stderr, which does not prevent the exit) |
| V5 | Done (no misdetection) | cmd.exe and powershell.exe panes were left idle for 25 s, then resized 5 times from an attach (100x30, 120x40, 80x24, 132x50, 90x28, 2.5 s apart), then idle for 15 s more: `pane.state` stayed `idle` (`decidedBy: bytes`) and never changed. Every resize triggers a full-screen repaint (about 2 KB at 80x24, several KB at 132x50), but it fell inside `byte-activity`'s 800 ms resize grace. Replaying the captured fixtures with the grace disabled also stayed idle. Visible output of `ping -n 14` (one line per second) did not make the pane working either (the threshold is 200 bytes per 3 s). A short burst such as `dir /s` finishes inside the grace (500 ms after input), so it does not become working. None of this is Windows-specific |
| V6 | Equivalent verified | The CI log was not checked. The same smoke was run on the host: it printed `conpty-ok` and exited 0 (output below) |
| V7 | Failures | See the list below. protocol: 56/56 pass. sdk-node: 77 of 125 fail; daemon: 82 of 217 fail (3 skipped); cli: 43 of 137 fail and 11 are cancelled (the `serve` tests do not end on SIGINT / SIGTERM). **All failures are assumptions in the tests** (no implementation bug found). e2e was not run. `daemon` `connection.test.ts` and `stream.test.ts` fail to listen on a `.sock` path and then never close the daemon, so the test runner does not exit (it finishes with `--test-timeout`) |
| V8 | Partial failure | The npm shim `npm.cmd` stood in (there is no `claude.cmd` on this machine). **Names without an extension (`npm`, `node`) fail** (`spawn failed: Cannot create process, error code: 2` / `File not found: `; the latter prints an empty file name). `npm.cmd`, the full path to `npm.cmd`, `node.exe`, `powershell.exe` and `cmd.exe /c npm --version` succeed. `npm.ps1` gives `error code: 193` (not a Win32 application). `pwsh.exe` gives `File not found`. So AZITO sending `cmd: ["claude"]` fails; it has to be `claude.cmd` or wrapped in `cmd.exe /c` |
| V9 | Done | `proc.process` is `"xterm-256color"` (the terminal name). `pane.info` (`ls --json`) has no foreground command field, so the name does not leak. The AGENT column of `ls` shows `cmd.exe` (derived from the head of `cmd`) |
| V10 | Done (E is out) | `net.createServer().listen('<profile>\\...\\a.sock')` fails with `EACCES: permission denied` |
| V11 | Done | Raw bytes of 6 scenarios were captured and saved under `packages/daemon/test/fixtures/windows/` (below) |

**DACL for V1 (SDDL and expansion)**

```
O:S-1-5-21-...-1001G:S-1-5-21-...-513D:(A;;FR;;;WD)(A;;FR;;;AN)(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;S-1-5-21-...-1001)
Everyone                      Allow  Read, Synchronize
NT AUTHORITY\ANONYMOUS LOGON  Allow  Read, Synchronize
NT AUTHORITY\SYSTEM           Allow  FullControl (2032127)
BUILTIN\Administrators        Allow  FullControl (2032127)
win-server-001\ServerUser     Allow  FullControl (2032127)   <- the launching user
```

**ACL conclusion**: A (the default DACL) is not "only the launching user" and is insufficient. The practical damage is smaller than feared (Everyone / anonymous have read only, cannot send requests, and have nothing to receive). Still, (1) anyone can connect and hold connection slots, (2) another host can reach the pipe through SMB, and (3) squatting is not prevented, so **option D (send the token from `%LOCALAPPDATA%\misao` right after connecting) is recommended**. Option B (native addon) only if the requirement rises to "other users must not even connect". E is out. The manual V2 measurements with another user / another host should confirm this.

**Output for V6**

```
"\u001b[?9001h\u001b[?1004h\u001b[?25l\u001b[2J\u001b[m\u001b[Hconpty-ok\r\n\u001b]0;C:\\WINDOWS\\SYSTEM32\\cmd.exe\u0007\u001b[?25h"
```

**Breakdown of the V7 failures (all in the tests)**

| Cause | Where |
| --- | --- |
| Test helpers listen on a `*.sock` in a temp directory (Windows needs a pipe name); `Daemon.start()` fails with `on Windows the daemon endpoint must be a named pipe` | daemon: `daemon.test.ts` (42), `daemon-limits.test.ts` (6), `connection.test.ts` (7), `stream.test.ts` (11). cli: attach / loop / command tests through `test/helpers/daemon.ts` (about 29). sdk-node: `listen EACCES ...fake.sock` in `client*.test.ts` and `rpc-connection.test.ts` |
| POSIX permission / owner checks (`not owned by the current user`, `mode 0600` is 438 = 0666, `ENOTSOCK` handling, an expected `EACCES`) | daemon: `socket.test.ts` (10), the Unix side of `endpoint.test.ts`, `persistence.test.ts` (1) |
| Creating symlinks fails with `EPERM` (standard user without developer mode / privilege) | `socket.test.ts`, `endpoint.test.ts` |
| Path assumptions (`/bin/bash`, `~/`, `/` separators), expectations of `readLoginShell` | cli: `cli-io.test.ts`, `config` tests (about 8), `buildDaemonOptions` in `serve` |
| Graceful exit on SIGINT / SIGTERM (not receivable by Node on Windows, section 5) | cli: 2 `serve` tests (time out after 5 s) |
| sdk-node `socket-path.test.ts` (9) | Reason not examined (likely path-format expectations) |

**V11 fixtures** (`packages/daemon/test/fixtures/windows/`, one JSON per line. The first line is `meta`; the rest are `{t, d}` (the string node-pty's `onData` delivered, and milliseconds since spawn), `{t, input}`, `{t, resize}` and `{t, exit}`)

| File | Content |
| --- | --- |
| `conpty-cmd-basic.jsonl` | cmd.exe start, `echo`, `dir`, Japanese text, `cls`, resize, `exit /b 5` |
| `conpty-cmd-idle-resize.jsonl` | cmd.exe left idle while resized 4 times |
| `conpty-ps-basic.jsonl` | powershell.exe start, `Get-Date`, multi-line output, Japanese text, resize, `exit` |
| `conpty-ps-idle-resize.jsonl` | powershell.exe left idle while resized 4 times |
| `conpty-cmd-flood.jsonl` | a large `dir /s` output (about 105 KB) and Ctrl-C |
| `conpty-fake-agent.jsonl` | **Synthetic** (`claude` is not on this machine). A script that updates the title glyph (`✳` / `⠂`) through OSC 0 and repaints a spinner. Not a capture of the real claude |

Observed (checking the hypotheses in section 7):

- At start: `ESC[?9001h ESC[?1004h`, `ESC[?25l ESC[2J ESC[m ESC[H`, the screen content, `OSC 0;<exe path> BEL`, `ESC[?25h`. cmd updates the title while a command runs (`OSC 0;...cmd.exe - ping ...`) and restores it afterwards.
- Newlines are `\r\n`. When the display advances, the cursor is placed with `ESC[<row>;<col>H` (for example `\x1b[7;1HC:\...>`).
- **Every resize repaints the whole screen**, from `ESC[8;<rows>;<cols>t ESC[H` followed by a run of `ESC[K\r\n`. Through `AnsiLineAssembler`, idle cmd with 4 resizes showed the same lines (two banner lines and the prompt) 4 times each and about 110 empty lines (powershell: the prompt 3 times). **Duplicates in the line stream do happen, as hypothesized.**
- Japanese text arrives as plain strings. cmd sometimes splits the title OSC and the echo into small chunks (`echo 日本語` came in several chunks).
- On exit powershell prints `ESC[?25l`, `ESC[92mexit\r\n`, `ESC[?25h`, then the `exit` event follows.

**Body text that disagreed with the host**: A in section 2.2 ("likely insufficient" -> confirmed), E ("expected" -> not possible), the double start ("expected" -> verified), and the `proc.process` and `close()` behavior in sections 3 and 6 were corrected as above. In addition, the host showed that node-pty installs from its prebuild even when npm 11 does not run `install-scripts`.

**Code to fix in the next stage, found on the host**

1. The win32 branch in `Pane.close()`: use `proc.kill()` instead of `process.kill(-pid, 'SIGHUP')` (grandchildren survive and closing takes about 4.7 s).
2. Test helpers (`daemon/test/helpers`, `cli/test/helpers/daemon.ts`, sdk-node's `fake.sock`, ...) should use a pipe name from `resolveSocketPath` instead of a `*.sock` in a temp directory.
3. `skip` (with a reason) or Windows expectations for POSIX-based tests: `socket.test.ts`, the Unix side of `endpoint.test.ts`, `persistence.test.ts` (mode), `cli-io.test.ts`, `config` tests, SIGINT / SIGTERM in `serve`, symlinks (`EPERM`).
4. `connection.test.ts` / `stream.test.ts` hang after `Daemon.start()` fails (clean up in `after` when `daemon` is `undefined`, or add `--test-timeout`).
5. Command resolution: resolve names without an extension (`claude`) through `PATHEXT` or wrap with `cmd.exe /c`. `spawn failed: File not found: ` has an empty file name, so include the name in the message. Run `.ps1` through `powershell.exe -File`.
6. A stop RPC / `misao stop` (as the `serve` tests show, SIGINT / SIGTERM do not stop it gracefully on Windows).
7. Application-level token authentication (option D, the conclusion of V1).
8. Handling repaints: a resize repaint puts duplicate and empty lines into the line stream. The grace makes `byte-activity` fine as it is, but `tail` / `pane.line` (after `AnsiLineAssembler`) need a Windows-specific de-duplication. The behavior of `scrollback` / `screen` still has to be checked separately.
9. The `Windows (experimental)` CI job: add `--test-timeout` (a hanging test otherwise waits for the 12-minute limit).

**Items that need manual work by the user**: V2's "connection from another standard user" and "connection from another host". Steps: (1) With `serve` running, create another standard user, connect with something like `runas /user:<other>` with `MISAO_SOCKET=\\.\pipe\<name>` and `node packages\cli\dist\main.js status`, and confirm that even if the connection succeeds, no response comes back (write is denied). (2) From another host open `\\<hostname>\pipe\<name>` (the firewall and SMB must allow it). If the results are as expected, adopt option D.

## 11. Proposed next stages

1. Decide the ACL direction (D / B) from V1-V4.
2. The win32 branch in `Pane.close()`, the stop RPC, and `misao stop`.
3. Turn redraws into fixtures and tune activity detection.
4. Foreground inference, the `shell` setting, and the Task Scheduler definition.

## 12. Rust implementation (stage 5, #43)

The Rust build (`crates/`) resolves the Windows items found above. Everything below was run on a Windows 11 25H2 host (build 26200, x64)
with the binary built by CI, and in the `Rust (windows)` / `Rust e2e (windows)` CI jobs. The Node build keeps the behavior described in sections 1-11.

| Item | Rust build |
| --- | --- |
| Endpoint | Named pipe with the SDK's name (`\\.\pipe\misao-<user>-<hash>`; `misao/src/pipe_name.rs` checks it against values generated by `resolveSocketPath`). The user name comes from `GetUserNameW`, as `os.userInfo()` does |
| ACL | Created with `SECURITY_ATTRIBUTES` whose DACL is `D:P(A;;GA;;;<launching user's SID>)`: no SYSTEM, no Administrators, no Everyone, nothing inherited. Measured on the host: `GetAccessControl().GetSecurityDescriptorSddlForm('All')` returned exactly `D:P(A;;FA;;;S-1-5-21-...-1001)` (the Node build's DACL had Everyone / ANONYMOUS LOGON read, section 10.1 V1) |
| Remote clients, name squatting | `PIPE_REJECT_REMOTE_CLIENTS` on every instance; `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first. Measured: `\\127.0.0.1\pipe\<name>` and `\\<host>\pipe\<name>` (which connected to the Node build, V2) are now refused with "access denied". A second daemon on the same name fails with "another daemon is already listening". Another user cannot create further instances of the name either, because the DACL gives them no `FILE_CREATE_PIPE_INSTANCE`. The client does not verify the server's identity (Node cannot; the Rust CLI could with `GetNamedPipeServerProcessId`, left out as the DACL already prevents squatting while an instance exists) |
| Data directory (`--data`) | The Unix "owner is me, not writable by group/other" check becomes: not a reparse point (symlink / junction), a directory, owned by the launching user (or by Administrators, the default owner of files an elevated process creates), and no allow-entry that grants write to Everyone / Users / Authenticated Users / Anonymous (`windows_dir.rs`, read with `GetNamedSecurityInfoW`) |
| ConPTY | portable-pty creates the pseudo console with `PSEUDOCONSOLE_INHERIT_CURSOR`, so ConPTY asks the terminal for the cursor position (`ESC[6n`) at start-up and **does not run the child until it gets an answer**. The daemon answers with the screen model's cursor (`cursor_query.rs`). Found on the host: without it `cmd.exe /c ...` never started and the pane stayed empty. The Node build (node-pty) does not set the flag |
| `pane.close` | The child is put in a Job Object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) right after `CreateProcess`; close is `TerminateJobObject`, and "does the pane still have processes" is the Job's active-process count. No grace period (Windows has no SIGHUP). A grandchild that is started in the instant between `CreateProcess` and the assignment would not be in the Job (portable-pty does not expose `CREATE_SUSPENDED`); shells need milliseconds to start, so this is a theoretical window. If the daemon dies the Job closes and the whole tree is killed. Measured: `misao kill --force` of a `cmd.exe /c "ping ... & ping ..."` pane returned in 243 ms including the CLI's start-up, and both `cmd.exe` and the `ping` it had started were gone (the Node build took about 4.7 s and left the grandchild) |
| Command resolution | `cmd[0]` is resolved with the child's `PATH` and `PATHEXT` (`cmd_resolve.rs`): `claude` -> `claude.cmd`, `npm` -> `npm.cmd`; names with a directory are looked up as given (relative to `cwd`); files without a runnable extension are never picked (npm's extension-less shell script). `.ps1` runs as `powershell.exe -NoProfile -ExecutionPolicy Bypass -File <file> ...` (the policy is relaxed for that one process only, because the default `Restricted` policy refuses scripts). `.cmd` / `.bat` / `.exe` are started directly. A command that cannot be found fails `pane.open` with `command not found: <name>` (no pane is created) |
| Login shell | `%COMSPEC%` (cmd.exe) without `-l`, unchanged: it works with ConPTY in every check above and needs no probing. PowerShell is `misao new -- powershell.exe`. A `shell` key in `misao.json` is not added (nobody asked yet) |
| Line stream | The whole-screen repaint ConPTY writes after every resize is dropped from `pane.line` (`repaint_filter.rs`). Armed by a resize that changes the size; the next frame that starts with `ESC[H` is skipped until its closing cursor restore (`ESC[<r>;<c>H` after `rows-1` line feeds) or `ESC[?25h`, at most 500 ms. Raw output, screen and activity detection still see the original bytes. Measured: four resizes of an idle `cmd.exe` pane (100x30, 80x24, 132x50, 80x24) add no line to `pane.line`. Not changed: ConPTY positions the cursor (`ESC[4;1H`) instead of writing a newline in places, so two consecutive screen lines can be joined into one line (`...reserved.C:\...>`); the Node build does the same |
| Activity detection | Unchanged (the 800 ms resize grace already covers the repaint, section 10.1 V5) |
| Foreground command | Absent on Windows, as in the Node build |
| Stop | `misao stop` (Rust build only). The protocol has no stop method and **stays at 0.3.0**: the daemon creates a named event `Global\misao-stop-<16 hex digits of sha256(lower-cased endpoint)>` whose DACL is the launching user only, `misao stop` sets it and waits for the daemon process to end (measured 53 ms for an idle daemon). The daemon also stops on Ctrl+C / Ctrl+Break / console close / logoff / shutdown. If a stop RPC is wanted later (cross-platform, usable from the SDK), `server.shutdown` would be an additive change and a minor protocol bump (0.4.0) per `version.ts` |
| attach | Raw console mode (`ENABLE_VIRTUAL_TERMINAL_INPUT`, no line editing / echo / Ctrl+C processing; output VT processing on), restored on exit and on panic. Windows has no SIGWINCH, so the terminal size is polled every 250 ms |
| pid file | `OpenProcess` + `GetExitCodeProcess` decide if the recorded pid is alive; a stale file is replaced, as on Unix |
| Supervision | Task Scheduler, not a Windows service (below) |

### Supervision: Task Scheduler

`deploy/windows/misao-task.xml` (installed by `install-task.ps1`, removed by `uninstall-task.ps1`) runs `misao serve` **at logon, as the user, without elevation**.

- A service would run in session 0 with a different environment (PATH, profile, `%USERPROFILE%`), which is what panes inherit; agents like `claude` are installed per user and rely on the user profile.
  Console programs in session 0 also have no user desktop, and ConPTY children there lose the user's HKCU settings. The service would need either `windows-service` (the SCM protocol in `misao` itself) or an external wrapper (WinSW / NSSM) and a stored account password.
- The pipe's ACL is the launching user, which is what the task is. With a service the account would be `SYSTEM` / a service account that the user cannot use.
- A task cannot run while the user is logged off; the daemon is a per-user tool (it owns that user's panes), so it should not.
- The action is `conhost.exe --headless cmd.exe /d /c ""<misao.exe>" serve >> "<log>" 2>&1"`: no console window, and the log (`stderr`) is appended to a file. The same command line was run on the host by hand: the daemon listened, `misao stop` ended it, and `conhost` / `cmd` exited.
- Restart: the task restarts on failure (non-zero exit) after one minute (Task Scheduler's minimum interval), not after `misao stop` (exit 0). systemd's `RestartSec=1` has no equivalent.
- **Registering the task was not run on the host** (it changes system configuration, outside the test sandbox): `install-task.ps1` and the XML are checked by parsing only. To install: `powershell -NoProfile -ExecutionPolicy Bypass -File deploy\windows\install-task.ps1 -MisaoPath C:\path\to\misao.exe -Start`; check with `misao status`; remove with `uninstall-task.ps1`.

### Tests

- `cargo test` on Windows: `windows_endpoint.rs` (ACL, first instance, remote refusal), `windows_panes.rs` (Job Object kills a grandchild, PATHEXT / `.ps1`, missing command, exit code, repaint), `windows_dir.rs` (data directory checks, `icacls` to grant Everyone write), `stop.rs` (`misao stop`), and the pure logic (`cmd_resolve`, `repaint_filter` replaying the recorded repaints, `cursor_query`, `pipe_name`).
- The existing e2e (`packages/e2e`) runs on Windows with POSIX-only cases branched: scripts use `node -e` instead of `sh -c`, `cmd.exe` replaces `bash`, and the cases that need `vim` / `bash` / systemd are skipped with a reason (`helpers/platform.ts`). The hub restart case has no `SIGTERM` on Windows, so only kill-style stops are exercised. `packages/cli/test` is not run on Windows (it drives Node internals and POSIX terminals; the Rust CLI is covered by `cargo test`).
- CI keeps the `Windows (experimental)` job (the Node build, `continue-on-error`) for now; it can be removed once the Node build is retired (#44). `Rust (windows)` and `Rust e2e (windows)` are the jobs that matter.

### Things to know when running on ConPTY

Measured on the host (the Node build behaves the same where it matters):

- **Input**: a line is confirmed by CR (Enter). A bare LF written with `pane.write` does not complete a line in a console program in cooked mode (a Node `process.stdin` read and PowerShell's `Read-Host` both wait). Send `\r`.
- **Resize**: `pane.resize` reaches programs that ask Win32 for the console size (PowerShell's `$Host.UI.RawUI.WindowSize` follows at once), but **Node programs keep the initial size**: `process.stdout.getWindowSize()` stays at 100x30 after a resize to 70x20, and no `resize` event comes (libuv does not see the change under a pseudo console). The Node build, which uses node-pty, shows the same. Node-based TUIs on Windows may therefore not re-layout after a resize.
- **Titles**: ConPTY sets the window title to the executable's path before the child starts, so a pane gets a `pane.title` event with that path first, and the child's own `OSC 0` title arrives after the child's output (the order is the other way round on Unix).
- **Raw replay**: ConPTY updates the screen with cursor-position sequences that rely on what was drawn before, so a raw replay of a ring that has overflowed cannot rebuild the screen. Use `replay: snapshot` for a client that joins late on Windows (the e2e checks only the snapshot there).
- **Lines**: where ConPTY moves the cursor instead of writing a newline, two screen lines come out as one line in `pane.line` (also in the Node build). `cls` produces a screen's worth of empty lines.
- **Exit**: ConPTY's output pipe does not reach EOF when the child exits, so the daemon treats the output as drained when nothing has arrived for 200 ms (at most 2 s); a pane's `pane.exited` follows the exit by about that much.
