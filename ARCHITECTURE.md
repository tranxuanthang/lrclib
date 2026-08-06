# Architecture

## Overview

Rust workspace for the LRCLIB API server.

- Root crate: thin CLI wrapper in `src/main.rs`.
- Main app: `server/` crate.
- Storage: SQLite via `rusqlite` + `r2d2`.
- HTTP: Axum.

Most real behavior lives in `server/`.

## Workspace Layout

- `src/main.rs`: CLI dispatch for `serve` and one-shot maintenance commands.
- `src/backfill_lyricsfiles.rs`: resumable Lyricsfile backfill orchestration, throttling, dry-run reporting, and exclusive process locking.
- `server/src/lib.rs`: thin crate facade that declares modules and re-exports the public server API.
- `server/src/bootstrap.rs`: server startup, listener lifecycle, graceful shutdown, and runtime composition.
- `server/src/metrics.rs`: bounded in-memory aggregate and route-specific latency histograms and interval counters.
- `server/src/state.rs`: `AppState` and construction of shared caches and resources.
- `server/src/router.rs`: Axum route registration, request tracing, metrics middleware, and CORS.
- `server/src/background.rs`: request metrics and recent lyrics counter tasks.
- `server/src/routes/*.rs`: HTTP handlers.
- `server/src/repositories/*.rs`: SQL access layer.
- `server/src/entities/*.rs`: small DB/domain structs.
- `server/src/lyricsfile.rs`: shared Lyricsfile parsing, legacy LRC conversion, YAML generation, and metadata retargeting used by the publish path.
- `server/src/search_index.rs` + `server/src/search_index/*.rs`: embedded Tantivy search index over track metadata (schema, query building, polling indexer).
- `server/src/utils.rs`: input normalization, publish-token verification, cache helpers.
- `server/migrations/*`: embedded SQLite schema/migrations, applied on startup.
- `TANTIVY_SEARCH_PLAN.md`: implementation and rollout record for replacing the `/api/search` FTS5 query path with an embedded Tantivy index.

## Runtime Flow

Server startup in `server::serve`, implemented in `server/src/bootstrap.rs`:

- init tracing from `LRCLIB_LOG`
- open SQLite pool
- apply PRAGMAs + migrations
- bootstrap the Tantivy search index + indexer thread; failures leave `/api/search` unavailable while other routes continue serving
- build shared `AppState`
- spawn background tasks
- build Axum router
- serve on `0.0.0.0:<port>` with graceful shutdown
- after shutdown, stop the search indexer with a final commit

Background tasks:

- request metrics reporter: once per minute logs aggregate HTTP metrics plus `/api/search` and `/api/get` request, cache, failure, and latency metrics
- recent lyrics counter: refreshes `last 10 minutes` publish count once/minute
- search indexer: dedicated OS thread that tails the `tracks` table into the Tantivy index (see Search Index)

## AppState

`AppState`, defined in `server/src/state.rs`, is the shared runtime container.

- `pool`: SQLite connection pool.
- `search_index`: optional embedded Tantivy index (`None` when bootstrap fails); `/api/search` requires it to be ready.
- `challenge_cache`: proof-of-work publish challenges, TTL 5 min.
- `get_metadata_cache`: `Cache<String, Option<TrackResponse>>` with 24h TTL / 4h idle; stores both hits and misses. Uses `try_get_with` for coalescing concurrent requests.
- `search_cache`: cached `/api/search` responses, 24h TTL, 4h idle timeout.
- `load_permits` / `load_queue_timeout`: bounded admission for blocking-load tasks; configurable via `LRCLIB_MAX_CONCURRENT_LOADS` (default 32) and `LRCLIB_LOAD_QUEUE_MS` (default 250). `GET /api/search` and `GET /api/get` return `503 + Retry-After: 1` when the load budget is exhausted.

- Moka caches use AHash for lower-overhead in-memory key hashing.
- `http_metrics`: aggregate HTTP metrics plus `/api/search` and metadata `/api/get` request, cache hit/miss, failure, route-latency, and backend-stage histograms; the reporter snapshots and resets these once per minute.
- `recent_lyrics_count`: recent publish-volume metric used to calculate challenge difficulty.

## Layering

There is no separate service layer.

- route handlers validate + normalize input
- handlers read/write caches when needed
- handlers call repository functions directly
- repositories contain SQL and row mapping

If you need API behavior changes, start in `routes/`.
If you need query/schema changes, start in `repositories/` and `migrations/`.

