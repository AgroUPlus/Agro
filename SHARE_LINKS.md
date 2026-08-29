# The share-link protocol

**Version 1.** Normative. Three codebases implement this, and none of them can see the others:

| Role | Implementation |
|---|---|
| **Minter** — builds links | Wanda, `ShareLinkRewriter.kt` · Wander |
| **Forwarder** — resolves them for a visitor | Agro, `src/listen.rs` · frwd.top, `listen/index.html` |
| **Receiver** — opens them in the app | Wanda, `MainActivity` + `LinkRepository.kt` |

The parameter names, their bounds, and the refusal rules are duplicated in Kotlin, Rust and
JavaScript. Nothing in any toolchain will catch them drifting apart. This document is the only
thing that holds them together, so a change here is a change to all three or to none.

The key words MUST, MUST NOT, SHOULD and MAY are to be read as in RFC 2119.

---

## 1. Why links are wrapped at all

A backend's own share URL names the backend. Sending someone a `navidrome.example.com` link
publishes the address of a private server to whoever the link reaches next; a `music.youtube.com`
link hands them to Google. Wrapping puts an address the user controls in front, and lets one link
serve a visitor with no app and a visitor with the app installed.

Wrapping is best-effort and silent. A link that cannot be carried is shared exactly as its backend
minted it, with no warning and no refusal.

---

## 2. Where a link is minted — the base

A minter MUST choose the first of these that is available:

1. **The user's own domain**, when set. Scheme MUST be `https`.
   → `https://<domain>/listen`
2. **Their Agro server**, when one is paired. Scheme MUST be whatever the server was configured
   with, so the link names an address the server actually answers on.
   → `<scheme>://<authority>/listen`

   In practice that is `https`. Wanda's `network_security_config.xml` permits cleartext only for
   `localhost`, `127.0.0.1` and `10.0.2.2`, so a plain-`http` Agro is unreachable from the app and
   never becomes a configured server; the `http` case is the `adb reverse` development flow. A
   self-hosted Agro needs a certificate.
3. **Nothing.** The backend's own URL is shared unchanged.

Tier 2 requires no custom domain and no additional endpoint: Agro routes `/listen` itself.

## 3. The endpoint

```
GET /listen?<target>[&s=<speed>&p=<pitch>]
```

### 3.1 Target parameters — exactly one

| Param | Value | Notes |
|---|---|---|
| `id` | Agro short-link UID | Minted by `createShortLink`. Resolved by `resolveShortLink`. Preferred whenever Agro is paired: it keeps raw URLs and video ids out of the query string, and it is the only form Agro's link manager can list, count or revoke. |
| `v` | YouTube video id | Exactly **11** characters of URL-safe base64. Carried in the open — it is public already, and it is what makes a shared link readable. |
| `u` | Any other track URL, percent-encoded | Checked against the allowlist in §5 before use. |

A forwarder that receives none of these, or more than one, MUST refuse (§6).

### 3.2 Playback parameters — both or neither

| Param | Value |
|---|---|
| `s` | Playback rate |
| `p` | Pitch, independent of rate |

- Both MUST be present, or neither. A lone `s` or `p` MUST be ignored.
- Both MUST lie in **`[0.5, 2.0]`** inclusive.
- They MUST be omitted entirely when both are `1.0`, so an ordinary share stays an ordinary URL.
- Out-of-range values MUST be dropped, NOT clamped. A link asking for 40× is not a link whose
  intent can be recovered, and clamping invents one.

Rationale: sharing a track you are playing at 1.25× and a tone lower is sharing *that* — the
version you meant, not the one the file happens to hold.

**These bounds are the player's, not the protocol's.** They are `SpeedAndPitch.RANGE` in Wanda and
`RATE_RANGE` in Agro, and they must be changed together.

### 3.3 Reserved

Any parameter not listed above is unassigned. A forwarder MUST NOT pass an unrecognised parameter
onward (§4). Future parameters take a new name; none is ever reinterpreted.

---

## 4. Rebuild, never pass through

A forwarder constructs the query string it hands onward **from the parameters it recognised and
validated**. It MUST NOT forward the raw query string it received.

