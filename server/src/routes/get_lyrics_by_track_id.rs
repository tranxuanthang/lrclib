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
    let maybe_track = {
        let mut conn = state.pool.get()?;
        get_track_by_id(track_id, &mut conn)?
    };

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
