use chrono::prelude::*;

pub struct Lyrics {
  pub id: i64,
  pub plain_lyrics: Option<String>,
  pub synced_lyrics: Option<String>,
  pub track_id: i64,
  pub has_plain_lyrics: bool,
  pub has_synced_lyrics: bool,
  pub instrumental: bool,
  pub source: Option<String>,
  pub created_at: Option<DateTime<Utc>>,
  pub updated_at: Option<DateTime<Utc>>,
}

pub struct SimpleLyrics {
  pub plain_lyrics: Option<String>,
  pub synced_lyrics: Option<String>,
  pub instrumental: bool,
}
