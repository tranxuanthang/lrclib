use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use indoc::indoc;
use crate::utils::prepare_input;
use chrono::prelude::*;

pub fn get_track_id_by_metadata(track_name: &str, artist_name: &str, album_name: &str, duration: f64, conn: &mut Connection) -> Result<Option<i64>> {
  let track_name_lower = prepare_input(track_name);
  let artist_name_lower = prepare_input(artist_name);
  let album_name_lower = prepare_input(album_name);

  let query = indoc! {"
    SELECT
      missing_tracks.id
    FROM
      missing_tracks
    WHERE
      missing_tracks.name_lower = ?
      AND missing_tracks.artist_name_lower = ?
      AND missing_tracks.album_name_lower = ?
      AND duration >= ?
      AND duration <= ?
    ORDER BY
      missing_tracks.id
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
    INSERT INTO missing_tracks (
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
