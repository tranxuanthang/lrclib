use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction};
use indoc::indoc;
use crate::{
  entities::{lyrics::SimpleLyrics, track::SimpleTrack},
  utils::prepare_input,
};
use chrono::prelude::*;
use rusqlite::params_from_iter;

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

pub fn get_track_by_metadata(
  track_name_lower: &str,
  artist_name_lower: &str,
  album_name_lower: Option<&str>,
  duration: Option<f64>,
  conn: &mut Connection,
) -> Result<Option<SimpleTrack>> {
  // Start building the SQL query
  let select_query = indoc! {"
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
  "};

  // Initialize where clauses and parameters
  let mut where_clauses = vec![
    "tracks.name_lower = ?".to_string(),
    "tracks.artist_name_lower = ?".to_string(),
  ];
  let mut params: Vec<rusqlite::types::Value> = vec![
    track_name_lower.to_string().into(),
    artist_name_lower.to_string().into(),
  ];

  // Conditionally add duration constraints
  if let Some(dur) = duration {
    let duration_min = dur - 2.0;
    let duration_max = dur + 2.0;
    where_clauses.push("tracks.duration >= ?".to_string());
    where_clauses.push("tracks.duration <= ?".to_string());
    params.push(duration_min.into());
    params.push(duration_max.into());
  }

  // Conditionally add album_name to the query
  if let Some(album_name_lower) = album_name_lower {
    where_clauses.push("tracks.album_name_lower = ?".to_string());
    params.push(album_name_lower.to_string().into());
  }

  // Combine all parts of the query
  let query = format!(
    "{select} WHERE {where_clause} ORDER BY tracks.id",
    select = select_query,
    where_clause = where_clauses.join(" AND ")
  );

  // Prepare and execute the statement
  let mut statement = conn.prepare(&query)?;
  let row = statement.query_row(
    params_from_iter(params.iter().map(|v| v as &dyn rusqlite::ToSql)),
    |row| {
      let instrumental = row.get::<_, Option<bool>>("instrumental")?.unwrap_or(false);

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
  q: Option<&str>,
  track_name: Option<&str>,
  artist_name: Option<&str>,
  album_name: Option<&str>,
  conn: &mut Connection,
) -> Result<Vec<SimpleTrack>> {
  // To search track by keyword, at least q or track_name must be present
  if q.is_none() && track_name.is_none() {
    return Ok(vec![])
  }

  // Determine whether to include ORDER BY rank based on the number of words in q or track_name
  let is_ordered = if let Some(query) = q {
    query.split_whitespace().count() > 3
  } else if let (Some(track_name), None, None) = (track_name, artist_name, album_name) {
    track_name.split_whitespace().count() > 3
  } else {
    true
  };

  // Build the subquery with or without ORDER BY rank
  let subquery = if is_ordered {
    indoc! {"SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ? ORDER BY rank LIMIT 20"}.to_string()
  } else {
    indoc! {"SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ? LIMIT 20"}.to_string()
  };

  // Build the complete query using the subquery
  let query = format!(
    "SELECT
      tracks.id,
      tracks.name,
      tracks.artist_name,
      tracks.album_name,
      tracks.duration,
      lyrics.instrumental,
      lyrics.plain_lyrics,
      lyrics.synced_lyrics
    FROM
      ({subquery}) AS search_results
      LEFT JOIN tracks ON search_results.rowid = tracks.id
      LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
    ",
    subquery = subquery
  );

  let mut statement = conn.prepare(&query)?;
  let fts_query = match q {
    Some(q) => prepare_input(&q),
    None => {
      match track_name {
        Some(track_name) => {
          let mut result = format!("(name_lower : \"{}\")", track_name).to_owned();
          if let Some(artist_name) = artist_name {
            result.push_str(format!("AND (artist_name_lower : {})", artist_name).as_ref());
          }
          if let Some(album_name) = album_name {
            result.push_str(format!("AND (album_name_lower : \"{}\")", album_name).as_ref());
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

pub fn flag_track_last_lyrics(track_id: i64, content: &str, conn: &mut Connection) -> Result<()> {
  let now = Utc::now();

  let query = indoc! {"
    INSERT INTO flags (lyrics_id, content, created_at)
    SELECT last_lyrics_id, ?, ? FROM tracks WHERE id = ?
  "};
  let mut statement = conn.prepare(query)?;
  statement.execute((content, now, track_id))?;
  Ok(())
}
