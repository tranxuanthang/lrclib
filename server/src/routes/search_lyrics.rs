use crate::{
    entities::track::SimpleTrack, errors::ApiError, errors::OverloadedError,
    metrics::MonitoredRoute,
    repositories::track_repository::get_tracks_by_ids, utils::process_param, AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Instant};

// Query parameters
#[derive(Serialize, Deserialize)]
pub struct QueryParams {
    q: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrackResponse {
    id: i64,
    name: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CachedResult {
    tracks: Vec<TrackResponse>,
    created_at: DateTime<Utc>,
}

pub async fn route(
    Query(params): Query<QueryParams>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TrackResponse>>, ApiError> {
    let q = process_param(params.q.as_deref());
    let track_name = process_param(params.track_name.as_deref());
    let artist_name = process_param(params.artist_name.as_deref());
    let album_name = process_param(params.album_name.as_deref());

    // Generate a cache key based on query parameters
    let cache_key = format!(
        "search:v3:{}:{}:{}:{}",
        q.as_deref().unwrap_or_default(),
        track_name.as_deref().unwrap_or_default(),
        artist_name.as_deref().unwrap_or_default(),
        album_name.as_deref().unwrap_or_default()
    );

    let cached_result = state.search_cache.get(&cache_key).await;

    if let Some(cached_result) = cached_result {
        state.http_metrics.record_cache_hit(MonitoredRoute::Search);
        return Ok(Json(cached_result.tracks));
    }

    state.http_metrics.record_cache_miss(MonitoredRoute::Search);

    let loader_state = Arc::clone(&state);
    let cached_result = state
        .search_cache
        .try_get_with(cache_key, async move {
            load_tracks(loader_state, q, track_name, artist_name, album_name).await
        })
        .await
        .map_err(|error| {
            if error.downcast_ref::<OverloadedError>().is_some() {
                state.http_metrics.record_overload(MonitoredRoute::Search);
                ApiError::OverloadedError
            } else {
                ApiError::UnknownError(anyhow::anyhow!(error.to_string()))
            }
        })?;

    Ok(Json(cached_result.tracks))
}

fn create_response(tracks: Vec<SimpleTrack>) -> Vec<TrackResponse> {
    tracks
        .iter()
        .map(|track| {
            let plain_lyrics = match track.last_lyrics {
                Some(ref lyrics) => lyrics.plain_lyrics.to_owned(),
                None => None,
            };

            let synced_lyrics = match track.last_lyrics {
                Some(ref lyrics) => lyrics.synced_lyrics.to_owned(),
                None => None,
            };

            let instrumental = match track.last_lyrics {
                Some(ref lyrics) => lyrics.instrumental.to_owned(),
                None => false,
            };

            let lyricsfile = match track.last_lyrics {
                Some(ref lyrics) => lyrics.lyricsfile.to_owned(),
                None => None,
            };

            TrackResponse {
                id: track.id,
                name: track.name.to_owned(),
                track_name: track.name.to_owned(),
                artist_name: track.artist_name.to_owned(),
                album_name: track.album_name.to_owned(),
                duration: track.duration,
                instrumental,
                plain_lyrics,
                synced_lyrics,
                lyricsfile,
            }
        })
        .collect()
}

async fn load_tracks(
    state: Arc<AppState>,
    q: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
) -> anyhow::Result<CachedResult> {
    #[cfg(test)]
    state
        .search_load_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let tracks =
        load_tracks_from_search_index(&state, q, track_name, artist_name, album_name).await?;

    Ok(CachedResult {
        tracks: create_response(tracks),
        created_at: Utc::now(),
    })
}

/// Searches through the embedded Tantivy index and joins the ranked ids to
/// their current track and lyrics data.
async fn load_tracks_from_search_index(
    state: &Arc<AppState>,
    q: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
) -> anyhow::Result<Vec<SimpleTrack>> {
    let search_index = state
        .search_index()
        .ok_or_else(|| anyhow::anyhow!("search index is unavailable"))?;
    if !search_index.is_ready() {
        anyhow::bail!("search index is not ready");
    }

    let search_index = Arc::clone(search_index);
    let state = Arc::clone(state);
    let blocking_started_at = Instant::now();
    let permit = state.acquire_load_permit().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .http_metrics
            .record_blocking_wait_latency(MonitoredRoute::Search, blocking_started_at.elapsed());

        let tantivy_started_at = Instant::now();
        let track_ids = search_index.search(
            q.as_deref(),
            track_name.as_deref(),
            artist_name.as_deref(),
            album_name.as_deref(),
        );
        state
            .http_metrics
            .record_tantivy_latency(tantivy_started_at.elapsed());
        let track_ids = track_ids?;

        if track_ids.is_empty() {
            return Ok(Vec::new());
        }

        let db_pool_started_at = Instant::now();
        let conn = state.db_connection();
        state
            .http_metrics
            .record_db_pool_wait_latency(MonitoredRoute::Search, db_pool_started_at.elapsed());
        let mut conn = conn?;
        let sql_started_at = Instant::now();
        let tracks = get_tracks_by_ids(&track_ids, &mut conn);
        state
            .http_metrics
            .record_sql_latency(MonitoredRoute::Search, sql_started_at.elapsed());
        tracks
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::repositories::{lyrics_repository, track_repository};
    use crate::search_index::{spawn_indexer, IndexerConfig, IndexerHandle, SearchIndex};
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tokio::sync::Barrier;
    use uuid::Uuid;

    struct SearchIndexTestState {
        state: Arc<AppState>,
        db_path: PathBuf,
        index_dir: PathBuf,
        indexer: Option<IndexerHandle>,
    }

    impl SearchIndexTestState {
        fn cleanup(mut self) {
            self.indexer.take();
            remove_test_database(&self.db_path);
            let _ = std::fs::remove_dir_all(&self.index_dir);
        }
    }

    /// Builds a state whose Tantivy index has already caught up with the
    /// tracks seeded by `seed`.
    async fn test_state_with_ready_search_index(
        seed: impl FnOnce(&mut rusqlite::Connection),
    ) -> SearchIndexTestState {
        let base = std::env::temp_dir().join(format!("lrclib-test-{}", Uuid::new_v4()));
        let db_path = PathBuf::from(format!("{}.sqlite3", base.display()));
        let index_dir = PathBuf::from(format!("{}.tantivy", base.display()));

        let pool = init_db(&db_path).expect("failed to initialize test database");
        {
            let mut conn = pool.get().unwrap();
            seed(&mut conn);
        }

        let search_index =
            Arc::new(SearchIndex::open_or_create(&index_dir).expect("failed to open search index"));
        let indexer = spawn_indexer(
            Arc::clone(&search_index),
            pool.clone(),
            IndexerConfig {
                poll_interval: Duration::from_millis(25),
                writer_memory_bytes: 32 * 1024 * 1024,
                ..IndexerConfig::default()
            },
        );

        tokio::time::timeout(Duration::from_secs(5), async {
            while !search_index.is_ready() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("search index did not become ready");
        search_index
            .reload_reader()
            .expect("failed to reload search index reader");

        let state = crate::state::build_app_state(pool, Some(search_index));

        SearchIndexTestState {
            state,
            db_path,
            index_dir,
            indexer: Some(indexer),
        }
    }

    fn remove_test_database(path: &PathBuf) {
        for path in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }


    #[tokio::test]
    async fn concurrent_cache_misses_share_one_search_load() {
        const REQUESTS: usize = 16;

        let test = test_state_with_ready_search_index(|conn| {
            track_repository::add_one(
                "Single Flight Song",
                "Test Artist",
                "Test Album",
                180.0,
                conn,
            )
            .unwrap();
        })
        .await;
        let state = Arc::clone(&test.state);

        let barrier = Arc::new(Barrier::new(REQUESTS + 1));
        let mut tasks = Vec::with_capacity(REQUESTS);

        for _ in 0..REQUESTS {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                route(
                    Query(QueryParams {
                        q: Some("Single Flight Song".to_owned()),
                        track_name: None,
                        artist_name: None,
                        album_name: None,
                    }),
                    State(state),
                )
                .await
            }));
        }

        barrier.wait().await;
        for task in tasks {
            let response = task
                .await
                .expect("search task failed")
                .unwrap_or_else(|_| panic!("search failed"));
            assert_eq!(response.0.len(), 1);
        }
        assert_eq!(state.search_load_count.load(Ordering::Relaxed), 1);

        drop(state);
        test.cleanup();
    }


    #[tokio::test]
    async fn tantivy_results_are_fetched_and_cached() {
        let test = test_state_with_ready_search_index(|conn| {
            let track_id = track_repository::add_one(
                "Benchmark Song",
                "Test Artist",
                "Test Album",
                180.0,
                conn,
            )
            .unwrap();
            lyrics_repository::add_one(
                &Some("Benchmark lyrics".to_owned()),
                &None,
                "version: '1.0'\nmetadata:\n  title: Benchmark Song\n  artist: Test Artist\n  instrumental: false\nlines: []\nplain: Benchmark lyrics\n",
                track_id,
                false,
                &Some("test".to_owned()),
                conn,
            )
            .unwrap();
        })
        .await;

        let response = route(
            Query(QueryParams {
                q: Some("Benchmark Song".to_owned()),
                track_name: None,
                artist_name: None,
                album_name: None,
            }),
            State(test.state.clone()),
        )
        .await
        .unwrap_or_else(|_| panic!("tantivy-backed search failed"));

        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].track_name.as_deref(), Some("Benchmark Song"));
        assert_eq!(
            response.0[0].plain_lyrics.as_deref(),
            Some("Benchmark lyrics")
        );
        assert!(test
            .state
            .search_cache
            .get("search:v3:benchmark song:::")
            .await
            .is_some());

        test.cleanup();
    }

