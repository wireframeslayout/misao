# Deploying misao as a systemd user unit

`misao.service` runs `misao serve` under your user's systemd manager. The daemon keeps
running (and restarts on failure) independently of any controlling app or SSH session.

- `KillMode=process`: on stop, systemd signals only the daemon, which closes every pane
  itself, so systemd and the daemon do not race over pane shutdown. After a crash, systemd
  does not touch processes left in the cgroup. Agents do not survive a restart.
- `Restart=on-failure`, `RestartSec=1`: the daemon comes back one second after a crash.
  Panes are not restored as live PTYs; their metadata is kept and they show up as `stopped`.

## Install

Requires Node.js >= 24 and a Linux host with `systemd --user`.

```bash
# 1. Build and put `misao` at the path the unit expects (%h/.local/bin/misao)
npm ci
npm run build
mkdir -p ~/.local/bin
ln -sf "$PWD/packages/cli/dist/main.js" ~/.local/bin/misao

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

## Cleaning up after a crash

If the daemon crashes, panes that ignore SIGHUP can remain. Stop them with:

```bash
systemctl --user kill --kill-whom=all misao.service
```

## Node is not on systemd's PATH

`misao` starts with `#!/usr/bin/env node`. A Node installed through nvm or similar is usually
not on the PATH of the systemd user manager. Override `ExecStart` with absolute paths:

```bash
systemctl --user edit misao.service
```

```ini
[Service]
ExecStart=
ExecStart=/absolute/path/to/node /absolute/path/to/misao/packages/cli/dist/main.js serve
```

## Uninstall

```bash
systemctl --user disable --now misao.service
rm ~/.config/systemd/user/misao.service ~/.local/bin/misao
systemctl --user daemon-reload
```

## macOS (launchd) with the release bundle

`com.misao.daemon.plist` is the launchd counterpart of `misao.service` (a separate unit that
runs `misao serve`, with `KeepAlive` with `SuccessfulExit` = false, so it comes back after a crash but not after a clean stop, like `Restart=on-failure`). It runs the single-file
release asset `misao-<version>.mjs`, which needs `node-pty` in a `node_modules` next to it
(see [Embedding](../docs/embedding.md#bundling-the-daemon-and-cli)).

launchd does not expand `~` and does not see nvm/Homebrew Node on its PATH, so the template has
four placeholders to replace: `__HOME__`, `__NODE__` (Node.js >= 24), `__BUNDLE__` (the `.mjs`
file) and `__PATH__`. launchd's default PATH has neither Homebrew nor nvm tools, so panes would not
find them; set `__PATH__` to the PATH you want panes to have (below, your current `$PATH`). The
template also sets `LANG=en_US.UTF-8`, because launchd's default locale is not UTF-8. Change it if
you use another UTF-8 locale.

`~/.misao` must have mode `700`: the daemon refuses a socket directory with looser permissions, and
launchd would then restart it in a loop. `mkdir -p` creates it as `755` under the usual umask `022`,
so use `install -d -m 700`.

```bash
mkdir -p ~/Library/LaunchAgents && install -d -m 700 ~/.misao
chmod 700 ~/.misao   # also when the directory already existed
sed -e "s|__HOME__|$HOME|g" -e "s|__NODE__|$(command -v node)|g" \
    -e "s|__BUNDLE__|/absolute/path/to/misao-0.1.0.mjs|g" -e "s|__PATH__|$PATH|g" \
    deploy/com.misao.daemon.plist > ~/Library/LaunchAgents/com.misao.daemon.plist
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/com.misao.daemon.plist

# Check / stop / uninstall
launchctl print "gui/$(id -u)/com.misao.daemon"
launchctl bootout "gui/$(id -u)/com.misao.daemon"
rm ~/Library/LaunchAgents/com.misao.daemon.plist
```

Logs go to `~/.misao/misao.log`. As with the systemd unit, agents do not survive a restart.

## The Rust build (single binary)

The Rust `misao` is one file, so no Node.js, bundle or `node_modules` is needed. The deployment files are the same units with a different command.

### Linux (systemd)

`misao.service` works unchanged: it runs `%h/.local/bin/misao serve`, so put the binary there (`install -m 755 misao ~/.local/bin/misao`) instead of the symlink to the Node build, and follow the install steps above (`systemctl --user enable --now misao.service`). `misao stop` stops the daemon (it closes every pane and exits with `0`, so `Restart=on-failure` does not restart it).

### macOS (launchd)

`com.misao.daemon.binary.plist` runs the binary directly. It has three placeholders: `__HOME__`, `__MISAO__` (the absolute path of the binary) and `__PATH__`. Everything else about launchd (the `700` directory, `KeepAlive`, logs) is as in the section above.

```bash
mkdir -p ~/Library/LaunchAgents && install -d -m 700 ~/.misao
sed -e "s|__HOME__|$HOME|g" -e "s|__MISAO__|/absolute/path/to/misao|g" -e "s|__PATH__|$PATH|g" \
    deploy/com.misao.daemon.binary.plist > ~/Library/LaunchAgents/com.misao.daemon.plist
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/com.misao.daemon.plist
```

`scripts/smoke-launchd.sh <binary>` does this in a throw-away environment and checks start, a pane, and stop (it runs in CI; it refuses to run outside CI unless `MISAO_SMOKE_FORCE=1`, because it uses `~/.misao` and `~/Library/LaunchAgents`).

### Windows (Task Scheduler)

`windows/misao-task.xml` is a Task Scheduler task that starts `misao serve` at logon, as the user, without elevation, with no console window and the log appended to `%USERPROFILE%\.misao\misao.log`. It restarts the daemon one minute after a failure, not after `misao stop`. It is a task, not a service, because panes must run in the user's session with the user's environment and the pipe's access control list is that user (the reasons are in [docs/en/windows.md](../docs/en/windows.md), section 12).

```powershell
# install (current user, no administrator rights), start now
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\windows\install-task.ps1 -MisaoPath C:\path\to\misao.exe -Start
misao status

# stop and remove
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\windows\uninstall-task.ps1
```

The default endpoint is the named pipe `\\.\pipe\misao-<user>-<hash of %USERPROFILE%\.misao>`, the same name the SDK resolves, so nothing has to be configured for clients.