## API Surface

Routes are mounted under `/api` in `server/src/router.rs`.

- `GET /api/get`: lookup by metadata.
- `GET /api/get/:track_id`: lookup by track id.
- `GET /api/search`: Tantivy-backed search.
- `POST /api/request-challenge`: create proof-of-work challenge.
- `POST /api/publish`: publish lyrics using challenge solution.
- `POST /api/flag`: flag current lyrics for a track.

## Main Request Paths

### `GET /api/get`

- Normalizes input with `utils::process_param`.
- Single-key cache lookup: `get:v4:{track_name_lower}:{artist_name_lower}:{album_name_lower}:{duration_bucket}`. All requests use the cache (including duration-less requests).
- Coalescing via Moka's `try_get_with` on the same cache — concurrent misses for the same key share one database load.
- On cache miss, queries `track_repository::get_track_by_metadata`.
- Both hits (`Some`) and misses (`None`) are cached with 24h TTL / 4h idle.
- Returns the track joined with `tracks.last_lyrics_id`.
- No manual invalidation on publish; eventual consistency via TTL expiry.

### `GET /api/search`

- Normalizes query params.
- Uses `search_cache`.
- Cache keys are versioned as `search:v3`.
- On a cache miss, the Tantivy query and subsequent `track_repository::get_tracks_by_ids` SQLite join run in the same `spawn_blocking` task. The connection is acquired only after Tantivy returns nonempty ranked ids, and response order preserves the ranking.
- If the index is unavailable, still catching up, or a Tantivy query fails, the search request fails; there is no FTS5 query fallback.
- Concurrent misses for the same cache key are coalesced into one database load by Moka.
- Cache-miss and background-refresh searches run through a bounded admission queue on `spawn_blocking`; cache hits remain fully async. The admission semaphore is sized by `LRCLIB_MAX_CONCURRENT_LOADS` (default 32). When saturated, new loads wait up to `LRCLIB_LOAD_QUEUE_MS` ms (default 250, 0 = strict shed) before returning `503 Service Unavailable` + `Retry-After: 1`.

- Once-per-minute metrics report route latency plus `spawn_blocking` wait, Tantivy query, database-pool wait, and SQLite result-fetch latency p50/p95/p99, along with requests, cache hits/misses, and 5xx failures. Backend-stage histograms contain actual coalesced loader executions, not every request that observed an initial cache miss.

### `POST /api/request-challenge`

- Generates a random prefix.
- Computes difficulty from recent publish volume.
- Stores `challenge:<prefix> -> target` in `challenge_cache`.

### `POST /api/publish`

- Requires `X-Publish-Token`.
- Token format is `prefix:nonce` and is verified against the cached challenge by SHA-256 threshold comparison.
- In one DB transaction, find or insert the track, then insert a new lyrics row.
- DB trigger updates `tracks.last_lyrics_id`.
- If `lyricsfile` is supplied, it is stored as raw YAML, legacy `plainLyrics` / `syncedLyrics` inputs are ignored, and stored legacy columns are derived best-effort from the Lyricsfile payload.
- If only legacy lyrics are supplied, the shared converter generates Lyricsfile YAML before the transaction. It preserves plain text, converts LRC timestamps to millisecond `lines`, sorts lines, infers each `end_ms` from the next line, and retains the request duration at millisecond precision.
- If only synced lyrics are supplied, plain lyrics are derived while preserving stanza separators and untimed text.
- Instrumental markers (`[au: instrumental]`) produce a lyrics row with no text and `instrumental = true`.
- For `lyricsfile` publishes, `instrumental` is populated by a best-effort YAML read of `metadata.instrumental`; instrumental rows have no stored plain or synced text.
- Repository insert functions require a non-empty Lyricsfile, so all current API write paths set `has_lyricsfile = true`.
- Metadata cache entries for the published track expire naturally via TTL (24h); publish does not actively invalidate them.

### `POST /api/flag`

- Reuses the same publish-token validation.
- Inserts into `flags` for the track's current `last_lyrics_id`.

## Maintenance CLI

`lrclib backfill-lyricsfiles --database <FILE>` populates Lyricsfile for historical rows where `has_lyricsfile = false`.

## Data Model

Core schema is in `server/migrations/`.

