# Coding Style & Architecture Rules

## 1. File Length & Structure

- **Hard cap 300 lines per file.** Split when a file passes 250 lines.
- One concept per file.
- Default to private (`pub(crate)` or unexported). Use `pub` only for genuine cross-module API.

## 2. Robustness & Error Handling

- **No speculative fallbacks.** Return explicit `Result<T, E>`. Never invent placeholder data.
- Never swallow errors with blanket ignores.
- **No dead code.** If an endpoint, helper, or test utility has no caller, it does not get written.

## 3. Dependencies & Pure Rust

- Keep dependencies lean and pure Rust where possible (SQLite is bundled via `rusqlite`).
- Avoid heavy runtime frameworks or unvetted crates.
