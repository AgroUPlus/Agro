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

### Environment

| | |
|---|---|
| `PORT` | Listen port. Default `8700`. |
| `AGRO_PUBLIC_URL` | Base URL used to build share links. |
| `AGRO_LIBRARY_ROOT` | The music library — any ordinary directory. **Unset means index-only**: agro records which device holds what, but never keeps the bytes. |
| `AGRO_SPOOL_ROOT` | Staging for in-flight uploads and files waiting for a peer. Default `./spool`. |
| `AGRO_SPOOL_MAX_BYTES` | Spool budget, oldest evicted first. Default 2 GiB. |
| `AGRO_SPOOL_TTL_HOURS` | How long a spooled file waits to be collected. Default 72. |
| `AGRO_ARCHIVE_HOOK` | Optional shell command run after a file is filed. Default: nothing. |

Agro writes to `AGRO_LIBRARY_ROOT` as a plain directory — no assumptions beyond that, and no
integration with whatever else reads it. If something *does* keep its own index of that directory,
`AGRO_ARCHIVE_HOOK` is how it gets told. The hook receives the new file's path relative to the root
in `AGRO_ARCHIVED_PATH` and the absolute path in `AGRO_ARCHIVED_ABS`, runs detached with a 60 s
timeout, and can never fail an upload — by the time it runs, the bytes are already filed.

A media scanner that watches the tree itself (Navidrome, Jellyfin) needs no hook. A Nextcloud data
directory does, because Nextcloud serves from its database rather than from the disk:

```ini
Environment=AGRO_ARCHIVE_HOOK=docker exec -u www-data nextcloud php occ files:scan --path="alpha/files/Music"
```

Archived files are created mode `0664`, so a library shared with another service through a common
group on a setgid directory stays writable by both.

### systemd

```ini
[Unit]
Description=Agro sync server
After=network-online.target

[Service]
Type=simple
User=agro
# The group the library directory is shared with, plus a umask that keeps new files group-writable.
SupplementaryGroups=www-data
UMask=0002
WorkingDirectory=/opt/agro
Environment=PORT=1674
Environment=AGRO_LIBRARY_ROOT=/srv/music
ExecStart=/opt/agro/agro
Restart=always
RestartSec=5
ProtectSystem=strict
# Both roots must be listed. Under ProtectSystem=strict the library is read-only otherwise, and
# every archive fails — this is the usual reason a correct AGRO_LIBRARY_ROOT still does not write.
ReadWritePaths=/opt/agro /srv/music
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

A token is scoped to the account it belongs to: every GraphQL field that names a `userId` checks it
against the identity the token resolved to, and answers `Forbidden` otherwise.

The archive hook runs a shell command as the service user. Treat `AGRO_ARCHIVE_HOOK` as trusted
configuration — the file paths it is given arrive in the environment rather than in the command
line, precisely so that client-supplied tags cannot get into what the shell parses.

## Deploying

`./deploy.sh [user@host]` builds the dashboard and the server here — the latter inside a Debian 12
container, so the binary matches the target's older glibc — then uploads it and restarts the
service. Defaults to `root@192.168.1.16`, the host the music library lives on.

The target never compiles anything, which is what lets it be a small container.
