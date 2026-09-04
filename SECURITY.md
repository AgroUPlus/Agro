# Security

Agro is a sync daemon that holds a listening history, a social graph, and the credentials to a
music library. This document is what the code assumes about attackers, what it defends, and what it
deliberately does not.

## Reporting a vulnerability

Email **kolbxyz@gmail.com** with `SECURITY` in the subject. Please include what you did, what
happened, and what you expected. There is no bounty; there is a fast reply and credit in the
changelog if you want it.

Do not open a public issue for anything that would let someone else reach an account that is not
theirs.

## Supported versions

`main` is the only supported branch. Agro is self-hosted and unreleased; run the current commit.

## The threat model

Agro assumes:

- **The network is hostile.** The server speaks plain HTTP and expects a reverse proxy to terminate
  TLS. On a public bind without one, every device token is readable in transit.
- **The database file is exfiltratable.** Backups get copied, disks get resold. Nothing in
  `agro_data.db` may be a usable credential on its own.
- **Any authenticated account may be hostile.** A member is not trusted with anything belonging to
  another member. This is what `src/guest_boundary_tests.rs` and `src/social_boundary_tests.rs`
  exist to hold — they are the security contract, more than any prose here.
- **The dashboard's origin may be attacked.** The token lives in `localStorage`, so a script running
  on this origin has it.

Agro does **not** defend against an attacker who already has the server's environment — that
includes `AGRO_SECRET_KEY` — or root on the host. Someone with both the database and the environment
has everything.

## What the credentials are

| Credential | Stored as | Notes |
|---|---|---|
| Account passphrase | Argon2id hash | Generated, not chosen. Shown once. |
| Device token | SHA-256 hash + 8-char clear prefix | The prefix is an index, not a secret. |
| TOTP secret | AES-256-GCM under `AGRO_SECRET_KEY` | The server must be able to read it to verify a code. |
| Recovery codes | SHA-256 | Generated and high-entropy, so no dictionary applies. |
| Vault key | **Not stored.** Only a client-sealed blob | The server cannot open it. See below. |
| Setup token | In memory only | Minted for an empty database, printed once, burned on use. |

Passphrases use Argon2id because a human-memorable secret has little entropy and the only defence is
making each guess expensive. Tokens use SHA-256 because 256 bits from the OS CSPRNG has no
dictionary to run against it, and this runs on **every** authenticated request — a memory-hard hash
there is a denial of service the server performs on itself.

## Why device tokens "bypass" two-factor authentication

This looks like a bug and is not.

A device token is issued *after* every factor has been satisfied. It is the outcome of
authentication, not a way around it — the same shape as an app password. Wanda and Wander hold one
each and never see a passphrase or a TOTP code again, which is the point: a sync daemon cannot
prompt for a code at 3am.

What makes this safe is that the tokens are **revocable and scoped**, and that anything which
invalidates the passphrase invalidates them too:

- Enrolling a second factor revokes every other token on the account.
- Changing the passphrase revokes every token, including the caller's.
- Disabling the second factor requires a current code.
- Tokens expire after `AGRO_TOKEN_IDLE_DAYS` (default 180) without use.

Without that revocation, enrolling 2FA would change nothing for an attacker who already traded a
leaked passphrase for a token. That is the property to preserve if this code is changed.

## The settings vault is zero-knowledge

The client generates a 32-byte key, seals it under Argon2id(passphrase), and sends only the sealed
blob and its salt. The server stores `vault_salt` and `vault_key_wrapped` and **cannot unwrap
either** — it holds a hash of the passphrase, not the passphrase.

The envelope is handed out at exactly one moment: the `/api/v1/login` response, because that is the
only instant the client holds the passphrase. A device paired by QR never receives it.

Accounts that sign in through SSO have no passphrase, so they seal the same key under a separate
**vault PIN** the user sets once. The server never learns that either. `db_identity.rs`'s
`the_server_stores_nothing_that_unwraps_the_vault` is the test that pins this.

