#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE' >&2
Usage: deploy-server.sh <ssh-target> [artifact] [health-url]

Examples:
  deploy-server.sh root@47.114.107.118
  deploy-server.sh root@47.114.107.118 dist/linux/kmsync-server-0.1.0-linux-x86_64-musl.tar.gz
  deploy-server.sh root@47.114.107.118 dist/linux/kmsync-server-0.1.0-linux-x86_64-musl.tar.gz http://47.114.107.118:24888/health
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -lt 1 || $# -gt 3 ]]; then
  usage
  exit 2
fi

SSH_TARGET="$1"
ARTIFACT="${2:-dist/linux/kmsync-server-0.1.0-linux-x86_64-musl.tar.gz}"

if [[ ! -f "$ARTIFACT" ]]; then
  echo "artifact not found: $ARTIFACT" >&2
  exit 1
fi

host_without_user="${SSH_TARGET##*@}"
host_without_port="${host_without_user%%:*}"
HEALTH_URL="${3:-http://${host_without_port}:24888/health}"
REMOTE_ARTIFACT="/tmp/$(basename "$ARTIFACT").$$"

echo "Uploading $ARTIFACT to $SSH_TARGET:$REMOTE_ARTIFACT"
scp "$ARTIFACT" "$SSH_TARGET:$REMOTE_ARTIFACT"

echo "Installing kmsync-server on $SSH_TARGET"
ssh "$SSH_TARGET" "REMOTE_ARTIFACT='$REMOTE_ARTIFACT' bash -s" <<'REMOTE'
set -euo pipefail

tmpdir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmpdir" "$REMOTE_ARTIFACT"
}
trap cleanup EXIT

tar -xzf "$REMOTE_ARTIFACT" -C "$tmpdir" --strip-components=1
server_bin="$tmpdir/bin/kmsync-server"
server_unit="$tmpdir/systemd/kmsync-server.service"
server_config="$tmpdir/config/kmsync-server.example.json"

if [[ ! -x "$server_bin" ]]; then
  echo "package is missing executable bin/kmsync-server" >&2
  exit 1
fi

if ! id -u kmsync >/dev/null 2>&1; then
  sudo useradd --system --home /var/lib/kmsync --shell /usr/sbin/nologin kmsync
fi

sudo install -d -o kmsync -g kmsync /opt/kmsync/bin /var/lib/kmsync
sudo install -d -o root -g kmsync /etc/kmsync

if [[ -x /opt/kmsync/bin/kmsync-server ]]; then
  backup="/opt/kmsync/bin/kmsync-server.backup.$(date +%Y%m%d%H%M%S)"
  sudo cp /opt/kmsync/bin/kmsync-server "$backup"
  echo "Backed up existing server to $backup"
fi

sudo install -m 0755 "$server_bin" /opt/kmsync/bin/kmsync-server

if [[ ! -f /etc/kmsync/kmsync-server.json ]]; then
  sudo install -m 0640 -o root -g kmsync "$server_config" /etc/kmsync/kmsync-server.json
  echo "Installed default config to /etc/kmsync/kmsync-server.json"
else
  echo "Keeping existing /etc/kmsync/kmsync-server.json"
fi

sudo install -m 0644 "$server_unit" /etc/systemd/system/kmsync-server.service
sudo systemctl daemon-reload
sudo systemctl enable kmsync-server
sudo systemctl restart kmsync-server
sudo systemctl --no-pager --full status kmsync-server
REMOTE

echo "Checking server capability at $HEALTH_URL"
health="$(curl -fsS "$HEALTH_URL")"
echo "$health"

if ! printf '%s' "$health" | grep -q '"relay_rx_status"[[:space:]]*:[[:space:]]*true'; then
  echo "server health does not report relay_rx_status=true; the running server is still too old for accurate sync-channel status" >&2
  exit 1
fi

echo "kmsync-server deployment verified: relay_rx_status=true"
