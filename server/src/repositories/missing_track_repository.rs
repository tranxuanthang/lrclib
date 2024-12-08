use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use indoc::indoc;
use chrono::prelude::*;

pub fn get_track_id_by_metadata(
  track_name_lower: &str,
  artist_name_lower: &str,
  album_name_lower: &str,
  duration: f64,
  conn: &mut Connection,
) -> Result<Option<i64>> {
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
  track_name_lower: &str,
  artist_name_lower: &str,
  album_name_lower: &str,
  duration: f64,
  conn: &mut Connection,
) -> Result<i64> {
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

pub fn clean_old_missing_tracks(conn: &mut Connection) -> Result<()> {
  // Delete all missing tracks older than 14 days
  let query = indoc! {"
    DELETE FROM missing_tracks
    WHERE created_at < DATETIME('now', '-14 day')
    LIMIT 10000
  "};
  let mut statement = conn.prepare(query)?;
  statement.execute(())?;
  Ok(())
}
