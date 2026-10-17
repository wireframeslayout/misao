# Native Windows support: investigation and prototype

First stage of issue #37. It records what was found by reading the code and the node-pty 1.1.0 package, and describes the prototype that can be verified on Linux.
**Nothing has been run on a Windows host yet.** Every statement marked "verify on host" is a hypothesis, to be checked with the list in section 10.

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
| A. Rely on the default DACL (current prototype) | The DACL from a `NULL` SECURITY_ATTRIBUTES (Microsoft documents full control for SYSTEM / administrators / creator owner and read for Everyone and anonymous) | None | Weak; read access may leak depending on connection behavior | S | Do not call it "protected" before checking the ACL on a host |
| B. Create the pipe in a native addon | `CreateNamedPipeW` with SDDL `D:P(A;;GA;;;<user SID>)`, bridged to Node as a Duplex | **Needs approval** (N-API addon or an ffi library, plus prebuild distribution) | Strong (with `FILE_FLAG_FIRST_PIPE_INSTANCE` also resists squatting) | L | An existing handle cannot be given to a libuv pipe handle, so the whole server has to live in the addon |
| C. Rewrite the DACL after listen | `SetNamedSecurityInfo`-style call from outside | No way found in PowerShell / .NET to change the DACL of a pipe another process created; `icacls` cannot address pipes (verify on host) | - | - | Not realistic |
| D. Authenticate at the application layer | A token file in `%LOCALAPPDATA%\misao` (user-only via the profile ACL) sent right after connecting | None (add an auth exchange to the protocol) | Medium: anyone can connect but cannot do anything. Within one user there is no boundary, as on Unix | M | Cheap to add while the protocol is 0.x |
| E. Use AF_UNIX | Windows 10 1803+ has AF_UNIX, giving the same "parent directory ACL" model as Unix | None | Strong | - | libuv is expected not to support AF_UNIX on Windows (verify on host: `net.createServer().listen('C:\\x\\a.sock')`) |
| F. Loopback TCP + token | Listen on loopback, authenticate with a token | None | Medium (other users can still connect) | M | The endpoint becomes TCP, changing assumptions in the SDK and AZITO |

**Recommendation**: first check the real ACL of option A on a host (item V1). Then D is the first candidate (Node only), and B the second candidate if a full ACL becomes a requirement. B adds a dependency and needs approval.

Other measures:

- **Client-side squatting**: if another user creates a pipe with the same name first, the SDK connects to it. Node cannot check the server process ID (`GetNamedPipeServerProcessId`). The token in D also covers this (a server without the token cannot answer). On the server side libuv is expected to set `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second daemon gets `EADDRINUSE` (verify on host).
- **Remote clients**: libuv is expected to set `PIPE_REJECT_REMOTE_CLIENTS` (verify on host).
- **dataDir**: under `%USERPROFILE%` it inherits the profile's default ACL (user / SYSTEM / administrators). The equivalent of the Unix "700 + uid" check is not implemented because Node cannot read ACLs; only symlink and directory checks are done.

## 3. Foreground process

- Existing problem found while reading the code: `foreground.ts` uses node-pty's `process` on non-Linux platforms, but `WindowsTerminal.process` in node-pty 1.1.0 returns `opt.name` (misao passes `xterm-256color`). On Windows the **terminal name would have been reported as the foreground command** instead of a program name.
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

- **ConPTY or winpty**: when `useConpty` is unset, `WindowsPtyAgent` uses ConPTY on Windows build 18309 or later and winpty (`winpty-agent.exe`) below that. misao does not set it, so it is auto-selected: ConPTY on Windows 10 1809+.
- **Build**: `install` runs `node scripts/prebuild.js || node-gyp rebuild`. `prebuilds/` contains `win32-x64`, `win32-arm64`, `darwin-arm64` and `darwin-x64` (`conpty.node`, `conpty_console_list.node`, `pty.node` (winpty), `winpty-agent.exe`, `winpty.dll`), so Windows is expected to use a prebuild without a compiler. There is no Linux prebuild; on Linux it is built with `node-gyp rebuild`. CI verifies this (V6).
- **spawn-helper**: Unix only (a helper binary used by `UnixTerminal` at `fork`, shipped in the macOS prebuilds). It is not used on Windows and needs nothing.
- **conpty.dll**: `useConptyDll` (default false) can switch to the bundled OpenConsole-based DLL (`third_party/conpty/1.23.251008001`). The default is the OS ConPTY. A candidate if old OS versions or the redraw differences below become a problem.
- **API differences** (read from `lib/windowsTerminal.js`):
  - `kill(signal)` throws `Signals not supported on windows.` when a signal is given. `Pane.close()` goes `process.kill(-pid, 'SIGHUP')` then, on failure, `process.kill(pid)` (a forced kill of that process only, not the tree). A win32 branch using `proc.kill()` (which ends the processes attached to the ConPTY console) is needed. **Not implemented in this PR**; item V4 observes the behavior.
  - `encoding: null` is falsy, so no warning is printed. Whether data arrives as Buffer or string needs checking (misao assumes Buffer).
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
| V1 | Default DACL | Start `node packages/cli/dist/main.js serve`; in another shell run `$p=[IO.Pipes.NamedPipeClientStream]::new('.','misao-<resolved name>','InOut'); $p.Connect(); $p.GetAccessControl().Access` | If Everyone / Users have an allow entry, option A is insufficient; if only the user, SYSTEM and administrators do, A is enough |
| V2 | Connection from another user | Create a standard user and try the same connection via `runas` | Must be refused |
| V3 | Double start and stale | Start two `serve` instances; `Stop-Process -Force` one and restart | The second start fails; restart works after a forced kill |
| V4 | ConPTY pane | `misao new -- cmd.exe`, `misao send`, `misao screen`, `misao attach`, resize, `exit` | Input, output, resize and exit detection (`pane.state` becomes exited) work; `pane.close` leaves no child |
| V5 | `server.info` / `pane.state` | Watch idle/working transitions with `misao status` and `misao events` | No "working" while idle (no redraw misdetection) |
| V6 | CI native install | Log of the `Windows (experimental)` job in the PR | `npm ci` succeeds and the node-pty ConPTY smoke prints `conpty-ok` |
| V7 | Unit tests | `npm test` in the same job | The failing tests become the phase-2 skip / fix list |
| V8 | Command resolution | `misao new -- claude` (npm shim), `-- powershell.exe`, `-- pwsh.exe` | Confirm that shims fail; wrap with `cmd.exe /c` if needed |
| V9 | Foreground command | The foreground command in `pane.info` is empty; the value of `proc.process` | The terminal name does not leak |
| V10 | AF_UNIX | `node -e "require('net').createServer().listen('C:\\\\Temp\\\\a.sock')"` | If it fails, option E is out |
| V11 | Raw bytes | Save cmd / PowerShell / claude output to files | Use as the fixtures for section 7 |

## 11. Proposed next stages

1. Decide the ACL direction (D / B) from V1-V4.
2. The win32 branch in `Pane.close()`, the stop RPC, and `misao stop`.
3. Turn redraws into fixtures and tune activity detection.
4. Foreground inference, the `shell` setting, and the Task Scheduler definition.
