# Commands, Builds & Quality Gates

## 1. Build and Test Sequence

The web dashboard must be compiled before compiling the Rust binary:

```bash
# 1. Build web dashboard assets
cd dashboard && npm ci && npm run build && cd ..

# 2. Compile server binary
cargo build --release

# 3. Execute test suite and boundary suites
cargo test

# 4. Run static analysis and style checks
cargo clippy --all-targets
cargo fmt --check
```

## 2. Additional Gates

```bash
cargo deny check                                          # advisories, licenses, sources (deny.toml)
UPDATE_SNAPSHOT=1 cargo test schema_snapshot              # regenerate docs/api/schema.graphql after an intended API change
```

- `clippy::unwrap_used` is denied outside tests (`Cargo.toml`, `clippy.toml`). Lock a `Mutex` or `RwLock` with `lock_recover()` / `read_recover()` / `write_recover()` from `src/lock.rs`, which survive a poisoned lock.
- `src/property_tests.rs` holds `proptest` properties for the normaliser, audit IP truncation and token helpers.
- `docs/api/schema.graphql` is the committed GraphQL schema Wanda and Wander build against. The snapshot test fails when it drifts.
- `fuzz/` holds `cargo-fuzz` targets (nightly): `cd fuzz && cargo +nightly fuzz run normalise` or `credentials`. They include `src/norm.rs` and `src/credentials.rs` directly, so those two files must stay free of `crate::` imports.
- Boundary suites are split by topic under `src/*_boundary_tests/`; moving a test is fine, changing an assertion is not.

## 3. Verification Guidelines for AI Agents

- Use real-time language server diagnostics (`rust-analyzer`) or `cargo check` before running comprehensive tests.
- Always run `cargo test` to verify that `guest_boundary_tests` and `social_boundary_tests` succeed without modification.
