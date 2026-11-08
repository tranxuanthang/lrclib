use super::lyrics::SimpleLyrics;
use chrono::prelude::*;

pub struct Track {
    pub id: i64,
    pub name: Option<String>,
    pub album_name: Option<String>,
    pub artist_name: Option<String>,
    pub duration: Option<f64>,
    pub last_lyrics_id: Option<i64>,
    pub last_lyrics: Option<SimpleLyrics>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub struct SimpleTrack {
    pub id: i64,
    pub name: Option<String>,
    pub album_name: Option<String>,
    pub artist_name: Option<String>,
    pub duration: Option<f64>,
    pub last_lyrics: Option<SimpleLyrics>,
}

/// Projection of a track used to build the full-text search index.
pub struct TrackSearchDoc {
    pub id: i64,
    pub name_lower: Option<String>,
    pub artist_name_lower: Option<String>,
    pub album_name_lower: Option<String>,
}
