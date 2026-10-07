# Song recognition for Wanda via Agro (plan)

Status: plan only. No code has been written in Wanda or Agro.

## Goal

Let Wanda name a song that is not in the user's own library. Today `RecognitionRepository` only matches
against the local library and returns null for anything else, by design.

## Decision

- Recognition of unknown songs runs **on the Agro server only**, through the unofficial Shazam endpoint.
- **No client-side service call, no API key, no key menu** in Wanda.
- Wanda gets **one opt-in toggle** in Privacy, off by default.
- Reason for server-side: the legal and breakage risk of an unofficial endpoint sits with the server
  admin, an endpoint change is fixed by one Agro update instead of an app release, and Wanda stays free
  of any reverse-engineered client (it is EUPL-1.2 and aims at a clean, sovereign image).

## Why not the other options

| Option | Why not |
|---|---|
| AudD | Free for 300 requests only, then about $5 per 1,000 |
| ACRCloud | Small free tier or short trial, limits unconfirmed, HMAC-signed requests |
| AcoustID / MusicBrainz | Free, but matches clean file fingerprints, not noisy microphone audio |
| Client-side Shazam | Puts the unofficial client in the app and spreads the risk to every user |

## Flow

1. User plays unknown music and starts recognition (tile or `RecognitionActivity`).
2. Wanda runs the existing local match first. A hit is returned as today.
3. On a local miss, if the toggle is on and Agro is connected with the feature enabled, Wanda sends about
   10 seconds of 16 kHz mono PCM to `POST /api/v1/recognize` on Agro.
4. Agro turns the clip into a Shazam signature, calls Shazam, and returns the match.
5. Wanda shows the result with a new engine type, so the UI says it came from the server.

If the toggle is off, or Agro is unreachable or has the feature off, nothing leaves the device and the
result stays null.

## Agro work

1. Add `Feature::SongRecognition` in `src/features.rs`. This is the admin switch under
   Management -> Plugins & Rules. A switched-off feature returns `FEATURE_DISABLED`, which Wanda reads
   from `serverFeatures` to hide the toggle. Description must say "unofficial".
2. Add `src/shazam_signature.rs`: FFT peak finding and the signature binary encoding, ported from
   [songrec-rust](https://github.com/BayernMuller/songrec-rust) (GPL-3.0+, compatible with Agro's
   AGPL-3.0). Credit songrec in the file header. Keep it under the 300-line file limit.
3. Add `POST /api/v1/recognize`:
   - authenticated users only, guests refused
   - rate limit with `FixedWindow` from `src/rate_limit.rs`
   - cap clip length and body size
   - never store or log the clip
   - explicit errors, no fake fallbacks
4. Add boundary tests: guest access refused, rate limit enforced, `FEATURE_DISABLED` returned when off.
   Add a unit test for the signature against a known clip.
5. Update the README feature list.

### Dependency decision

The `songrec` crate (v0.7.4) is a desktop app: GTK, cpal and gettext are non-optional dependencies, so it
is unsuitable for a headless server. Port the signature code instead of depending on it. Shelling out to
a `songrec` binary was rejected because every Agro host would have to install it.

## Wanda work

1. Add one Privacy toggle, off by default. Suggested text: "Send a 10-second clip to your Agro server,
   which asks Shazam." It is disabled when Agro is not connected or the feature is off.
2. Add a client call to `/api/v1/recognize` after a local miss in `RecognitionRepository`.
3. Add a new `RecognitionEngine` value for server answers, and show it in the result UI.
4. Reuse the app's existing HTTPS enforcement in `HttpClientFactory`.

## Privacy notes

- With the toggle on, the clip goes phone -> Agro -> Shazam. The Agro admin's server sees the clip, and
  the toggle text must say so.
- Agro must not keep clips. Whether to log who requested a recognition is open (see below).

## Risks

- Shazam can change or block the unofficial endpoint at any time, and the use likely breaches their terms.
  Mitigation: server-side only, admin can switch it off, description says "unofficial".
- Wanda users without Agro get no recognition of unknown songs.

## Open questions

1. Should Agro log which user requested a recognition? Useful against abuse, but it conflicts with the
   zero-telemetry stance.
2. What per-user rate limit is right? A starting point is a small number of requests per minute.

## Process notes

- Agro's rules forbid AI co-author tags in git commits (`CLA.md` section 8).
- Agro's research-before-implementation workflow is covered by the dependency decision above.
