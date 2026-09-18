# Agro Developer Documentation Index

This directory contains developer documentation and engineering guides for Agro (self-hosted music sync daemon & web dashboard in Rust), organized following Linux-kernel style modular documentation principles.

## Structure

```
docs/dev/
├── process/
│   ├── coding-style.md          # Rust conventions, Result<T, E> handling & 300-line limit
│   ├── research-workflow.md     # Mandatory 4-step research workflow
│   └── git-and-authorship.md    # Clean commits, CLA Section 8, no AI attribution
├── architecture/
│   ├── overview.md              # Subsystems, embedded dashboard, SQLite storage & Axum routes
│   ├── security-and-vault.md    # Zero-knowledge settings vault, Argon2id, sealed presence
│   ├── boundary-contracts.md    # Boundary test suites (guest & social) and authorization laws
│   └── wanda-parity.md          # Track normalization (src/norm.rs) parity with Wanda
└── tools/
    └── commands-and-testing.md  # Dashboard build (npm), cargo build, test, and clippy gates
```

## Guiding Principles

1. **Lightweight Root Pointer**: Root `AGENTS.md` and `CLAUDE.md` act as fast navigation indexes and invariant guards; deep implementation details live here.
2. **Boundary Suites are Law**: Authorization assertions in boundary test suites must never be loosened.
3. **Zero Knowledge Privacy**: The server routes ciphertext it cannot read and holds encrypted vaults it cannot unwrap.
