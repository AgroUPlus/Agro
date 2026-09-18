# Boundary Contracts & Authorization Law

## 1. Boundary Suites are Law

- The integration test suites `guest_boundary_tests.rs` and `social_boundary_tests.rs` define the non-negotiable authorization and access contract of the Agro server.
- **CRITICAL INVARIANT**: Any Pull Request or code change that loosens an assertion to make a test pass is strictly invalid.
- Guest users must have zero unauthorized read access to private libraries, listening history, or unshared device metadata.
- Social boundary tests verify that friendship barriers, blocked users, and privacy toggles cannot be bypassed via GraphQL or REST.
