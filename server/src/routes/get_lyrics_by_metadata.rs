use crate::{
    entities::track::SimpleTrack,
    errors::ApiError,
    errors::OverloadedError,
    metrics::MonitoredRoute,
    repositories::track_repository::get_track_by_metadata,
    utils::process_param,
    AppState,
};
use anyhow::Result;
use axum::{
    extract::{Query, State},
    Json,
};
use axum_macros::debug_handler;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Instant};
use validator::Validate;

#[derive(Clone, Validate, Deserialize)]
pub struct QueryParams {
    #[validate(length(min = 1, message = "cannot be empty"))]
    track_name: String,
    #[validate(length(min = 1, message = "cannot be empty"))]
    artist_name: String,
    album_name: Option<String>,
    #[validate(range(min = 1.0, max = 3600.0, message = "must be between 1 and 3600"))]
    duration: Option<f64>,
}

#[derive(Serialize, Clone)]
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

#[debug_handler]
pub async fn route(
    Query(params): Query<QueryParams>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<TrackResponse>, ApiError> {
    params
        .validate()
        .map_err(|e| ApiError::ValidationError(e.to_string()))?;

    // Process input parameters once
    let track_name_lower = process_param(Some(params.track_name.as_str()));
    let artist_name_lower = process_param(Some(params.artist_name.as_str()));
    let album_name_lower = process_param(params.album_name.as_deref());

    if let (Some(track_name_lower), Some(artist_name_lower)) = (track_name_lower, artist_name_lower)
    {
        let cache_key = build_cache_key(
            &track_name_lower,
            &artist_name_lower,
            album_name_lower.as_deref(),
            params.duration,
        );

        match state.get_metadata_cache.get(&cache_key).await {
            Some(cached) => {
                state
                    .http_metrics
                    .record_cache_hit(MonitoredRoute::GetMetadata);
                return cached
                    .map(|response| Json(response))
                    .ok_or(ApiError::TrackNotFoundError);
            }
            None => {
                state
                    .http_metrics
                    .record_cache_miss(MonitoredRoute::GetMetadata);
            }
        }

        let loader_state = Arc::clone(&state);
        let response = state
            .get_metadata_cache
            .try_get_with(cache_key, async move {
                load_metadata_response(
                    loader_state,
                    params,
                    track_name_lower,
                    artist_name_lower,
                    album_name_lower,
                )
                .await
            })
            .await
            .map_err(|error| {
                if error.downcast_ref::<OverloadedError>().is_some() {
                    state.http_metrics.record_overload(MonitoredRoute::GetMetadata);
                    ApiError::OverloadedError
                } else {
                    ApiError::UnknownError(anyhow::anyhow!(error.to_string()))
                }
            })?;

        return response
            .map(|response| Json(response))
            .ok_or(ApiError::TrackNotFoundError);
    }

    Err(ApiError::TrackNotFoundError)
}

fn build_cache_key(
    track_name_lower: &str,
    artist_name_lower: &str,
    album_name_lower: Option<&str>,
    duration: Option<f64>,
) -> String {
    let album = album_name_lower.unwrap_or("");
    let bucket = duration.map(|d| d.round() as i64).unwrap_or(0);
    format!("get:v4:{track_name_lower}:{artist_name_lower}:{album}:{bucket}")
}

async fn load_metadata_response(
    state: Arc<AppState>,
    params: QueryParams,
    track_name_lower: String,
    artist_name_lower: String,
    album_name_lower: Option<String>,
) -> Result<Option<TrackResponse>> {
    let duration = params.duration;
    let state_clone = Arc::clone(&state);
    let track_name = track_name_lower.clone();
    let artist_name = artist_name_lower.clone();
    let album_name = album_name_lower.clone();

    let blocking_started_at = Instant::now();
    let permit = state.acquire_load_permit().await?;
    let track = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state_clone
            .http_metrics
            .record_blocking_wait_latency(
                MonitoredRoute::GetMetadata,
                blocking_started_at.elapsed(),
            );

        let db_pool_started_at = Instant::now();
        let mut conn = state_clone.pool.get()?;
        state_clone
            .http_metrics
            .record_db_pool_wait_latency(
                MonitoredRoute::GetMetadata,
                db_pool_started_at.elapsed(),
            );

        let sql_started_at = Instant::now();
        let track = get_track_by_metadata(
            &track_name,
            &artist_name,
            album_name.as_deref(),
            duration,
            &mut conn,
        );
        state_clone
            .http_metrics
            .record_sql_latency(MonitoredRoute::GetMetadata, sql_started_at.elapsed());
        track
    })
    .await??;

    if let Some(track) = track {
        return Ok(Some(create_response(track)));
    }

    Ok(None)
}

