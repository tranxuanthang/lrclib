use axum::{extract::{Query, State}, Json};
use serde::{Deserialize,Serialize};
use std::sync::Arc;
use crate::{
    entities::{missing_track::MissingTrack, track::SimpleTrack},
    errors::ApiError,
    repositories::{track_repository::get_track_by_metadata, missing_track_repository},
    utils::process_param,
    AppState,
};
use axum_macros::debug_handler;
use validator::Validate;
use anyhow::Result;
use crossbeam_queue::ArrayQueue;

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

  // Retry fetching the track without the album name
  if let Some(_) = process_param(&params.album_name) {
    if let Some(track) = fetch_track_without_album(&params, &state).await? {
      return Ok(Json(create_response(track)));
    }
  }

  // If not found, handle missing track logic
  let params_clone = params.clone();
  let state_clone = state.clone();
  tokio::spawn(async move {
    if let Err(e) = handle_missing_track(&params_clone, &state_clone).await {
      tracing::error!(message = "failed to handle missing track", error = e.to_string());
    }
  });

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

async fn handle_missing_track(params: &QueryParams, state: &Arc<AppState>) -> Result<()> {
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

    let mut conn = state.pool.get()?;

    let missing_track_id = missing_track_repository::get_track_id_by_metadata(&params.track_name, &params.artist_name, &album_name, duration, &mut conn)?;

    if let None = missing_track_id {
      missing_track_repository::add_one(&params.track_name, &params.artist_name, &album_name, duration, &mut conn)?;
      send_to_queue(missing_track, &state.queue);
    }
  }

  Ok(())
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

fn send_to_queue(missing_track: MissingTrack, queue: &ArrayQueue<MissingTrack>) {
  match queue.push(missing_track.clone()) {
    Ok(_) => tracing::info!(
      message = "sent missing track to queue",
      track_name = missing_track.name,
      artist_name = missing_track.artist_name,
      album_name = missing_track.album_name,
      duration = missing_track.duration,
    ),
    Err(missing_track) => tracing::error!(
      message = "failed to push to queue",
      track_name = missing_track.name,
      artist_name = missing_track.artist_name,
      album_name = missing_track.album_name,
      duration = missing_track.duration,
    ),
  }
}
