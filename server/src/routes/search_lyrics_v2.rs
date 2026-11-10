use crate::{
    entities::track::SimpleTrackSummary,
    errors::{ApiError, OverloadedError},
    metrics::MonitoredRoute,
    repositories::track_repository::get_track_summaries_by_ids,
    utils::process_param,
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Instant};

// Pilot endpoint: metadata and lyrics availability without lyrics text.
#[derive(Serialize, Deserialize)]
pub struct QueryParams {
    q: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrackSummaryResponse {
    id: i64,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: bool,
    has_word_sync: bool,
    has_plain_lyrics: bool,
    has_synced_lyrics: bool,
    has_lyricsfile: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CachedResult {
    tracks: Vec<TrackSummaryResponse>,
    created_at: DateTime<Utc>,
}

pub async fn route(
    Query(params): Query<QueryParams>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TrackSummaryResponse>>, ApiError> {
    let q = process_param(params.q.as_deref());
    // Keyword search ignores all structured constraints in the query builder.
    let (track_name, artist_name, album_name) = if q.is_some() {
        (None, None, None)
    } else {
        (
            process_param(params.track_name.as_deref()),
            process_param(params.artist_name.as_deref()),
            process_param(params.album_name.as_deref()),
        )
    };

    let cache_key = build_cache_key(
        q.as_deref(),
        track_name.as_deref(),
        artist_name.as_deref(),
        album_name.as_deref(),
    );

    if let Some(cached_result) = state.search_v2_cache.get(&cache_key).await {
        state
            .http_metrics
            .record_cache_hit(MonitoredRoute::SearchV2);
        return Ok(Json(cached_result.tracks));
    }

    state
        .http_metrics
        .record_cache_miss(MonitoredRoute::SearchV2);

    let loader_state = Arc::clone(&state);
    let cached_result = state
        .search_v2_cache
        .try_get_with(cache_key, async move {
            load_tracks(loader_state, q, track_name, artist_name, album_name).await
        })
        .await
        .map_err(|error| {
            if error.downcast_ref::<OverloadedError>().is_some() {
                state.http_metrics.record_overload(MonitoredRoute::SearchV2);
                ApiError::OverloadedError
            } else {
                ApiError::UnknownError(anyhow::anyhow!(error.to_string()))
            }
        })?;

    Ok(Json(cached_result.tracks))
}

fn build_cache_key(
    q: Option<&str>,
    track_name: Option<&str>,
    artist_name: Option<&str>,
    album_name: Option<&str>,
) -> String {
    match (q, track_name) {
        (Some(q), _) => format!("search:v5:q:{q}"),
        (None, Some(track_name)) => format!(
            "search:v5:metadata:{track_name}:{}:{}",
            artist_name.unwrap_or_default(),
            album_name.unwrap_or_default()
        ),
        (None, None) => "search:v5:empty".to_owned(),
    }
}

fn create_response(summaries: Vec<SimpleTrackSummary>) -> Vec<TrackSummaryResponse> {
    summaries
        .into_iter()
        .map(|track| TrackSummaryResponse {
            id: track.id,
            track_name: track.name,
            artist_name: track.artist_name,
            album_name: track.album_name,
            duration: track.duration,
            instrumental: track.instrumental,
            has_word_sync: track.has_word_sync,
            has_plain_lyrics: track.has_plain_lyrics,
            has_synced_lyrics: track.has_synced_lyrics,
            has_lyricsfile: track.has_lyricsfile,
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
    let search_index = state
        .search_index()
        .ok_or_else(|| anyhow::anyhow!("search index is unavailable"))?;
    if !search_index.is_ready() {
        anyhow::bail!("search index is not ready");
    }

    let search_index = Arc::clone(search_index);
    let blocking_started_at = Instant::now();
    let permit = state.acquire_load_permit().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .http_metrics
            .record_blocking_wait_latency(MonitoredRoute::SearchV2, blocking_started_at.elapsed());

        let tantivy_started_at = Instant::now();
        let track_ids = search_index.search(
            q.as_deref(),
            track_name.as_deref(),
            artist_name.as_deref(),
            album_name.as_deref(),
        );
        state
            .http_metrics
            .record_tantivy_latency_for(MonitoredRoute::SearchV2, tantivy_started_at.elapsed());
        let track_ids = track_ids?;

        if track_ids.is_empty() {
            return Ok(CachedResult {
                tracks: Vec::new(),
                created_at: Utc::now(),
            });
        }

        let db_pool_started_at = Instant::now();
        let conn = state.db_connection();
        state
            .http_metrics
            .record_db_pool_wait_latency(MonitoredRoute::SearchV2, db_pool_started_at.elapsed());
        let mut conn = conn?;
        let sql_started_at = Instant::now();
        let summaries = get_track_summaries_by_ids(&track_ids, &mut conn);
        state
            .http_metrics
            .record_sql_latency(MonitoredRoute::SearchV2, sql_started_at.elapsed());
        drop(conn);
        Ok(CachedResult {
            tracks: create_response(summaries?),
            created_at: Utc::now(),
        })
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
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::Duration;
    use uuid::Uuid;

    fn test_pool() -> (r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("lrclib-v2-summary-{}.sqlite3", Uuid::new_v4()));
        let pool = init_db(&path).expect("failed to initialize test database");
        (pool, path)
    }

    fn cleanup(path: &PathBuf) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    struct SearchIndexTestState {
        state: Arc<AppState>,
        db_path: PathBuf,
        index_dir: PathBuf,
        indexer: Option<IndexerHandle>,
    }

    impl SearchIndexTestState {
        fn cleanup(mut self) {
            self.indexer.take();
            cleanup(&self.db_path);
            let _ = std::fs::remove_dir_all(&self.index_dir);
        }
    }

    async fn test_state_with_ready_search_index(
        seed: impl FnOnce(&mut rusqlite::Connection),
    ) -> SearchIndexTestState {
        let base = std::env::temp_dir().join(format!("lrclib-v2-route-{}", Uuid::new_v4()));
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

        SearchIndexTestState {
            state: crate::state::build_app_state(pool, Some(search_index)),
            db_path,
            index_dir,
            indexer: Some(indexer),
        }
    }

    fn insert_lyrics(conn: &mut rusqlite::Connection, track_id: i64, title: &str, artist: &str) {
        let lyricsfile = format!(
            "version: '1.0'\nmetadata:\n  title: {title}\n  artist: {artist}\n  instrumental: false\nlines:\n  - text: Hello\n    start_ms: 1000\nplain: Hello\n"
        );
        lyrics_repository::add_one(
            &Some("Hello".to_owned()),
            &Some("[00:01.000]Hello".to_owned()),
            &lyricsfile,
            track_id,
            false,
            false,
            &Some("test".to_owned()),
            conn,
        )
        .unwrap();
    }

    #[test]
    fn summary_query_returns_flags_without_lyrics_text() {
        let (pool, path) = test_pool();
        let track_id = {
            let mut conn = pool.get().unwrap();
            let track_id = track_repository::add_one(
                "Summary Song",
                "Summary Artist",
                "Summary Album",
                180.0,
                &mut conn,
            )
            .unwrap();
            insert_lyrics(&mut conn, track_id, "Summary Song", "Summary Artist");
            track_id
        };

        let mut conn = pool.get().unwrap();
        let summaries = get_track_summaries_by_ids(&[track_id], &mut conn).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, track_id);
        assert_eq!(summaries[0].name.as_deref(), Some("Summary Song"));
        assert!(summaries[0].has_plain_lyrics);
        assert!(summaries[0].has_synced_lyrics);
        assert!(summaries[0].has_lyricsfile);
        assert!(!summaries[0].has_word_sync);
        assert!(!summaries[0].instrumental);

        drop(conn);
        drop(pool);
        cleanup(&path);
    }

    #[tokio::test]
    async fn route_returns_summary_without_lyrics_text() {
        let test = test_state_with_ready_search_index(|conn| {
            let track_id =
                track_repository::add_one("V2 Summary Song", "V2 Artist", "V2 Album", 180.0, conn)
                    .unwrap();
            insert_lyrics(conn, track_id, "V2 Summary Song", "V2 Artist");
        })
        .await;

        let response = route(
            Query(QueryParams {
                q: Some("V2 Summary Song".to_owned()),
                track_name: None,
                artist_name: None,
                album_name: None,
            }),
            State(Arc::clone(&test.state)),
        )
        .await
        .unwrap_or_else(|_| panic!("v2 search failed"));

        assert_eq!(response.0.len(), 1);
        assert_eq!(response.0[0].track_name.as_deref(), Some("V2 Summary Song"));
        assert!(response.0[0].has_plain_lyrics);
        assert!(response.0[0].has_synced_lyrics);
        assert!(response.0[0].has_lyricsfile);

        let value = serde_json::to_value(&response.0[0]).unwrap();
        assert!(value.get("plainLyrics").is_none());
        assert!(value.get("syncedLyrics").is_none());
        assert!(value.get("lyricsfile").is_none());

        test.cleanup();
    }

    #[test]
    fn summary_response_omits_lyrics_text() {
        let response = create_response(vec![SimpleTrackSummary {
            id: 7,
            name: Some("Summary Song".to_owned()),
            artist_name: Some("Summary Artist".to_owned()),
            album_name: Some("Summary Album".to_owned()),
            duration: Some(180.0),
            instrumental: false,
            has_word_sync: true,
            has_plain_lyrics: true,
            has_synced_lyrics: true,
            has_lyricsfile: true,
        }]);

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value,
            json!([{
                "id": 7,
                "trackName": "Summary Song",
                "artistName": "Summary Artist",
                "albumName": "Summary Album",
                "duration": 180.0,
                "instrumental": false,
                "hasWordSync": true,
                "hasPlainLyrics": true,
                "hasSyncedLyrics": true,
                "hasLyricsfile": true,
            }])
        );
        assert!(value[0].get("plainLyrics").is_none());
        assert!(value[0].get("syncedLyrics").is_none());
        assert!(value[0].get("lyricsfile").is_none());
    }
}