fn create_response(track: SimpleTrack) -> TrackResponse {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::repositories::track_repository;
    use std::path::PathBuf;
    use tokio::sync::Barrier;
    use uuid::Uuid;

    fn test_state() -> (Arc<AppState>, PathBuf) {
        let path = std::env::temp_dir().join(format!("lrclib-test-{}.sqlite3", Uuid::new_v4()));
        let pool = init_db(&path).expect("failed to initialize test database");
        (crate::state::build_app_state(pool, None), path)
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_metadata_misses_share_one_load() {
        const REQUESTS: usize = 16;

        let (state, path) = test_state();
        let expected_track_id = {
            let mut conn = state.pool.get().unwrap();
            track_repository::add_one(
                "Single Flight Metadata",
                "Test Artist",
                "Test Album",
                180.0,
                &mut conn,
            )
            .unwrap()
        };
        let barrier = Arc::new(Barrier::new(REQUESTS + 1));
        let mut tasks = Vec::with_capacity(REQUESTS);

        for _ in 0..REQUESTS {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                route(
                    Query(QueryParams {
                        track_name: "Single Flight Metadata".to_owned(),
                        artist_name: "Test Artist".to_owned(),
                        album_name: Some("Test Album".to_owned()),
                        duration: Some(180.0),
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
                .expect("metadata task failed")
                .unwrap_or_else(|_| panic!("metadata lookup failed"));
            assert_eq!(response.0.id, expected_track_id);
        }

        drop(state);
        remove_test_database(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_missing_metadata_requests_share_one_load() {
        const REQUESTS: usize = 16;

        let (state, path) = test_state();
        let barrier = Arc::new(Barrier::new(REQUESTS + 1));
        let mut tasks = Vec::with_capacity(REQUESTS);

        for _ in 0..REQUESTS {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                route(
                    Query(QueryParams {
                        track_name: "Missing Metadata".to_owned(),
                        artist_name: "Missing Artist".to_owned(),
                        album_name: Some("Missing Album".to_owned()),
                        duration: Some(180.0),
                    }),
                    State(state),
                )
                .await
            }));
        }

        barrier.wait().await;
        for task in tasks {
            let result = task.await.expect("missing metadata task failed");
            assert!(matches!(result, Err(ApiError::TrackNotFoundError)));
        }

        drop(state);
        remove_test_database(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cached_miss_avoids_db_on_retry() {
        let (state, path) = test_state();

        let params = QueryParams {
            track_name: "Nonexistent Track".to_owned(),
            artist_name: "Nonexistent Artist".to_owned(),
            album_name: Some("Nonexistent Album".to_owned()),
            duration: Some(180.0),
        };

        let result = route(Query(params.clone()), State(Arc::clone(&state))).await;
        assert!(matches!(result, Err(ApiError::TrackNotFoundError)));

        let result = route(Query(params), State(Arc::clone(&state))).await;
        assert!(matches!(result, Err(ApiError::TrackNotFoundError)));

        drop(state);
        remove_test_database(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cached_hit_returns_track_on_retry() {
        let (state, path) = test_state();

        let track_id = {
            let mut conn = state.pool.get().unwrap();
            track_repository::add_one(
                "Cached Track",
                "Cached Artist",
                "Cached Album",
                240.0,
                &mut conn,
            )
            .unwrap()
        };

        let params = QueryParams {
            track_name: "Cached Track".to_owned(),
            artist_name: "Cached Artist".to_owned(),
            album_name: Some("Cached Album".to_owned()),
            duration: Some(240.0),
        };

        let response = route(Query(params.clone()), State(Arc::clone(&state)))
            .await
            .unwrap_or_else(|_| panic!("first lookup failed"));
        assert_eq!(response.0.id, track_id);

        let response = route(Query(params), State(Arc::clone(&state)))
            .await
            .unwrap_or_else(|_| panic!("second lookup failed"));
        assert_eq!(response.0.id, track_id);

        drop(state);
        remove_test_database(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permits_exhausted_returns_503_with_retry_after() {
        use axum::response::IntoResponse;

        let (state, path) = test_state();

        let held: Vec<_> = std::iter::repeat_with(|| {
            state.load_permits.clone().try_acquire_owned().ok()
        })
        .take_while(Option::is_some)
        .flatten()
        .collect();
        assert!(!held.is_empty(), "must have held at least one permit");

        let error = match route(
            Query(QueryParams {
                track_name: "Overloaded Get".to_owned(),
                artist_name: "Artist".to_owned(),
                album_name: Some("Album".to_owned()),
                duration: Some(180.0),
            }),
            State(Arc::clone(&state)),
        )
        .await
        {
            Ok(_) => panic!("get should be rejected when permits are exhausted"),
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
        remove_test_database(&path);
    }
}
