# Agro

Self-hosted music sync daemon, social graph, and web dashboard in Rust. Central sync and relay backend for **Wanda** (Android) and **Wander** (TUI). Battery-first, privacy-first, zero telemetry.

## 1. Non-Negotiable Hard Rules

| Rule | Why |
| --- | --- |
| **Boundary suites are law** | `guest_boundary_tests` & `social_boundary_tests` are inviolable; never loosen assertions. |
| **Zero-knowledge vault** | Server stores `vault_salt` / wrapped key under Argon2id; never holds plaintext keys. |
| **Sealed presence & handoff** | Client metadata in transit is end-to-end sealed; server routes ciphertext it cannot read. |
| **`src/norm.rs` Wanda parity** | Direct port of Wanda's `TrackDeduplicator`; must remain in exact lockstep. |
| **Dashboard builds before binary** | `cargo build` embeds `dashboard/dist/`; compile web assets before server. |
| **300 lines max per file** | Split when file reaches 250 lines; 1 concept per file. |
| **No dead code & no fake fallbacks** | Surface explicit errors; never swallow exceptions with generic catch. |
| **Hashed secrets only** | Argon2id for passphrases, SHA-256 for device tokens; never log credentials. |
| **Research before implementation** | Follow mandatory 4-step research workflow before adding dependencies. |
| **No AI attribution in git** | Comply with `CLA.md` Section 8; never add `Co-Authored-By` AI tags. |

## 2. Documentation Directory Map

Detailed developer guides and architectural specifications in `docs/dev/`:

| Topic | Pointer / Specification |
| --- | --- |
| **Coding Style & Conventions** | [`docs/dev/process/coding-style.md`](docs/dev/process/coding-style.md) |
| **Mandatory Research Workflow** | [`docs/dev/process/research-workflow.md`](docs/dev/process/research-workflow.md) |
| **Git & Authorship Policy** | [`docs/dev/process/git-and-authorship.md`](docs/dev/process/git-and-authorship.md) |
| **Architecture & Subsystems** | [`docs/dev/architecture/overview.md`](docs/dev/architecture/overview.md) |
| **Security & Zero-Knowledge Vault** | [`docs/dev/architecture/security-and-vault.md`](docs/dev/architecture/security-and-vault.md) |
| **Boundary Test Contracts** | [`docs/dev/architecture/boundary-contracts.md`](docs/dev/architecture/boundary-contracts.md) |
| **Wanda Parity & Normalization** | [`docs/dev/architecture/wanda-parity.md`](docs/dev/architecture/wanda-parity.md) |
| **Commands & Testing** | [`docs/dev/tools/commands-and-testing.md`](docs/dev/tools/commands-and-testing.md) |

## 3. Quick Commands

```bash
cd dashboard && npm ci && npm run build && cd ..   # Build web dashboard
cargo build --release                             # Compile server binary
cargo test                                        # Run test suite & boundary checks
cargo clippy --all-targets                        # Lint checks
```
