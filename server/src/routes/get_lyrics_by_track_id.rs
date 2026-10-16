use crate::{
    entities::track::SimpleTrack, errors::ApiError,
    repositories::track_repository::get_track_by_id, AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackResponse {
    id: i64,
    name: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: bool,
    has_word_sync: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}

pub async fn route(
    Path(track_id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<TrackResponse>, ApiError> {
    let permit = state
        .acquire_load_permit()
        .await
        .map_err(|_| ApiError::OverloadedError)?;

    let state_clone = Arc::clone(&state);
    let maybe_track = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut conn = state_clone.pool.get()?;
        get_track_by_id(track_id, &mut conn)
    })
    .await??;

    match maybe_track {
        Some(track) => Ok(Json(create_response(track))),
        None => Err(ApiError::TrackNotFoundError),
    }
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

    let has_word_sync = match track.last_lyrics {
        Some(ref lyrics) => lyrics.has_word_sync,
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
        has_word_sync,
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
    use axum::response::IntoResponse;
    use std::path::PathBuf;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_track_by_id() {
        let (state, path) = test_state();

        let track_id = {
            let mut conn = state.pool.get().unwrap();
            track_repository::add_one(
                "Track Id Lookup",
                "Test Artist",
                "Test Album",
                180.0,
                &mut conn,
            )
            .unwrap()
        };

        let response = route(Path(track_id), State(Arc::clone(&state)))
            .await
            .unwrap_or_else(|_| panic!("track id lookup failed"));
        assert_eq!(response.0.id, track_id);
        assert_eq!(response.0.track_name.as_deref(), Some("Track Id Lookup"));

        drop(state);
        remove_test_database(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_track_returns_not_found() {
        let (state, path) = test_state();

        let error = match route(Path(42), State(Arc::clone(&state))).await {
            Ok(_) => panic!("missing track should not be returned"),
            Err(error) => error,
        };
        assert!(matches!(error, ApiError::TrackNotFoundError));

        drop(state);
        remove_test_database(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permits_exhausted_returns_503_with_retry_after() {
        let (state, path) = test_state();

        let held: Vec<_> =
            std::iter::repeat_with(|| state.load_permits.clone().try_acquire_owned().ok())
                .take_while(Option::is_some)
                .flatten()
                .collect();
        assert!(!held.is_empty(), "must have held at least one permit");

        let error = match route(Path(1), State(Arc::clone(&state))).await {
            Ok(_) => panic!("get by track id should be rejected when permits are exhausted"),
            Err(e) => e,
        };

        let response = error.into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
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
