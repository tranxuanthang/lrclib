use axum::{extract::{Query, State}, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{
  entities::track::SimpleTrack,
  errors::ApiError,
  repositories::track_repository::get_tracks_by_keyword,
  AppState,
};

// Query parameters
#[derive(Deserialize)]
pub struct QueryParams {
  q: Option<String>,
  track_name: Option<String>,
  artist_name: Option<String>,
  album_name: Option<String>,
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

pub async fn route(Query(params): Query<QueryParams>, State(state): State<Arc<AppState>>) -> Result<Json<Vec<TrackResponse>>, ApiError> {
  let tracks = {
    let mut conn = state.pool.get()?;
    get_tracks_by_keyword(
      &params.q,
      &params.track_name,
      &params.artist_name,
      &params.album_name,
      &mut conn,
    )?
  };
  Ok(Json(create_response(tracks)))
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
