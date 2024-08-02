use axum::{extract::{Query, State}, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{
  entities::track::SimpleTrack, errors::ApiError, repositories::track_repository::get_tracks_by_keyword, utils::prepare_input, AppState
};

// Query parameters
#[derive(Serialize, Deserialize)]
pub struct QueryParams {
  q: Option<String>,
  track_name: Option<String>,
  artist_name: Option<String>,
  album_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
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

pub async fn route(Query(params): Query<QueryParams>, State(state): State<Arc<AppState>>) -> Result<Json<Vec<TrackResponse>>, ApiError> {
  let q = params.q.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let track_name = params.track_name.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let artist_name = params.artist_name.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let album_name = params.album_name.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());

  // Generate a cache key based on query parameters
  let cache_key = format!(
    "{}:{}:{}:{}",
    q.as_deref().unwrap_or_default(),
    track_name.as_deref().unwrap_or_default(),
    artist_name.as_deref().unwrap_or_default(),
    album_name.as_deref().unwrap_or_default()
  );

  // Check if cached result is available
  let cached_tracks = {
    state.search_cache.get(&cache_key).await
  };

  if let Some(cached_result) = cached_tracks {
    // Deserialize cached result
    let tracks: Vec<TrackResponse> = serde_json::from_str(&cached_result)?;
    return Ok(Json(tracks));
  }

  let tracks = {
    let mut conn = state.pool.get()?;
    get_tracks_by_keyword(
      &q,
      &track_name,
      &artist_name,
      &album_name,
      &mut conn,
    )?
  };

  let response = create_response(tracks);

  // Serialize and cache the response
  let response_json = serde_json::to_string(&response)?;
  {
    state.search_cache.insert(cache_key, response_json).await;
  }

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
