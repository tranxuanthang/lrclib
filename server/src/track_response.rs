use serde::{ Deserialize, Serialize };
use crate::{
    entities::{track::SimpleTrack},
};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrackResponse {
    pub id: i64,
    pub name: Option<String>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub duration: Option<f64>,
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub lyricsfile: Option<String>,
}


pub fn create_track_response(track: SimpleTrack) -> TrackResponse {
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