## What is playing is sealed too

The vault key derives per-purpose subkeys (HKDF-SHA256, contexts `agro/v1/settings`,
`agro/v1/presence`, `agro/v1/p2p-relay`), and a client holding one seals its now-playing metadata
before sending it. `handoff_state.track_title` and `artist_name` then carry a placeholder and the
real values live in `encrypted_payload`, which the server stores and relays without being able to
open.

That envelope is sealed to the account's *own* vault key, so only its other devices can read it —
which is what a handoff is for. Friends are served separately: the same metadata is sealed once per
friend device public key from `user_device_keys`, the way a drop note is, and those copies live in
`handoff_presence_ciphertexts`. Each viewer is handed only the copy addressed to the device it is
asking from. The server holds N ciphertexts and no key to any of them.

**This changes what the server can read, never who is allowed to look.** `show_now_playing` and
incognito are enforced exactly as before, on rows the server cannot decrypt; a friend who has not
been opted into sees nothing whether or not a copy was sealed for them. The boundary suites in
`social_boundary_tests.rs` assert both halves.

Copies are dropped when the session goes stale, when a device withdraws its key, when a friendship
ends, and when an account is deleted — a ciphertext sealed to a key nobody holds is unreadable by
everyone, so keeping it stores a secret on nobody's behalf.

What is still in clear, deliberately: cross-account popularity (`db_popularity.rs`) aggregates
across accounts and no per-recipient sealing permits that. It is opt-in.

## Federated identity

An unauthenticated SSO callback can sign into an **already-linked** account or create a **brand-new**
one. It can never attach itself to an existing account.

Linking to an existing account happens from inside a signed-in session and nowhere else. The join
key is `(issuer, subject)`; `email` and `preferred_username` are display hints an IdP administrator
can edit, so neither is ever identity. A `preferred_username` that collides with an existing account
produces a suffixed *new* account — never the existing one.

## Configuration that matters

| Variable | Default | Why you care |
|---|---|---|
| `AGRO_SECRET_KEY` | unset | Required for TOTP. Without it, enrolment refuses rather than storing secrets in the clear. |
| `AGRO_REQUIRE_TOTP_ADMIN` | **on** | Administrators must enrol a second factor before using the API. Set to `0` only to recover from a lockout. |
| `AGRO_ALLOWED_ORIGIN` | unset (no CORS) | A wildcard here would let any page make authenticated requests from a visitor's browser. |
| `AGRO_TRUSTED_PROXY` | private ranges | Who may set `X-Forwarded-For`. Wrong value = rate-limit evasion. |
| `AGRO_SIGNUP` | `approval` | `invite` or `closed` on a public bind. Also governs SSO account creation. |
| `AGRO_TOKEN_IDLE_DAYS` | 180 | `0` disables idle expiry. |
| `AGRO_AUDIT_RETENTION_DAYS` | 180 | An unbounded audit log is its own liability. |
| `AGRO_SCROBBLE_RETENTION_DAYS` | unset | Opt-in. Listening history is kept forever unless you say otherwise. |

## Known limitations

These are understood and accepted, not overlooked.

- **The token is in `localStorage`.** An httpOnly cookie would resist XSS better but drags in CSRF
  protection the header-based design currently does not need. Mitigated with a strict CSP.
- **The WebSocket accepts a token in the query string.** Browsers cannot set headers on a handshake.
  It lands in access logs. Clients that can send a header should.
- **The server speaks plain HTTP.** TLS is the reverse proxy's job. HSTS is only sent when the
  request arrived over TLS.
- **Rate limiting is per-process and in memory.** Two instances behind a load balancer each keep
  their own counters.
- **Failed logins are slowed, not blocked.** A hard per-account lockout would let anyone lock a
  known account out on purpose; the delay is capped and the correct passphrase always works.