- `tracks`: canonical track metadata plus normalized `*_lower` columns and `last_lyrics_id`.
- `lyrics`: lyrics versions linked to a track; latest version is selected through `tracks.last_lyrics_id`. It also stores raw `lyricsfile` YAML plus `has_lyricsfile` for efficient presence/count queries. The column remains nullable for historical rows, but current repository writes require it.
- `flags`: reports against a lyrics row.
- `missing_tracks`: table created by migration 04; no code writes to it anymore.
- `tracks_fts`: retained FTS5 virtual table over normalized track fields; it is not queried by `/api/search`.

Important DB behavior:

- `tracks.last_lyrics_id` is maintained by an `AFTER INSERT ON lyrics` trigger.
- FTS rows are maintained by insert/update/delete triggers on `tracks`.
- track uniqueness is enforced on normalized `(name_lower, artist_name_lower, album_name_lower, duration)`.
- the uniqueness constraint's autoindex covers full metadata lookups; `idx_tracks_metadata_no_album` separately supports metadata lookups that omit the album but include duration.
- DB opens in WAL mode with tuned PRAGMAs in `server/src/db.rs`; connection-local PRAGMAs and a busy timeout are applied to every pooled connection. Startup logs the linked SQLite version plus the effective and compiled maximum mmap sizes, with all SQLite compile options available at debug level. `LRCLIB_MMAP_SIZE` and `LRCLIB_CACHE_SIZE` override the mmap and suggested page-cache limits, while `LRCLIB_POOL_MAX_SIZE` sets the r2d2 connection limit (default 16).
- `LRCLIB_MAX_CONCURRENT_LOADS` (default 32) and `LRCLIB_LOAD_QUEUE_MS` (default 250) control the blocking-load admission. `LRCLIB_MAX_BLOCKING_THREADS` optionally caps the tokio blocking thread pool (only when explicitly set; default follows tokio). When permitted loads are exhausted, `/api/get` and `/api/search` return `503 + Retry-After: 1`.
- `idx_lyrics_has_lyricsfile` supports efficient counts and filters over Lyricsfile adoption.

Current lyrics API responses remain backward-compatible:

- `plainLyrics` and `syncedLyrics` are still returned for legacy rows
- `lyricsfile` is returned as an additional raw YAML field when present

## Search Index (Tantivy)

`server/src/search_index.rs` embeds a Tantivy index over `(track_id, name_lower, artist_name_lower, album_name_lower)`. It is the sole search backend for `/api/search` (see `TANTIVY_SEARCH_PLAN.md`).

- SQLite remains the source of truth; the index is a disposable derived artifact under `<db_path>.tantivy/` (override: `LRCLIB_SEARCH_INDEX_DIR`).
- The indexer is a dedicated OS thread that polls `tracks` for `id > cursor` (default every 2s, batches of 10k). This is race-free because `tracks.id` is AUTOINCREMENT-monotonic and the indexed name columns are immutable after insert; there is no hook in the publish path.
- The cursor (last indexed track id) is stored as the Tantivy commit payload, so it is atomic with each commit. Restarts replay `id > cursor`; crashes lose only uncommitted work that the replay re-indexes.
- `SearchIndex::is_ready()` turns true once the indexer has caught up. Search requests fail until then; missing/corrupt/version-mismatched indexes rebuild in the background while non-search routes continue serving.
- The index format version lives in `lrclib_index_version` inside the index dir; bump `INDEX_FORMAT_VERSION` when the schema/tokenizer changes to force a rebuild. Recreation refuses to wipe directories that do not look like a search index.
- Query semantics: `q` = AND of terms across the three columns; structured params = per-column phrase queries. Inputs are expected to be normalized by `utils::prepare_input`.
- Tantivy emits its own `tracing` logs; use e.g. `LRCLIB_LOG=info,tantivy=warn` to quiet them.

## Matching Rules

Track matching is intentionally fuzzy but narrow:

- names are normalized by `utils::prepare_input`
- punctuation is mostly removed/collapsed
- comparisons are lowercase
- metadata duration matching uses about +/-2 seconds
- `/api/get` prefers tracks whose current lyrics include synced lyrics

If you change matching behavior, review both:

- `utils::prepare_input`
- repository metadata queries
- `search_index/query.rs`

## Agent Notes

- Most edits belong in `server/`, not the root crate.
- Preserve write transactions around track + lyrics creation.
- All new lyrics repository writes must supply a non-empty Lyricsfile; old rows may still contain `NULL`.
- Historical Lyricsfile population belongs in the root maintenance CLI; keep schema-aware batch SQL and its timestamp-invariance tests in `lyrics_repository`.
- Schema changes require a new migration and usually repository updates.
