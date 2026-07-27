use crate::{
    AppState, entities::lyrics::SimpleLyrics, errors::ApiError, repositories::track_repository::{get_lyrics_by_id, get_track_by_id}, track_response::{TrackResponse, create_track_response}
};
use axum::{
    extract::{Path, State},
    Json,
};
use std::sync::Arc;
use serde::{ Deserialize, Serialize };

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LyricsResponse {
    lyrics_id: i64,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}



pub async fn route(
    Path(lyrics_id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<LyricsResponse>, ApiError> {
    let maybe_lyrics = {
        let mut conn = state.pool.get()?;
        get_lyrics_by_id(lyrics_id, &mut conn)?
    };

    match maybe_lyrics {
        Some(lyrics) => Ok(Json(create_lyrics_response(lyrics))),
        None => Err(ApiError::TrackNotFoundError),
    }
}



fn create_lyrics_response(lyrics: SimpleLyrics) -> LyricsResponse {
    LyricsResponse {
        lyrics_id: lyrics.lyrics_id,
        instrumental: lyrics.instrumental,
        plain_lyrics: lyrics.plain_lyrics,
        synced_lyrics: lyrics.synced_lyrics,
        lyricsfile: lyrics.lyricsfile,
    }
}