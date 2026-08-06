use chrono::prelude::*;

pub struct Lyrics {
    pub id: i64,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub lyricsfile: Option<String>,
    pub track_id: i64,
    pub has_plain_lyrics: bool,
    pub has_synced_lyrics: bool,
    pub has_lyricsfile: bool,
    pub instrumental: bool,
    pub source: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub struct SimpleLyrics {
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub lyricsfile: Option<String>,
    pub instrumental: bool,
}

#[derive(Debug)]
pub struct LyricsfileBackfillCandidate {
    pub id: i64,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub lyricsfile: Option<String>,
    pub instrumental: bool,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration: f64,
}

#[derive(Debug)]
pub struct LyricsfileBackfillUpdate {
    pub id: i64,
    pub lyricsfile: String,
    pub expected_lyricsfile: Option<String>,
}
