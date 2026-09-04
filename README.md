# Agro

Background sync daemon for [Wander](https://github.com/Kolbxyz/wander) (Linux TUI) and
[Wanda](https://github.com/Kolbxyz/Wanda) (Android). It keeps one playback handoff and a set of registered nodes per user, so a
session started on one device can be picked up on another, and serves its own dashboard.

With more than one account it is also the social layer: profiles, friends, a live "what your friends
are playing" feed, and listen-along. All of it is off until each account opts in — see
*Friends, and what a friendship reveals*.

- GraphQL API — `POST /graphql`
- Live push — `GET /ws/sync` (WebSocket; `HANDOFF`, `NODE_UPDATE`, `SETTINGS_SYNC`, `LIBRARY_UPDATED`, `SYNC_OFFER`, `FRIEND_PRESENCE`, `FRIEND_REQUEST`, `LISTEN_ALONG`). `HANDOFF` and `FRIEND_PRESENCE` carry sealed metadata when the sender holds a vault key — see [SECURITY.md](SECURITY.md); `FRIEND_PRESENCE` is sent per recipient rather than broadcast, because a sealed copy is addressed to one device's key.
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
| `AGRO_ALLOWED_ORIGIN` | CORS origin for the dashboard. No wildcard. |
| `AGRO_SIGNUP` | `approval` (default), `invite` or `closed`. See *Opening the server to other people*. |

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

### Behind a reverse proxy

Agro speaks plain HTTP on `PORT` and expects something in front of it to terminate TLS. Three of
its routes are not ordinary request/response traffic, and a proxy's defaults will break each one in
a way that looks like a bug in the app rather than in the proxy:

| Route | What it does | What a default proxy does to it |
|---|---|---|
| `/ws/sync` | WebSocket | Fails to upgrade; clients silently never receive pushes |
| `/api/v1/library/upload/{id}` | One `PUT` of the remaining bytes | Rejected at `client_max_body_size` (default 1 MB) |
| `/api/v1/relay/{id}/send` and `/receive` | A live duplex stream between two devices | Buffered, so the receiver gets no response headers until a buffer fills — the transfer appears to hang, then times out having delivered nothing |

The relay one is worth spelling out because it is silent. `receive` answers `200` immediately and
then streams bytes as the sending device produces them. With `proxy_buffering on` — nginx's default
— nothing reaches the client until nginx has filled a buffer, and since the sender is waiting on
the receiver to drain, neither side moves. The client sees an open socket that never delivers.

#### Nginx Proxy Manager

Turn on **Websockets Support** on the proxy host (that covers `/ws/sync`), then paste this into
**Advanced → Custom Nginx Configuration**, replacing the address with your server's:

```nginx
# Uploads are one PUT of whatever is left of the file, resumable by offset — not small chunks.
client_max_body_size 0;

location /api/v1/relay/ {
    proxy_pass http://192.168.1.16:1674;
    proxy_http_version 1.1;

    # The two that matter. Without them the relay opens, streams nothing, and times out.
    proxy_buffering off;
    proxy_request_buffering off;

    # A relay lasts as long as the transfer does.
    proxy_read_timeout 1h;
    proxy_send_timeout 1h;

    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
}
```

#### Plain nginx

The same thing, in a server block:

```nginx
location / {
    proxy_pass http://127.0.0.1:1674;
    proxy_http_version 1.1;
    client_max_body_size 0;
    proxy_set_header Host $host;

    # WebSocket upgrade for /ws/sync.
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}

location /api/v1/relay/ {
    proxy_pass http://127.0.0.1:1674;
    proxy_http_version 1.1;
    proxy_buffering off;
    proxy_request_buffering off;
    proxy_read_timeout 1h;
    proxy_send_timeout 1h;
}
```

#### Caddy

Caddy streams by default and sets no body limit, so it needs none of this:

```caddyfile
agro.example.com {
    reverse_proxy 127.0.0.1:1674
}
```

#### Checking it

The relay only runs when two devices cannot reach each other directly, so the way to exercise it is
to take one device off the local network — mobile data is enough. A working relay logs
`Relay stream open` on the receiving client; a proxy still buffering shows the session opening and
then nothing at all.

### Sizing

Building wants ~4 GB RAM and ~12 GB disk (tokio, async-graphql, reqwest, lofty). The running server
idles at 20–30 MB RSS, so a build-once container can be dialled back to 1 GB afterwards. Use
`cargo build --release -j2` if memory is tight.

## Quickstart — setting up a new user

The server starts with no accounts. On a database with none, it prints a **one-time setup token** to
its log at boot; that token is the only thing that can create the first administrator, it is never
stored, and a restart replaces it.

**1. Start the server** and read the token out of the log:

```
journalctl -u agro | grep -A2 'setup token'
```

**2. Create the administrator.**

```bash
curl -s -X POST https://agro.example.com/api/v1/bootstrap \
  -H 'Content-Type: application/json' \
  -d '{"setup_token":"<from the log>","username":"alpha"}'
```

The response carries the **passphrase** and a device token, both shown once. Save the passphrase —
the server keeps an Argon2 hash and cannot show it again, and there is no reset.

**3. Unlock the dashboard.** Reload it and sign in with the username and passphrase. It trades them
for a device token of its own and keeps that in localStorage.

**4. Pair each device.** A client never uses the passphrase as a credential. It sends it once to
`/api/v1/login`, which returns a token scoped to that device:

```bash
curl -s -X POST https://agro.example.com/api/v1/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"alpha","passphrase":"<your passphrase>","label":"Pixel 10"}'
```

Wanda (Android) does this for you: **Settings → Agro Device**, enter the server, username and
passphrase. The dashboard's **Pairing** tab does the same thing as a QR code.

`appPasswords(userId:)` lists labels and last-used times, never the tokens. `revokeAppPassword(userId:,
label:)` removes one — a lost phone is revoked on its own, without changing what every other device
uses.

Wander (TUI) — `~/.config/wander/config.toml`:

```toml
[agro]
enabled = true
server = "https://agro.example.com"
username = "alpha"
passphrase = "<that device's token>"
device_id = "wander-desktop"
sync_settings = true
```

## Opening the server to other people

Set `AGRO_SIGNUP` and `POST /api/v1/signup` starts accepting strangers:

| `AGRO_SIGNUP` | |
|---|---|
| `approval` | Default. Anyone may register; the account is created `pending` and cannot sign in until you let it in from the dashboard's **People** tab. An invite code, if one is offered, skips the queue. |
| `invite` | A valid code is required, and spending one lets the account straight in. Codes are minted under **People**. |
| `closed` | Registrations are refused. |

Signup is rate-limited per client address, like `/api/v1/login`: ten attempts per five minutes.
It also refuses outright until the server has an administrator, whatever `AGRO_SIGNUP` says — the
first account has to be the one bootstrap creates. A stranger who got in first would occupy the
empty database that `/api/v1/bootstrap` requires, leaving an instance with a pending account and
nobody entitled to approve it.

### Friends, and what a friendship reveals

Nothing, by itself. A friendship is a door, not a window — each surface is gated on its own switch
on the account being looked at, and **every switch defaults off**:

| | |
|---|---|
| `showNowPlaying` | Accepted friends may see what you are playing, and may follow it with listen-along. |
| `showStats` | Accepted friends may see your listening statistics and how far your taste overlaps theirs. |
| `discoverable` | You appear in `searchUsers`, which is the only way a stranger can find you to send a request. |

Set them with `setVisibility`; Wanda exposes them under **Settings → Privacy**. Search is
prefix-anchored and lists only discoverable, active accounts, so the directory cannot be walked.
Blocking is symmetric in effect and never disclosed to the account it was applied to.

Every refusal on this path — not a friend, switch is off, no such account — is deliberately the same
refusal, so an error message cannot be used as the directory that `discoverable` exists to opt out
of. `src/social_boundary_tests.rs` is where those guarantees are written down.

## Share links on your own domain

The wire format is specified in [`SHARE_LINKS.md`](SHARE_LINKS.md), which is normative for all
three implementations. Proxy settings for this and every other route are under
[*Behind a reverse proxy*](#behind-a-reverse-proxy).

Optional, and off until asked for. With it on, Wanda and Wander stop sharing their backends' own
links — a Navidrome URL only you can reach, a YouTube link useless to someone who does not use it —
and send out `https://your-domain/listen?v=<id>` instead. This server forwards whoever opens one to
where the track actually is.

Set it up in the dashboard, under **Share Links**:

1. **Share Domain** — the domain you own, e.g. `frwd.top`. Point its DNS at this server (an `A`
   record to this host, or a `CNAME` if it sits behind the same proxy) and make sure the proxy
   serves `/listen` from here.
2. **Forward To** — your music server's host, comma separated for more than one. YouTube's hosts
   are always allowed. Everything else is refused: a forwarder that will send a visitor to any
   address handed to it is an open redirect wearing your domain, so the list is the whole point.
3. **On**, then **Sync to Devices**. Every paired player picks the domain up on its next
   foreground — nothing to type into each one.

Turning it **Off** puts the players back to their backends' own links immediately.

Both players also have a local field for this (Wanda: *Settings → Sharing*; Wander: `[share]` in
`config.toml`), used when no server publishes one. Sharing never depends on Agro being present,
paired or reachable — this only saves configuring it twice.

`/listen` is public, like `/share/{token}`: a shared link is opened by someone with no account
here. It records nothing — no log line, no counter, no cookie.

## Authentication

Every `/graphql` and `/ws/sync` request needs `Authorization: Bearer <device token>`. Browsers
cannot set headers on a WebSocket handshake, so `/ws/sync` also accepts `?token=`.

A **passphrase is not a bearer token.** It is Argon2-hashed, it is accepted only by
`/api/v1/login`, and what that returns is a per-device credential you can revoke on its own. The two
used to be the same string, which meant photographing a pairing QR handed over the whole account.

Four routes are reachable without a token, each for a reason it could not work otherwise:

| | |
|---|---|
| `POST /api/v1/bootstrap` | Creates the first admin. Needs the setup token from the log, and refuses once any account exists. |
| `POST /api/v1/login` | Trades a passphrase for a device token. |
| `POST /api/v1/signup` | Registers a stranger, when `AGRO_SIGNUP` allows it. |
| `GET /share/{token}`, `GET /listen` | Capability URLs — the token in the path *is* the credential. |

The first three are rate-limited per client address. The dashboard's static files are public too;
it holds no data of its own.

## Security

Requests are authenticated (see above). Passphrases are stored as Argon2 hashes and device tokens as
SHA-256 hashes, so the database no longer holds a credential that can be replayed — but it still
holds everyone's listening history, and `agro_data.db` should not be world-readable. It is
gitignored.

A token is scoped to the account it belongs to: every GraphQL field that names a `userId` checks it
against the identity the token resolved to, and answers `Forbidden` otherwise. The social fields are
the one deliberate exception, and they are gated by the per-surface switches described above rather
than by friendship alone.

Two test suites exist to keep those boundaries from quietly reopening — `guest_boundary_tests.rs`
for what a hostile account cannot reach, and `social_boundary_tests.rs` for what a friendship must
still refuse. Both run under `cargo test`.

The archive hook runs a shell command as the service user. Treat `AGRO_ARCHIVE_HOOK` as trusted
configuration — the file paths it is given arrive in the environment rather than in the command
line, precisely so that client-supplied tags cannot get into what the shell parses.

## Deploying

`./deploy.sh <user@host>` builds the dashboard and the server here — the latter inside a Debian 12
container, so the binary matches the target's older glibc — then uploads it and restarts the
service. You can also pass the host via the `AGRO_DEPLOY_HOST` environment variable.

The target never compiles anything, which is what lets it be a small container.

## Licence

AGPL-3.0 — see [`LICENSE`](LICENSE).

Section 13 is the part that matters for a server: run a modified Agro where other people can reach
it, and those people must be offered its source. That is deliberate. The project is given away;
what is sold is running it. See [`AGRO_PREMIUM.md`](AGRO_PREMIUM.md).

Contributions require agreement to [`CLA.md`](CLA.md) — see [`CONTRIBUTING.md`](CONTRIBUTING.md).
