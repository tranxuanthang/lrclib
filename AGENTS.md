This is the public, self-hostable LRCLIB server written in Rust with Axum and SQLite.

The workspace has `core/` (`lrclib-core`) for persistence and domain code and `server/` (`lrclib-server`) for the HTTP runtime. Hosted-only queue, gateway, rate-limit, and wiretap crates are intentionally absent.

Before working, read the root `ARCHITECTURE.md`.

After changes, update `ARCHITECTURE.md` when architecture or runtime behavior changes.
