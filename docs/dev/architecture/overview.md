# Agro Architecture Overview

Agro is an open-source, self-hosted music synchronization daemon, social graph, and web dashboard written in Rust.

## 1. Directory Structure

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

## 2. Core Architectural Invariants

- **Dashboard Embedded via `rust-embed`**: The web dashboard is built into static assets and embedded directly into the Rust binary. A fresh workspace requires `npm --prefix dashboard ci && npm --prefix dashboard run build` before `cargo build`.
- **Privacy by Default**: Every social feature defaults to disabled (`off`).
- **Realtime Synchronization**: WebSockets (`/ws/sync`) push cross-device playback handoff, peer presence, synchronized listening jams, and inbox notifications.
