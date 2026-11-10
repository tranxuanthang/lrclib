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

/// Track metadata and lyrics availability flags for the v2 search summary.
///
/// This deliberately omits lyrics text so the search path does not fetch or
/// serialize the large plain/synced/Lyricsfile payloads.
pub struct SimpleTrackSummary {
    pub id: i64,
    pub name: Option<String>,
    pub album_name: Option<String>,
    pub artist_name: Option<String>,
    pub duration: Option<f64>,
    pub instrumental: bool,
    pub has_word_sync: bool,
    pub has_plain_lyrics: bool,
    pub has_synced_lyrics: bool,
    pub has_lyricsfile: bool,
}

/// Projection of a track used to build the full-text search index.
pub struct TrackSearchDoc {
    pub id: i64,
    pub name_lower: Option<String>,
    pub artist_name_lower: Option<String>,
    pub album_name_lower: Option<String>,
}

/// Track row whose normalized metadata is missing and can be recomputed from
/// the raw values.
#[derive(Debug)]
pub struct TrackNormalizationCandidate {
    pub id: i64,
    pub name: Option<String>,
    pub name_lower: Option<String>,
    pub artist_name: Option<String>,
    pub artist_name_lower: Option<String>,
    pub album_name: Option<String>,
    pub album_name_lower: Option<String>,
}

/// Guarded update produced from a `TrackNormalizationCandidate`.
#[derive(Clone, Debug)]
pub struct TrackNormalizationUpdate {
    pub id: i64,
    pub expected_name_lower: Option<String>,
    pub expected_artist_name_lower: Option<String>,
    pub expected_album_name_lower: Option<String>,
    pub name_lower: Option<String>,
    pub artist_name_lower: Option<String>,
    pub album_name_lower: Option<String>,
}