    #[tokio::test]
    async fn tantivy_results_preserve_rank_order() {
        let test = test_state_with_ready_search_index(|conn| {
            track_repository::add_one(
                "Hello World Extra Tokens Here",
                "Artist",
                "Album",
                180.0,
                conn,
            )
            .unwrap();
            track_repository::add_one("Hello", "Artist", "Album", 200.0, conn).unwrap();
        })
        .await;

        let response = route(
            Query(QueryParams {
                q: Some("Hello".to_owned()),
                track_name: None,
                artist_name: None,
                album_name: None,
            }),
            State(test.state.clone()),
        )
        .await
        .unwrap_or_else(|_| panic!("tantivy-backed search failed"));

        // BM25 ranks the exact title (id 2) above the longer one (id 1), while
        // a bare SQL `IN` lookup would return id order; the response must
        // follow the ranking.
        assert_eq!(
            response.0.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![2, 1]
        );

        test.cleanup();
    }

    #[tokio::test]
    async fn search_index_not_ready_returns_an_error() {
        let base = std::env::temp_dir().join(format!("lrclib-test-{}", Uuid::new_v4()));
        let db_path = PathBuf::from(format!("{}.sqlite3", base.display()));
        let index_dir = PathBuf::from(format!("{}.tantivy", base.display()));

        let pool = init_db(&db_path).expect("failed to initialize test database");
        {
            let mut conn = pool.get().unwrap();
            track_repository::add_one("Unavailable Song", "Artist", "Album", 180.0, &mut conn)
                .unwrap();
        }

        // Index attached but no indexer running: never becomes ready.
        let search_index =
            Arc::new(SearchIndex::open_or_create(&index_dir).expect("failed to open search index"));
        let state = crate::state::build_app_state(pool, Some(search_index));

        let error = match route(
            Query(QueryParams {
                q: Some("Unavailable Song".to_owned()),
                track_name: None,
                artist_name: None,
                album_name: None,
            }),
            State(state.clone()),
        )
        .await
        {
            Ok(_) => panic!("search unexpectedly used an index that was not ready"),
            Err(error) => error,
        };
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        drop(state);
        remove_test_database(&db_path);
        let _ = std::fs::remove_dir_all(&index_dir);
    }

