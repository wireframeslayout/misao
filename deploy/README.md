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
