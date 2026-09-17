# Agro

Open-source, self-hosted music sync daemon, social graph, and web dashboard in Rust.
Serves as the central synchronization and relay backend for **Wanda** (Android) and **Wander** (TUI).
Battery-first, privacy-first, zero-telemetry, and zero-knowledge encrypted where it matters.

## Architecture

```
src/
  db/           rusqlite storage: accounts, devices, history, popularity, library index
  schema/       async-graphql schemas (social, popularity, library, devices)
  api/          axum REST routes: auth, bootstrap, relay, upload, listen/share links
  ws/           WebSocket live push (/ws/sync): handoff, presence, jams, notifications
  norm.rs       Track deduplication & title normalization (mirrors Wanda's TrackDeduplicator)
  plugins.rs    Server capability plugins
dashboard/      React + Vite administrative and web dashboard, embedded via rust-embed
```

Rules of the road:

- **Privacy first and non-negotiable.** Every social feature defaults to off.
- **Boundary suites are law.** `guest_boundary_tests.rs` and `social_boundary_tests.rs` define the
  authorization contract under `cargo test`. A PR that loosens an assertion to pass is invalid.
- **Zero-knowledge settings vault.** The server holds `vault_salt` and `vault_key_wrapped` sealed
  under Argon2id(passphrase). The server must never receive or store anything that can unwrap the vault.
- **Sealed presence and handoff.** Metadata in transit across friends or devices is end-to-end sealed;
  the server routes ciphertext it cannot read.
- **`src/norm.rs` must stay in step with Wanda.** It is a direct port of Wanda's `TrackDeduplicator`.
  If normalisation drifts, library diffs produce corrupted matches across devices. Change both or neither.
- **The dashboard builds before the binary.** `cargo build` embeds `dashboard/dist/` via `rust-embed`.
  A fresh workspace requires `npm --prefix dashboard ci && npm --prefix dashboard run build`.

## Coding style

- **Hard cap 300 lines per file.** Split when a file passes 250 lines. One concept per file.
- **No speculative fallbacks.** Return explicit `Result<T, E>`. Never invent placeholder data, never
  swallow errors with blanket ignores.
- **No dead code.** If an endpoint, helper, or test utility has no caller, it does not get written.
- Default to private (`pub(crate)` or unexported). `pub` only for genuine cross-module API.
- Keep dependencies lean and pure Rust where possible (SQLite is bundled).

## Security

- Secrets (passphrases, tokens) are never logged and never stored in plain text.
- Passphrases stored as **Argon2id** hashes; device tokens as **SHA-256** hashes.
- Device tokens are revocable and expire after idle duration.
- The archive hook receives parameters via environment variables, never shell arguments.
- No analytics, telemetry, or third-party phone-home SDKs.

## AI Tools, Authorship and Non-Appropriation

- AI models and automated code agents are assistive utilities only. They are **not** authors or contributors.
- **Do not add** `Co-Authored-By` trailers or metadata referencing AI models to git commit messages.
- Any use of AI tools must strictly adhere to [`CLA.md`](CLA.md) Section 8. No AI vendor or automated system acquires ownership, copyright, or licensing claims over project code.

## Commands

```bash
cd dashboard && npm ci && npm run build   # build dashboard
cargo build --release                    # compile server binary
cargo test                               # run test suite & boundary checks
cargo clippy --all-targets               # linter checks
cargo fmt --check                        # style checks
```

## Research Before Implementation (MANDATORY)

Before implementing code, researching code, or answering technical questions, the AI agent MUST follow this research workflow:

### Step 1: Look up official documentation
- Use documentation tools or MCP servers to fetch up-to-date documentation for any library/framework about to be used.
- Understand the latest API surface, breaking changes, and recommended usage patterns.

### Step 2: Evaluate pros, cons, and alternatives
- Use web search to research:
  - Pros and cons of the library/approach
  - Alternative libraries or approaches that solve the same problem
  - Known issues, performance concerns, or deprecation notices
- Compare and evaluate whether the chosen library/approach is the best fit for this project.

### Step 3: Study OSS best practices
- Search well-known open-source projects to see how they implement similar features.
- Verify the approach follows established best practices before adopting it.
- Pay attention to patterns used in projects with similar architecture.

### Step 4: Make a decision and justify
- Only proceed with implementation after completing steps 1-3.
- If a library/approach has significant drawbacks or better alternatives exist, recommend the better option to the user before proceeding.
- Document the rationale briefly when introducing new dependencies or patterns.

## Verification After Code Changes

- Use real-time language server diagnostics (`rust-analyzer`) or `cargo check` before running full tests.
- Run `cargo test` and `cargo clippy` to verify boundary tests and clean code before concluding work.
