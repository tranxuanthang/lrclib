use axum::{extract::{Query, State}, Json};
use serde::{Deserialize,Serialize};
use std::{collections::VecDeque, sync::Arc};
use crate::{entities::{missing_track::MissingTrack, track::SimpleTrack}, errors::ApiError, repositories::track_repository::get_track_by_metadata, AppState};
use axum_macros::debug_handler;
use validator::Validate;
use anyhow::Result;

#[derive(Validate, Deserialize)]
pub struct QueryParams {
  #[validate(length(min = 1, message = "cannot be empty"))]
  track_name: String,
  #[validate(length(min = 1, message = "cannot be empty"))]
  artist_name: String,
  album_name: Option<String>,
  #[validate(range(min = 1.0, max = 3600.0, message = "must be between 1 and 3600"))]
  duration: Option<f64>,
}

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
  plain_lyrics: Option<String>,
  synced_lyrics: Option<String>,
}

#[debug_handler]
pub async fn route(Query(params): Query<QueryParams>, State(state): State<Arc<AppState>>) -> Result<Json<TrackResponse>, ApiError> {
  params.validate().map_err(|e| ApiError::ValidationError(e.to_string()))?;

  // Attempt to fetch the track with all provided metadata
  if let Some(track) = fetch_track(&params, &state).await? {
    return Ok(Json(create_response(track)));
  }

  // If not found, handle missing track logic
  handle_missing_track(&params, &state).await;

  // Retry fetching the track without the album name
  if let Some(track) = fetch_track_without_album(&params, &state).await? {
    return Ok(Json(create_response(track)));
  }

  Err(ApiError::TrackNotFoundError)
}

async fn fetch_track(params: &QueryParams, state: &Arc<AppState>) -> Result<Option<SimpleTrack>> {
  let mut conn = state.pool.get()?;
  get_track_by_metadata(
    &params.track_name,
    &params.artist_name,
    params.album_name.as_deref(),
    params.duration,
    &mut conn,
  )
}

async fn fetch_track_without_album(params: &QueryParams, state: &Arc<AppState>) -> Result<Option<SimpleTrack>> {
  let mut conn = state.pool.get()?;
  get_track_by_metadata(
    &params.track_name,
    &params.artist_name,
    None,
    params.duration,
    &mut conn,
  )
}

async fn handle_missing_track(params: &QueryParams, state: &Arc<AppState>) {
  if let (Some(album_name), Some(duration)) = (
    params.album_name.as_ref().filter(|name| !name.trim().is_empty()),
    params.duration,
  ) {
    let missing_track = MissingTrack {
      name: params.track_name.trim().to_owned(),
      artist_name: params.artist_name.trim().to_owned(),
      album_name: album_name.trim().to_owned(),
      duration,
    };

    let cache_key = format!("missing_track:{}", missing_track);
    if !state.get_cache.contains_key(&cache_key) {
      state.get_cache.insert(cache_key, "1".to_owned()).await;
      let mut queue_lock = state.queue.lock().await;
      send_to_queue(missing_track, &mut *queue_lock).await;
    }
  }
}

fn create_response(track: SimpleTrack) -> TrackResponse {
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

async fn send_to_queue(missing_track: MissingTrack, queue: &mut VecDeque<MissingTrack>) {
  tracing::info!(
    message = format!("sending missing track to queue"),
    track_name = missing_track.name,
    artist_name = missing_track.artist_name,
    album_name = missing_track.album_name,
    duration = missing_track.duration,
  );
  queue.push_back(missing_track);
}
