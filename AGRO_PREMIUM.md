# Agro Premium — what to sell, and what to give away

A decision record. Written because "Agro Plus vs Agro Community Edition" is a licensing
question before it is a product question, and the licensing has a deadline the product does not.

---

## The blocker, first

Wanda is **AGPL-3.0**. Agro has **no LICENSE file at all**, and `src/norm.rs` says in its own
header:

> Ported from Wanda's `TrackDeduplicator`, deliberately including its conservatism.

That makes Agro a derivative work of AGPL code. AGPL's §13 is the network clause specifically:
anyone *interacting with the server over a network* must be offered its Corresponding Source. A
closed-source "Agro Premium" that you host is precisely what that forbids.

**This is fixable and cheap, because `git log` shows one author on both repos.** Sole copyright
holder means you can still relicense unilaterally. That stops being true the moment you merge your
first outside PR without a contributor agreement — and it stops being true permanently.

So the deadline is on the paperwork, not on the product.

---

## The decision: one repo, AGPL, sell the hosting

Not open-core. Here is the reasoning, in the order that decides it.

**Real open-core needs the premium code absent from the public repo.** A `cargo` feature flag in a
public repository is not a business boundary — anyone can build with the flag on. To gate at the
code level you need a second repo, or a private crate. That is a permanent tax on a solo
maintainer, and it is the cost you were right to worry about.

**But look at what Agro's sellable features actually are.** `db_jam`, `db_social`, `db_drops`,
`db_feed`, listen-along, presence, the library relay and its spool. Every one of them needs *a
server both people can reach*, with an address, a certificate, uptime, and disk. The value was
never the source code. It is that you run it.

**So the moat is operational, and the code being open costs you nothing.** Someone with the skill
and inclination to self-host was never a customer. They were going to self-host either way, and
having them do it on your code — filing your bugs, on your protocol — is worth more than the
licence fee you would not have collected.

This is Grafana's and Sentry's shape, not MongoDB's.

---

## Do these three things now

1. **Add `LICENSE` to Agro: AGPL-3.0.** This is not a concession, it is a clarification. Agro is
   already AGPL-encumbered through `norm.rs`; today the repo just does not say so, which means it
   currently ships in an unclear state that is worse for you than either answer.

2. **Add a CLA or copyright-assignment policy to both repos.** This is the item with the deadline.
   It costs one file and preserves, forever, your ability to relicense, dual-license, or grant
   yourself a proprietary exception later. Every open-core company does this on day one because it
   cannot be done retroactively.

3. **Keep `norm.rs` as a port, and say so.** Now that both repos are AGPL, the port is simply
   correct. The module's own doc explains why it must not diverge from Wanda's matcher — a
   clean-room rewrite to escape the licence would reintroduce exactly the drift that comment warns
   against, and buy nothing you cannot get from step 2.

---

## What Cloud sells

Same binary, same source, same features. The product is that it is running.

| | Community (self-host) | Agro Cloud |
|---|---|---|
| Features | All of them | All of them |
| Source | AGPL, yours | AGPL, same commit |
| Server | You run it | Runs, backed up, updated |
| Address | Your DNS + TLS | Hostname included |
| Share domain | Your own | `frwd.top` subdomain, or bring your own |
| Library relay | Your disk, your `AGRO_SPOOL_MAX_BYTES` | Quota by plan |
| Friends across NAT | Both ends need reachability | Just works |
| Upgrades | `git pull`, rebuild | Done for you |

The plan axis that follows from the code is **spool budget and relay bandwidth** — those are the
two things in Agro that genuinely cost you money per user (`AGRO_SPOOL_ROOT`,
`AGRO_SPOOL_MAX_BYTES`, `library::begin_upload` / `fetch`). Price on those, not on feature count.
A free tier with a small spool is honest and costs little, because index-only mode
(`AGRO_LIBRARY_ROOT` unset) already exists and never stores bytes.

---

## If you ever do need code-level gating

Do not fork. Use a **private crate** pulled in as an optional dependency: core stays one public
AGPL repo, and the proprietary piece is a small separate crate that only your builds resolve. You
maintain one codebase and one small addition, rather than two diverging servers.

Reserve that for something genuinely separable — an admin console, SSO, a billing integration.
Never for a feature the social graph depends on, because then Community and Cloud stop being the
same program and every bug report becomes "which build?".

---

## What not to do

- **Do not gate with build flags in the public repo.** It is trivially bypassed and reads as
  hostile for no revenue.
- **Do not relicense Wanda to something permissive to "unblock" Agro.** It would let anyone ship a
  closed fork of the *client*, which is the one asset where the code really is the product.
- **Do not add telemetry to measure any of this.** Wanda's CLAUDE.md commits to no telemetry, no
  analytics, no third-party SDK that phones home. A Cloud account is a billing relationship you
  already have; it needs no instrumentation in the client.
