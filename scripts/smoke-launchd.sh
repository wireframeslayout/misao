#!/usr/bin/env bash
# launchd の配布用テンプレート (deploy/com.misao.daemon.binary.plist) で、misao のデーモンが
# ユーザーエージェントとして起動・停止できることを確かめる (macOS 専用)。
#
# 使い方: scripts/smoke-launchd.sh <misao バイナリの絶対パス>
# ホームの ~/.misao と ~/Library/LaunchAgents を使うので、使い捨ての環境 (CI) か MISAO_SMOKE_FORCE=1 のときだけ動く。
set -euo pipefail

BIN="${1:?usage: $0 <absolute path of the misao binary>}"
LABEL="com.misao.daemon"
TEMPLATE="$(cd "$(dirname "$0")/.." && pwd)/deploy/com.misao.daemon.binary.plist"
AGENT_DIR="$HOME/Library/LaunchAgents"
PLIST="$AGENT_DIR/$LABEL.plist"
MISAO_DIR="$HOME/.misao"

if [ "$(uname -s)" != "Darwin" ]; then echo "macOS only" >&2; exit 2; fi
if [ "${CI:-}" != "true" ] && [ "${MISAO_SMOKE_FORCE:-}" != "1" ]; then
  echo "refusing to touch ~/.misao and ~/Library/LaunchAgents outside CI (set MISAO_SMOKE_FORCE=1 to override)" >&2
  exit 2
fi
if [ -e "$PLIST" ] || [ -e "$MISAO_DIR" ]; then
  echo "refusing to overwrite an existing $PLIST or $MISAO_DIR" >&2
  exit 2
fi

DOMAIN=""
cleanup() {
  if [ -n "$DOMAIN" ]; then launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true; fi
  rm -f "$PLIST"
  rm -rf "$MISAO_DIR"
}
trap cleanup EXIT

install -d -m 700 "$MISAO_DIR"
mkdir -p "$AGENT_DIR"
sed -e "s#__HOME__#$HOME#g" -e "s#__MISAO__#$BIN#g" -e "s#__PATH__#/usr/bin:/bin:/usr/sbin:/sbin#g" "$TEMPLATE" > "$PLIST"
plutil -lint "$PLIST"

# GUI セッションがあれば gui/<uid>、無ければ user/<uid> に載せる。
for candidate in "gui/$(id -u)" "user/$(id -u)"; do
  if launchctl bootstrap "$candidate" "$PLIST" 2>/tmp/misao-bootstrap.err; then DOMAIN="$candidate"; break; fi
  echo "bootstrap into $candidate failed: $(cat /tmp/misao-bootstrap.err)" >&2
done
if [ -z "$DOMAIN" ]; then echo "launchd would not load the agent in any domain" >&2; exit 1; fi
echo "loaded into $DOMAIN"

export MISAO_SOCKET="$MISAO_DIR/misao.sock"
for _ in $(seq 1 60); do
  if "$BIN" status >/dev/null 2>&1; then break; fi
  sleep 0.5
done
"$BIN" status

# ペインを作り、ログインシェルのコマンドが動くこと
"$BIN" new --cwd "$HOME" -- sleep 300 >/dev/null
"$BIN" ls
PANE_PID="$("$BIN" ls --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["pid"])')"

# 停止: launchd が SIGTERM を送り、デーモンがペインを閉じて終わる (KeepAlive は正常終了では再起動しない)
launchctl bootout "$DOMAIN/$LABEL"
DOMAIN=""
for _ in $(seq 1 40); do
  if ! "$BIN" status >/dev/null 2>&1; then break; fi
  sleep 0.5
done
if "$BIN" status >/dev/null 2>&1; then echo "the daemon is still running after bootout" >&2; exit 1; fi
if kill -0 "$PANE_PID" 2>/dev/null; then echo "the pane child $PANE_PID survived the daemon" >&2; exit 1; fi
echo "launchd smoke ok"
