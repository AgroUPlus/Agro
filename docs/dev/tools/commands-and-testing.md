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

## 2. Verification Guidelines for AI Agents

- Use real-time language server diagnostics (`rust-analyzer`) or `cargo check` before running comprehensive tests.
- Always run `cargo test` to verify that `guest_boundary_tests` and `social_boundary_tests` succeed without modification.
