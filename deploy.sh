#!/usr/bin/env bash
# Build Agro and deploy it to the server.
#
# The build runs here, in a Debian 12 container, not on the target: the LXC has 512 MB of RAM and
# a few GB of disk, which is not enough to compile Rust, and its glibc (2.36) is older than this
# machine's, so a binary built directly on the host would not run there.
#
#   ./deploy.sh                     # deploys to root@192.168.1.15
#   ./deploy.sh root@other-host     # deploys somewhere else
set -euo pipefail

HOST="${1:-root@192.168.1.15}"
REMOTE_PATH="/opt/agro"

echo "==> Building the dashboard"
(cd dashboard && npm install --silent && npm run build)

echo "==> Building the server for Debian 12"
docker run --rm \
    -v "$PWD":/src -w /src \
    -e CARGO_TARGET_DIR=/src/target-deb \
    rust:1-bookworm \
    cargo build --release

echo "==> Uploading to $HOST"
scp -q target-deb/release/agro "$HOST:$REMOTE_PATH/agro.new"

echo "==> Restarting the service"
# The binary is swapped while the service is stopped: replacing a running executable in place is
# what "Text file busy" means.
ssh "$HOST" bash -euo pipefail <<REMOTE
systemctl stop agro
mv $REMOTE_PATH/agro.new $REMOTE_PATH/agro
chown agro:agro $REMOTE_PATH/agro
chmod 755 $REMOTE_PATH/agro
systemctl start agro
sleep 2
systemctl is-active agro
REMOTE

echo "==> Checking it answers"
PORT_URL="http://${HOST#*@}:1674/graphql"
curl -fsS -m 10 -X POST "$PORT_URL" \
    -H 'Content-Type: application/json' \
    -d '{"query":"{ health }"}'
echo
echo "Deployed. Dashboard: http://${HOST#*@}:1674/"
