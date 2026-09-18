# Wanda Parity & Track Deduplication

## 1. Synchronization of Normalization Logic

- `src/norm.rs` implements track deduplication, artist matching, and title normalization.
- It is a direct Rust port of Wanda's Android Kotlin `TrackDeduplicator`.
- **CRITICAL INVARIANT**: `src/norm.rs` must remain in exact, lockstep synchronization with Wanda's implementation.
- If normalization logic drifts between Wanda and Agro, diff calculations will produce corrupted match states and mismatched track libraries across client devices.
- Any change to track normalization algorithms must be applied to both Wanda and Agro simultaneously.
