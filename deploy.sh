#!/usr/bin/env bash
# Rsync the source to a host, rebuild it there and restart the service.
#
# Building on the target rather than shipping a binary avoids glibc mismatches between the dev
# machine and the container. Usage: ./deploy.sh root@192.168.1.18 [/opt/agro]
set -euo pipefail

HOST="${1:?usage: deploy.sh <user@host> [remote-path]}"
REMOTE_PATH="${2:-/opt/agro}"

# The database lives on the target and must survive a deploy; node_modules and target/ are rebuilt
# there, so sending them would only be slow.
rsync -az --delete \
    --exclude 'target/' \
    --exclude 'dashboard/node_modules/' \
    --exclude 'dashboard/dist/' \
    --exclude 'agro_data.db' \
    --exclude '.git/' \
    ./ "$HOST:$REMOTE_PATH/"

ssh "$HOST" bash -euo pipefail <<REMOTE
cd "$REMOTE_PATH/dashboard"
npm ci --silent
npm run build
cd "$REMOTE_PATH"
cargo build --release
systemctl restart agro
systemctl --no-pager --lines=5 status agro
REMOTE
