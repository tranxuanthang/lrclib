use axum::{extract::{Query, State}, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{
  entities::track::SimpleTrack,
  errors::ApiError,
  repositories::track_repository::get_tracks_by_keyword,
  utils::process_param,
  AppState,
};

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
}

#[derive(Serialize, Deserialize)]
pub struct CachedResult {
  tracks: Vec<TrackResponse>,
  created_at: DateTime<Utc>,
}

pub async fn route(Query(params): Query<QueryParams>, State(state): State<Arc<AppState>>) -> Result<Json<Vec<TrackResponse>>, ApiError> {
  let q = process_param(params.q.as_deref());
  let track_name = process_param(params.track_name.as_deref());
  let artist_name = process_param(params.artist_name.as_deref());
  let album_name = process_param(params.album_name.as_deref());

  // Generate a cache key based on query parameters
  let cache_key = format!(
    "{}:{}:{}:{}",
    q.as_deref().unwrap_or_default(),
    track_name.as_deref().unwrap_or_default(),
    artist_name.as_deref().unwrap_or_default(),
    album_name.as_deref().unwrap_or_default()
  );

  let cached_result: Option<CachedResult> = {
    let cached_result_str = state.search_cache.get(&cache_key).await;
    if let Some(cached_result_str) = cached_result_str {
      serde_json::from_str(&cached_result_str).ok()
    } else {
      None
    }
  };

  if let Some(cached_result) = cached_result {
    let tracks: Vec<TrackResponse> = cached_result.tracks.clone();

    let now = Utc::now();
    let created_at = cached_result.created_at;

    if (now - created_at).num_hours() >= 20 {
      let state_clone = Arc::clone(&state);
      let cache_key_clone = cache_key.clone();
      let q_clone = q.clone();
      let track_name_clone = track_name.clone();
      let artist_name_clone = artist_name.clone();
      let album_name_clone = album_name.clone();

      tokio::spawn(async move {
        let _ = fetch_and_cache_tracks(
          state_clone,
          cache_key_clone,
          q_clone.as_deref(),
          track_name_clone.as_deref(),
          artist_name_clone.as_deref(),
          album_name_clone.as_deref(),
        ).await;
      });
    }

    return Ok(Json(tracks));
  }

  let response = fetch_and_cache_tracks(
    state,
    cache_key,
    q.as_deref(),
    track_name.as_deref(),
    artist_name.as_deref(),
    album_name.as_deref(),
  ).await?;

  Ok(Json(response))
}

fn create_response(tracks: Vec<SimpleTrack>) -> Vec<TrackResponse> {
  tracks.iter().map(
    |track| {
      let plain_lyrics = match track.last_lyrics {
        Some(ref lyrics) => lyrics.plain_lyrics.to_owned(),
        None => None
      };

      let synced_lyrics = match track.last_lyrics {
        Some(ref lyrics) => lyrics.synced_lyrics.to_owned(),
        None => None
      };

      let instrumental = match track.last_lyrics {
        Some(ref lyrics) => lyrics.instrumental.to_owned(),
        None => false
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
      }
    }
  ).collect()
}

async fn fetch_and_cache_tracks(
  state: Arc<AppState>,
  cache_key: String,
  q: Option<&str>,
  track_name: Option<&str>,
  artist_name: Option<&str>,
  album_name: Option<&str>,
) -> Result<Vec<TrackResponse>, ApiError> {
  let mut conn = state.pool.get()?;
  let tracks = get_tracks_by_keyword(
      q,
      track_name,
      artist_name,
      album_name,
      &mut conn,
  )?;

  let response = create_response(tracks);

  let cached_result = CachedResult {
      tracks: response.clone(),
      created_at: Utc::now(),
  };

  state.search_cache.insert(cache_key, serde_json::to_string(&cached_result)?).await;

  Ok(response)
}
