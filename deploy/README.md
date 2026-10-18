# Deploying misao as a service

misao runs as a per-user daemon: `misao serve`. The deployment files here keep it running
independently of any controlling app or SSH session, and bring it back after a crash:

| Platform | File | Supervisor |
|---|---|---|
| Linux | `misao.service` | systemd user manager |
| macOS | `com.misao.daemon.binary.plist` | launchd user agent |
| Windows | `windows/misao-task.xml` (+ `install-task.ps1`) | Task Scheduler (at logon) |

All of them run the single `misao` binary from a [GitHub Release](https://github.com/wireframeslayout/misao/releases).
The Node build (`misao-<version>.mjs` and `com.misao.daemon.plist`) is kept during the migration;
see [the last section](#the-node-build-until-it-is-retired).

## Getting the binary

Each release has one archive per platform, `misao-<version>-<target>.tar.gz` (Windows:
`.zip`), and a `SHA256SUMS` that covers every asset of the release.

| Target | Platform |
|---|---|
| `x86_64-unknown-linux-musl` | Linux x64 (static, no glibc needed) |
| `aarch64-unknown-linux-musl` | Linux arm64 (static) |
| `aarch64-apple-darwin` | macOS on Apple silicon |
| `x86_64-apple-darwin` | macOS on Intel |
| `x86_64-pc-windows-msvc` | Windows x64 (Windows 10 version 1809 or later) |

An archive unpacks to one directory, `misao-<version>-<target>/`, holding `misao` (Windows:
`misao.exe`), `LICENSE`, `NOTICE` and `THIRD-PARTY-LICENSES.txt` (the licenses of the crates compiled in).

```bash
VERSION=0.3.0 TARGET=x86_64-unknown-linux-musl   # pick the release and your platform
BASE=https://github.com/wireframeslayout/misao/releases/download/v$VERSION
curl -fsSLO "$BASE/misao-$VERSION-$TARGET.tar.gz" -O "$BASE/SHA256SUMS"
sha256sum -c --ignore-missing SHA256SUMS          # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
tar -xzf "misao-$VERSION-$TARGET.tar.gz"
misao-$VERSION-$TARGET/misao --version            # misao 0.3.0
```

The binary has no runtime dependencies: no Node.js, no `node_modules`, no glibc.

## Linux (systemd)

`misao.service` runs `%h/.local/bin/misao serve` under your user's systemd manager.

- `KillMode=process`: on stop, systemd signals only the daemon, which closes every pane
  itself, so systemd and the daemon do not race over pane shutdown. After a crash, systemd
  does not touch processes left in the cgroup. Agents do not survive a restart.
- `Restart=on-failure`, `RestartSec=1`: the daemon comes back one second after a crash.
  Panes are not restored as live PTYs; their metadata is kept and they show up as `stopped`.
- `misao stop` stops the daemon (it closes every pane and exits with `0`, so `Restart=on-failure`
  does not restart it).

```bash
# 1. Put the binary at the path the unit expects
install -D -m 755 misao-$VERSION-$TARGET/misao ~/.local/bin/misao

# 2. Install and start the unit
mkdir -p ~/.config/systemd/user
cp deploy/misao.service ~/.config/systemd/user/misao.service
systemctl --user daemon-reload
systemctl --user enable --now misao.service

# 3. Check
systemctl --user status misao.service
misao status
```

To keep the daemon running after you log out, enable lingering once:

```bash
loginctl enable-linger "$USER"
```

Panes that ignore SIGHUP can remain after a crash. Stop them with:

```bash
systemctl --user kill --kill-whom=all misao.service
```

Uninstall:

```bash
systemctl --user disable --now misao.service
rm ~/.config/systemd/user/misao.service ~/.local/bin/misao
systemctl --user daemon-reload
```

## macOS (launchd)

`com.misao.daemon.binary.plist` is the launchd counterpart of `misao.service`: a user agent that
runs `misao serve`, with `KeepAlive` with `SuccessfulExit` = false, so it comes back after a crash
but not after a clean stop (like `Restart=on-failure`). It has three placeholders: `__HOME__`,
`__MISAO__` (the absolute path of the binary) and `__PATH__`. launchd does not expand `~`, and its
default PATH has neither Homebrew nor nvm tools, so panes would not find them: set `__PATH__` to
the PATH you want panes to have (below, your current `$PATH`). The template also sets
`LANG=en_US.UTF-8`, because launchd's default locale is not UTF-8. Change it if you use another
UTF-8 locale.

`~/.misao` must have mode `700`: the daemon refuses a socket directory with looser permissions, and
launchd would then restart it in a loop. `mkdir -p` creates it as `755` under the usual umask `022`,
so use `install -d -m 700`.

```bash
install -d -m 755 ~/.local/bin && install -m 755 misao-$VERSION-$TARGET/misao ~/.local/bin/misao
mkdir -p ~/Library/LaunchAgents && install -d -m 700 ~/.misao
chmod 700 ~/.misao   # also when the directory already existed
sed -e "s|__HOME__|$HOME|g" -e "s|__MISAO__|$HOME/.local/bin/misao|g" -e "s|__PATH__|$PATH|g" \
    deploy/com.misao.daemon.binary.plist > ~/Library/LaunchAgents/com.misao.daemon.plist
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/com.misao.daemon.plist

# Check / stop / uninstall
launchctl print "gui/$(id -u)/com.misao.daemon"
launchctl bootout "gui/$(id -u)/com.misao.daemon"
rm ~/Library/LaunchAgents/com.misao.daemon.plist
```

Logs go to `~/.misao/misao.log`. As with the systemd unit, agents do not survive a restart.
The binary is not notarized. A file downloaded with `curl` has no quarantine flag; one downloaded
with a browser needs `xattr -d com.apple.quarantine misao` first.

`scripts/smoke-launchd.sh <binary>` does this in a throw-away environment and checks start, a pane,
and stop (it runs in CI; it refuses to run outside CI unless `MISAO_SMOKE_FORCE=1`, because it uses
`~/.misao` and `~/Library/LaunchAgents`).

## Windows (Task Scheduler)

`windows/misao-task.xml` is a Task Scheduler task that starts `misao serve` at logon, as the user,
without elevation, with no console window and the log appended to `%USERPROFILE%\.misao\misao.log`.
It restarts the daemon one minute after a failure, not after `misao stop`. It is a task, not a
service, because panes must run in the user's session with the user's environment and the pipe's
access control list is that user (the reasons are in [docs/en/windows.md](../docs/en/windows.md), section 12).

The scripts come from the repository (`deploy\windows\`); the binary comes from the release `.zip`.

```powershell
# install (current user, no administrator rights), start now
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\windows\install-task.ps1 -MisaoPath C:\path\to\misao.exe -Start
misao status

# stop and remove
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\windows\uninstall-task.ps1
```

The default endpoint is the named pipe `\\.\pipe\misao-<user>-<hash of %USERPROFILE%\.misao>`, the
same name the SDK resolves, so nothing has to be configured for clients. The binary is not
code-signed, so a `.zip` downloaded with a browser may trigger SmartScreen.

## The Node build (until it is retired)

During the migration every release also carries the Node build, `misao-<version>.mjs` with
`misao-<version>.LICENSES.txt`, for apps that still ship it (see
[Embedding](../docs/embedding.md#the-node-bundle-legacy), which also gives the retirement schedule).
It needs Node.js >= 24 and `node-pty` in a `node_modules` next to the file. The units differ only in the command.

- systemd: put `misao` at `~/.local/bin/misao` as a symlink to a build (`npm ci && npm run build`,
  then `ln -sf "$PWD/packages/cli/dist/main.js" ~/.local/bin/misao`) and use the same `misao.service`.
  `misao` then starts with `#!/usr/bin/env node`; a Node installed through nvm is usually not on the
  PATH of the systemd user manager, so override `ExecStart` with absolute paths
  (`systemctl --user edit misao.service`):

  ```ini
  [Service]
  ExecStart=
  ExecStart=/absolute/path/to/node /absolute/path/to/misao/packages/cli/dist/main.js serve
  ```

- launchd: `com.misao.daemon.plist` runs the `.mjs`. It has the placeholders `__HOME__`, `__NODE__`
  (Node.js >= 24), `__BUNDLE__` (the `.mjs` file) and `__PATH__`. Everything else (the `700`
  directory, `KeepAlive`, logs) is as in the macOS section.

  ```bash
  sed -e "s|__HOME__|$HOME|g" -e "s|__NODE__|$(command -v node)|g" \
      -e "s|__BUNDLE__|/absolute/path/to/misao-0.2.0.mjs|g" -e "s|__PATH__|$PATH|g" \
      deploy/com.misao.daemon.plist > ~/Library/LaunchAgents/com.misao.daemon.plist
  ```

- Windows: the Node build has no `misao stop` and is not supported by the Task Scheduler task. Use the binary.
