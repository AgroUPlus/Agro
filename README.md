# Agro

Background sync daemon for [Wander](https://github.com/Kolbxyz/wander) (Linux TUI) and
[Wanda](https://github.com/Kolbxyz/Wanda) (Android). It keeps one playback handoff and a set of registered nodes per user, so a
session started on one device can be picked up on another, and serves its own dashboard.

- GraphQL API — `POST /graphql`
- Live push — `GET /ws/sync` (WebSocket; broadcasts `HANDOFF`, `NODE_UPDATE`, `SETTINGS_SYNC`)
- Dashboard — served at `/`, compiled into the binary
- Storage — SQLite, single file, no external database

## Build

The React dashboard is embedded into the Rust binary by `rust-embed`
(`#[folder = "dashboard/dist/"]`), so **the dashboard must be built first** — a fresh clone has no
`dashboard/dist/` and `cargo build` will fail without it.

```bash
cd dashboard && npm ci && npm run build
cd .. && cargo build --release
```

Requires a Rust toolchain and Node 20+. SQLite is bundled — no system library needed.

## Run

```bash
PORT=1674 ./target/release/agro
```

`PORT` defaults to `8700`. The listener always binds `0.0.0.0`.

The database path is **relative** (`agro_data.db`), so run it from the directory you want the
database to live in — under systemd, set `WorkingDirectory`.

### systemd

```ini
[Unit]
Description=Agro sync server
After=network-online.target

[Service]
Type=simple
User=agro
WorkingDirectory=/opt/agro
Environment=PORT=1674
ExecStart=/opt/agro/target/release/agro
Restart=always
RestartSec=5
ProtectSystem=strict
ReadWritePaths=/opt/agro
PrivateTmp=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

`systemctl enable --now agro` · `journalctl -fu agro`

### Sizing

Building wants ~4 GB RAM and ~12 GB disk (tokio, async-graphql, reqwest, lofty). The running server
idles at 20–30 MB RSS, so a build-once container can be dialled back to 1 GB afterwards. Use
`cargo build --release -j2` if memory is tight.

## Security

**The API is unauthenticated.** Any client that can reach `/graphql` can read and write any user's
nodes, handoff and settings — the `Authorization` header clients send is not checked. That is
acceptable on a trusted LAN and is not acceptable on a public address. If you expose it through a
reverse proxy, put access control in front of it.

## Deploying

`./deploy.sh <host>` rsyncs the source, rebuilds on the target and restarts the service over SSH.
