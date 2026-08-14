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

## Quickstart — setting up a new user

The server starts with no accounts. While there are none, the API is open — that window exists so
you can create the first account, and it closes the moment that account exists.

**1. Start the server** and open the dashboard (`http://<host>:1674/`, or your proxy's domain).

**2. Create the account.** From the dashboard's user menu, or from a terminal:

```bash
curl -s -X POST https://agro.example.com/graphql \
  -H 'Content-Type: application/json' \
  -d '{"query":"mutation{ createAccount(username:\"alpha\"){ username passphrase qrData } }"}'
```

The response contains the **passphrase**. Save it — it is the account credential, and the only
thing that can create app passwords. There is no recovery: if you lose it, delete the row from
`users` and start again.

**3. Unlock the dashboard.** Reload it; it now asks for the passphrase. It is stored in your
browser's localStorage, so a reload does not sign you out.

**4. Issue one app password per device.** Give each client its own credential, so a lost phone can
be revoked without changing what every other device uses:

```bash
curl -s -X POST https://agro.example.com/graphql \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer <passphrase>' \
  -d '{"query":"mutation{ createAppPassword(userId:\"alpha\", label:\"Pixel 10\"){ token } }"}'
```

The token is shown **once**. `appPasswords(userId:)` lists labels and last-used times afterwards,
never the tokens themselves. `revokeAppPassword(userId:, label:)` removes one.

**5. Point the clients at it.**

Wanda (Android) — Settings → Agro Device → server `agro.example.com`, username `alpha`, passphrase
= that device's app password.

Wander (TUI) — `~/.config/wander/config.toml`:

```toml
[agro]
enabled = true
server = "https://agro.example.com"
username = "alpha"
passphrase = "<that device's app password>"
device_id = "wander-desktop"
sync_settings = true
```

## Authentication

Every `/graphql` and `/ws/sync` request needs `Authorization: Bearer <passphrase or app password>`.
Browsers cannot set headers on a WebSocket handshake, so `/ws/sync` also accepts `?token=`.

Two endpoints stay public by design: the dashboard's static files, and `/share/{token}` — a share
link is a capability URL whose token *is* the credential.

## Security

Requests are authenticated (see above). Credentials are stored as plain tokens in SQLite, so the
database file is sensitive — it is gitignored, and `agro_data.db` should not be world-readable.

Note the trust model: a valid token grants access to **every** account on the server, not just its
own. That is fine for a single-user or household deployment, which is what this is built for.

## Deploying

`./deploy.sh [user@host]` builds the dashboard and the server here — the latter inside a Debian 12
container, so the binary matches the target's older glibc — then uploads it and restarts the
service. Defaults to `root@192.168.1.15`.

The target never compiles anything, which is what lets it be a 512 MB container.