    #[tokio::test]
    async fn tantivy_structured_search_requires_artist_phrase_in_the_artist() {
        let test = test_state_with_ready_search_index(|conn| {
            track_repository::add_one("Scoped Song", "Alpha Beta", "Collection", 180.0, conn)
                .unwrap();
            track_repository::add_one("Scoped Song", "Alpha", "Beta Collection", 181.0, conn)
                .unwrap();
            track_repository::add_one(
                "Scoped Song",
                "Alpha Featured Beta",
                "Collection",
                182.0,
                conn,
            )
            .unwrap();
        })
        .await;

        let response = route(
            Query(QueryParams {
                q: None,
                track_name: Some("Scoped Song".to_owned()),
                artist_name: Some("Alpha Beta".to_owned()),
                album_name: None,
            }),
            State(test.state.clone()),
        )
        .await
        .unwrap_or_else(|_| panic!("structured tantivy search failed"));

        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].id, 1);

        test.cleanup();
    }

    #[tokio::test]
    async fn permits_exhausted_returns_503_with_retry_after() {
        let test = test_state_with_ready_search_index(|conn| {
            track_repository::add_one("Overloaded Search", "Artist", "Album", 180.0, conn)
                .unwrap();
        })
        .await;
        let state = Arc::clone(&test.state);

        let held: Vec<_> = std::iter::repeat_with(|| {
            state.load_permits.clone().try_acquire_owned().ok()
        })
        .take_while(Option::is_some)
        .flatten()
        .collect();
        assert!(!held.is_empty(), "must have held at least one permit");

        let error = match route(
            Query(QueryParams {
                q: Some("Overloaded Search".to_owned()),
                track_name: None,
                artist_name: None,
                album_name: None,
            }),
            State(Arc::clone(&state)),
        )
        .await
        {
            Ok(_) => panic!("search should be rejected when permits are exhausted"),
            Err(e) => e,
        };

        let response = error.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get("retry-after")
                .map(|v: &axum::http::HeaderValue| v.to_str().unwrap()),
            Some("1")
        );

        drop(held);
        drop(state);
        test.cleanup();
    }
}