This is not tidiness. The onward query string is placed into a `wanda://` URL and an `intent://`
URL, and forwarding it verbatim would put whatever a stranger appended into the app's own
addressing — the same open-redirect mistake §5 exists to prevent for `u`.

---

## 5. The allowlist — forwarders are not open redirects

A forwarder MUST NOT send a visitor to an address merely because it was handed one. Before using
a `u` target it MUST check:

- Scheme is `https`. An `http` target is a downgrade the recipient never agreed to.
- Host, after stripping `www.` and `m.`, is one of:
  - YouTube: `youtube.com`, `music.youtube.com`, `youtu.be`
  - the configured Navidrome server
  - a host an account on this server has allowed

A forwarder that will send a visitor to any address given to it is a phishing URL wearing the
user's domain, with that domain's reputation paying for it.

The **minter** applies the same allowlist before wrapping, and the **receiver** applies it again
after unwrapping. All three check, because each is reachable without the others: a short link is a
URL a stranger can type.

---

## 5a. Escaping — the allowlist checks the host, not the rest

A forwarder that renders the target into a page MUST HTML-escape it, and MUST escape `</` in any
JSON it writes into a `<script>` block.

The allowlist in §5 validates the **authority** of a URL. It says nothing about the path or the
query, so `?u=https://youtube.com/"><script>…` passes the host test carrying whatever it likes.
Interpolated raw into an `href`, that is script execution on the forwarder's own origin — which,
for Agro, is where the dashboard keeps its device token.

The same applies to anything else rendered: a track title is library metadata, read from a file's
tags or from a backend, and never authored by the server showing it.

## 6. Refusal

A forwarder that cannot resolve a link MUST serve a human-readable refusal page. It MUST NOT
redirect anywhere, and MUST NOT reveal why beyond "this link is not one we forward".

---

## 7. Reaching the app

A forwarder MUST NOT rely on domain verification: the user's domain is set at runtime and an
Android intent filter is not, so no app claims `https://<their-domain>/listen` in advance.

It therefore offers the app its own scheme, carrying the rebuilt query string from §4:

```
wanda://listen?<rebuilt>
intent://listen?<rebuilt>#Intent;scheme=wanda;package=com.wander.android;S.browser_fallback_url=<target>;end
```

A receiver MUST accept a link on any host it mints on (§2) — its own domain *and* its Agro
server, over `http` or `https` — or it will mint links it cannot open.

---

## 8. Privacy

- A forwarder MUST NOT log the visitor. No log line, no counter keyed to a person, no cookie. A
  shortener is a redirect log by construction, and these players are built on the premise that
  nobody keeps one.
- Aggregate counts against the *link* (not the visitor) are permitted; Agro's short links keep
  one.
- `Referrer-Policy: no-referrer` and `robots: noindex, nofollow` MUST be set on the page.

---

## 9. Conformance

| Rule | Wanda | Agro | frwd.top |
|---|---|---|---|
| §2 tier order | ✅ `shareBase()` | n/a | n/a |
| §3.1 target params | ✅ | ✅ | ✅ |
| §3.2 `s`/`p` bounds | ✅ `SpeedAndPitch.RANGE` | ✅ `RATE_RANGE` | ✅ `rebuiltSearch()` |
| §4 rebuild not pass-through | n/a | ✅ | ✅ `rebuiltSearch()` |
| §5 allowlist | ✅ `isAllowed()` | ✅ `resolve()` | ✅ `ALLOWED_HOSTS` |
| §5a escaping | n/a | ✅ `escape_html()` | ✅ DOM APIs, no `innerHTML` |
| §7 accepts own hosts | ✅ `ownHosts()` | n/a | n/a |
| §8 no visitor log | n/a | ✅ | ✅ |

All three conform as of version 1.

One caveat on frwd.top: being static, it cannot consult a server for which hosts an account has
allowed, so its `ALLOWED_HOSTS` is a literal list in the page. It ships with YouTube's hosts only.
**A self-hosted Navidrome must be added to that list by hand**, or `?u=` links pointing at it will
be refused. This is the deliberate trade: before this, the page forwarded to any host it was
given, which is an open redirect on the domain.
