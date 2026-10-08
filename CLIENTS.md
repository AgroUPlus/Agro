# Writing an Agro client

Agro does not speak Subsonic. It has its own small API, and any app can use it. This is the
shortest path to a client that signs in, shows your other devices and hands a session over.

Every shape below comes from the source. When something here disagrees with a running server,
the server wins: read `/graphql/sdl` and `/api/docs/openapi.json` on your own instance.

## The pieces

| What | Where |
|---|---|
| Sign in | `POST /api/v1/login` (REST, no token needed) |
| Everything else | `POST /graphql` with `Authorization: Bearer <token>` |
| Live events | `GET /ws/sync?device=<deviceId>` (WebSocket, same bearer token) |
| Schema | `GET /graphql/sdl`, interactive client at `/graphql/playground` |
| REST docs | `/api/docs`, raw spec at `/api/docs/openapi.json` |

Login lives outside GraphQL on purpose, so the schema is never reachable without a token.

## 1. Sign in

```http
POST /api/v1/login
{ "username": "ada", "passphrase": "…", "label": "My player on Pixel 9" }
```

`label` is the name shown in the account's device list. The answer carries `token`, `username`,
`role`, and the vault fields (`vaultSalt`, `vaultKeyWrapped`) used only for sealed metadata, see
step 5.

- If the account has a second factor the first attempt fails with `totpRequired`. Ask the user
  and send the same request again with `totpCode`. A recovery code is also accepted there.
- A `401` has three meanings (bad credentials, code needed, bad code). The response header
  `X-Agro-Auth-Stage` says which one it is.
- Logins are rate-limited per address, so back off rather than retrying in a loop.
- Store the token in the platform's secure storage. It is the device's credential.

## 2. Check what the server can do

Ask for `capabilities` and `serverVersion` on a node (see step 3). Capabilities are names such
as `catalog.batchPublish` or `vault.backups`. Look for the name you need and fall back when it
is missing; do not compare version numbers. Names are only ever added, never changed.

## 3. Register your device

```graphql
mutation {
  registerNode(userId: "ada", deviceId: "pixel9-player", clientType: "myplayer",
               deviceName: "Pixel 9", version: "0.1.0") { deviceId petname }
}
```

Pick a stable `deviceId` and keep it. Notes:

- `clientType` is your own name for the client: 1-32 characters from `a-z 0-9 . _ -`, lowercased.
  Anything else is rejected. (`wanda` and `wander` are the two first-party names; any value
  containing either collapses to it.) Register before you connect the socket or send a handoff,
  because those only guess a type for a device they have not seen, and never overwrite yours.
- List the account's devices with the `activeNodes` query. A device counts as online for a short
  while after its last report, so heartbeat while playing.

## 4. Hand a session over

Report what you are playing with `updateHandoff`. Send it on track change and as a heartbeat.

```graphql
mutation {
  updateHandoff(input: {
    userId: "ada", deviceId: "pixel9-player",
    trackUri: "subsonic:abc123", trackTitle: "Karma Police", artistName: "Radiohead",
    positionMs: 41000, isPlaying: true,
    queue: [{ trackUri: "subsonic:abc123", trackTitle: "Karma Police", artistName: "Radiohead" }],
    queueIndex: 0
  })
}
```

Read the latest session of your *other* devices:

```graphql
query { playbackHandoff(userId: "ada", excludeDevice: "pixel9-player") {
  trackUri trackTitle artistName positionMs isPlaying queue { trackUri trackTitle } queueIndex
} }
```

- `trackUri` is **your** id for the track and is opaque to Agro. Prefix it with the backend
  (`subsonic:…`, `navidrome:…`, `ytm:…`). A receiving client resolves it against its own backends
  and falls back to title and artist when it does not share the source.
- A heartbeat may omit `queue` to keep the stored queue.
- The queue is capped (the first 100 entries), not rejected.
- Never send a `local:` id. Those are paths on someone's device.

## 5. Listen for changes

Open `GET /ws/sync?device=<deviceId>` with the same bearer header. The events you will see
include `HANDOFF`, `NODE_UPDATE`, `SETTINGS_SYNC`, `LIBRARY_UPDATED`, `SYNC_OFFER`,
`FRIEND_PRESENCE`, `FRIEND_REQUEST` and `LISTEN_ALONG`. Ignore the ones you do not use.

Reconnect with backoff. `HANDOFF` carries the same fields as `playbackHandoff`
(`trackTitle`, `artistName`, `positionMs`, `isPlaying`, `deviceId`). The WebSocket frame envelope
is defined in `src/ws.rs`; read it before parsing.

## 6. Keep it private (optional, recommended)

If the account has a vault key your client can unseal, put the real metadata in `encryptedPayload`
and the server forwards ciphertext it cannot read. A client without the key shows the session as
private. The format and key handling are described in `SECURITY.md` and
`docs/dev/architecture/security-and-vault.md`. Plain, unsealed handoff works without any of it.

## What you do not need

- A Subsonic server. Agro never streams audio. Your client keeps talking to its own music
  backend; Agro only carries state between devices.
- To store the music server's password. Agro can sync that server's address and username between
  a user's devices (sealed), but never the password. It stays on each device.

## Checklist

1. Sign in, store the token.
2. `registerNode` once per install.
3. `updateHandoff` on track change plus a heartbeat.
4. `playbackHandoff(excludeDevice: me)` to offer "continue on this device".
5. WebSocket for live updates.
