use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction};
use indoc::indoc;
use crate::{
  entities::{lyrics::SimpleLyrics, track::SimpleTrack},
  utils::prepare_input,
};
use chrono::prelude::*;

pub fn get_track_by_id(track_id: i64, conn: &mut Connection) -> Result<Option<SimpleTrack>> {
  let query = indoc! {"
    SELECT
      tracks.id,
      tracks.name,
      tracks.album_name,
      tracks.artist_name,
      tracks.duration,
      tracks.last_lyrics_id,
      lyrics.instrumental,
      lyrics.plain_lyrics,
      lyrics.synced_lyrics
    FROM
      tracks
      LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
    WHERE
      tracks.id = ?
  "};
  let mut statement = conn.prepare(query)?;
  let row = statement.query_row(
    [track_id],
    |row| {
      let instrumental = match row.get("instrumental")? {
        Some(value) => value,
        None => false
      };

      let last_lyrics = SimpleLyrics {
        plain_lyrics: row.get("plain_lyrics")?,
        synced_lyrics: row.get("synced_lyrics")?,
        instrumental,
      };

      Ok(SimpleTrack {
        id: row.get("id")?,
        name: row.get("name")?,
        artist_name: row.get("artist_name")?,
        album_name: row.get("album_name")?,
        duration: row.get("duration")?,
        last_lyrics: Some(last_lyrics),
      })
    }
  ).optional()?;
  Ok(row)
}

pub fn get_track_id_by_metadata(track_name: &str, artist_name: &str, album_name: &str, duration: f64, conn: &mut Connection) -> Result<Option<i64>> {
  let track_name_lower = prepare_input(track_name);
  let artist_name_lower = prepare_input(artist_name);
  let album_name_lower = prepare_input(album_name);

  let query = indoc! {"
    SELECT
      tracks.id
    FROM
      tracks
    WHERE
      tracks.name_lower = ?
      AND tracks.artist_name_lower = ?
      AND tracks.album_name_lower = ?
      AND duration >= ?
      AND duration <= ?
    ORDER BY
      tracks.id
  "};
  let mut statement = conn.prepare(query)?;
  let row = statement.query_row(
    (track_name_lower, artist_name_lower, album_name_lower, duration - 2.0, duration + 2.0),
    |row| {
      Ok(row.get("id")?)
    }
  ).optional()?;
  Ok(row)
}

pub fn get_track_id_by_metadata_tx(track_name: &str, artist_name: &str, album_name: &str, duration: f64, conn: &mut Transaction) -> Result<Option<i64>> {
  let track_name_lower = prepare_input(track_name);
  let artist_name_lower = prepare_input(artist_name);
  let album_name_lower = prepare_input(album_name);

  let query = indoc! {"
    SELECT
      tracks.id
    FROM
      tracks
    WHERE
      tracks.name_lower = ?
      AND tracks.artist_name_lower = ?
      AND tracks.album_name_lower = ?
      AND duration >= ?
      AND duration <= ?
    ORDER BY
      tracks.id
  "};
  let mut statement = conn.prepare(query)?;
  let row = statement.query_row(
    (track_name_lower, artist_name_lower, album_name_lower, duration - 2.0, duration + 2.0),
    |row| {
      Ok(row.get("id")?)
    }
  ).optional()?;
  Ok(row)
}

pub fn get_track_by_metadata(track_name: &str, artist_name: &str, album_name: &str, duration: f64, conn: &mut Connection) -> Result<Option<SimpleTrack>> {
  let track_name_lower = prepare_input(track_name);
  let artist_name_lower = prepare_input(artist_name);
  let album_name_lower = prepare_input(album_name);

  let query = indoc! {"
    SELECT
      tracks.id,
      tracks.name,
      tracks.artist_name,
      tracks.album_name,
      tracks.duration,
      tracks.last_lyrics_id,
      lyrics.instrumental,
      lyrics.plain_lyrics,
      lyrics.synced_lyrics
    FROM
      tracks
      LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
    WHERE
      tracks.name_lower = ?
      AND tracks.artist_name_lower = ?
      AND tracks.album_name_lower = ?
      AND duration >= ?
      AND duration <= ?
    ORDER BY
      tracks.id
  "};
  let mut statement = conn.prepare(query)?;
  let row = statement.query_row(
    (track_name_lower, artist_name_lower, album_name_lower, duration - 2.0, duration + 2.0),
    |row| {
      let instrumental = match row.get("instrumental")? {
        Some(value) => value,
        None => false
      };

      let last_lyrics = SimpleLyrics {
        plain_lyrics: row.get("plain_lyrics")?,
        synced_lyrics: row.get("synced_lyrics")?,
        instrumental,
      };

      Ok(SimpleTrack {
        id: row.get("id")?,
        name: row.get("name")?,
        artist_name: row.get("artist_name")?,
        album_name: row.get("album_name")?,
        duration: row.get("duration")?,
        last_lyrics: Some(last_lyrics),
      })
    }
  ).optional()?;
  Ok(row)
}

pub fn get_tracks_by_keyword(
  q: &Option<String>,
  track_name: &Option<String>,
  artist_name: &Option<String>,
  album_name: &Option<String>,
  conn: &mut Connection,
) -> Result<Vec<SimpleTrack>> {
  let q = q.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let track_name = track_name.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let artist_name = artist_name.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());
  let album_name = album_name.as_ref().map(|s| prepare_input(s)).filter(|s| !s.is_empty()).map(|s| s.to_owned());

  // To search track by keyword, at least q or track_name must be present
  if q.is_none() && track_name.is_none() {
    return Ok(vec![])
  }

  let query = indoc! {"
  SELECT
    tracks.id,
    tracks.name,
    tracks.artist_name,
    tracks.album_name,
    tracks.duration,
    lyrics.instrumental,
    lyrics.plain_lyrics,
    lyrics.synced_lyrics,
    search_results.rank AS rank
  FROM
    (
      SELECT
        rank,
        rowid
      FROM
        tracks_fts
      WHERE
        tracks_fts MATCH ?
      ORDER BY
        rank
      LIMIT
        20
    ) AS search_results
    LEFT JOIN tracks ON search_results.rowid = tracks.id
    LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
  "};
  let mut statement = conn.prepare(query)?;
  let fts_query = match q {
    Some(q) => prepare_input(&q),
    None => {
      match track_name {
        Some(track_name) => {
          let mut result = format!("(name_lower : {})", track_name).to_owned();
          if let Some(artist_name) = artist_name {
            result.push_str(format!("AND (artist_name_lower : {})", artist_name).as_ref());
          }
          if let Some(album_name) = album_name {
            result.push_str(format!("AND (album_name_lower : {})", album_name).as_ref());
          }
          result
        }
        None => "".to_owned()
      }
    }
  };

  tracing::debug!("FTS query: {}", fts_query);

  let mut rows = statement.query([fts_query])?;

  let mut tracks = Vec::new();

  while let Some(row) = rows.next()? {
    let instrumental = match row.get("instrumental")? {
      Some(value) => value,
      None => false
    };

    let last_lyrics = SimpleLyrics {
      plain_lyrics: row.get("plain_lyrics")?,
      synced_lyrics: row.get("synced_lyrics")?,
      instrumental,
    };

    let track = SimpleTrack {
        id: row.get("id")?,
        name: row.get("name")?,
        artist_name: row.get("artist_name")?,
        album_name: row.get("album_name")?,
        duration: row.get("duration")?,
        last_lyrics: Some(last_lyrics),
    };

    tracks.push(track);
  }

  Ok(tracks)
}

pub fn add_one(
  track_name: &str,
  artist_name: &str,
  album_name: &str,
  duration: f64,
  conn: &mut Connection,
) -> Result<i64> {
  let track_name_lower = prepare_input(track_name);
  let artist_name_lower = prepare_input(artist_name);
  let album_name_lower = prepare_input(album_name);

  let now = Utc::now();
  let query = indoc! {"
    INSERT INTO tracks (
      name,
      name_lower,
      artist_name,
      artist_name_lower,
      album_name,
      album_name_lower,
      duration,
      created_at,
      updated_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
  let mut statement = conn.prepare(query)?;
  let row_id = statement.insert(
    (
      track_name,
      track_name_lower,
      artist_name,
      artist_name_lower,
      album_name,
      album_name_lower,
      duration,
      now,
      now,
    )
  )?;
  Ok(row_id)
}

pub fn add_one_tx(
  track_name: &str,
  artist_name: &str,
  album_name: &str,
  duration: f64,
  conn: &mut Transaction,
) -> Result<i64> {
  let track_name_lower = prepare_input(track_name);
  let artist_name_lower = prepare_input(artist_name);
  let album_name_lower = prepare_input(album_name);

  let now = Utc::now();
  let query = indoc! {"
    INSERT INTO tracks (
      name,
      name_lower,
      artist_name,
      artist_name_lower,
      album_name,
      album_name_lower,
      duration,
      created_at,
      updated_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
  let mut statement = conn.prepare(query)?;
  let row_id = statement.insert(
    (
      track_name,
      track_name_lower,
      artist_name,
      artist_name_lower,
      album_name,
      album_name_lower,
      duration,
      now,
      now,
    )
  )?;
  Ok(row_id)
}
