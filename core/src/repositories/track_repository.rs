use crate::{
    entities::{
        lyrics::SimpleLyrics,
        track::{
            SimpleTrack, SimpleTrackSummary, TrackNormalizationCandidate, TrackNormalizationUpdate,
            TrackSearchDoc,
        },
    },
    text::prepare_input,
};
use anyhow::{ensure, Context, Result};
use chrono::prelude::*;
use indoc::indoc;
use rusqlite::params_from_iter;
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::collections::HashMap;

/// Maps a row containing track columns joined with the current lyrics row.
fn simple_track_from_row(row: &rusqlite::Row) -> rusqlite::Result<SimpleTrack> {
    let instrumental = row.get::<_, Option<bool>>("instrumental")?.unwrap_or(false);
    let has_word_sync = row
        .get::<_, Option<bool>>("has_word_sync")?
        .unwrap_or(false);

    let last_lyrics = SimpleLyrics {
        plain_lyrics: row.get("plain_lyrics")?,
        synced_lyrics: row.get("synced_lyrics")?,
        lyricsfile: row.get("lyricsfile")?,
        instrumental,
        has_word_sync,
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
      lyrics.synced_lyrics,
      lyrics.lyricsfile,
      lyrics.has_word_sync
    FROM
      tracks
      LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
    WHERE
      tracks.id = ?
  "};
    let mut statement = conn.prepare_cached(query)?;
    let row = statement
        .query_row([track_id], simple_track_from_row)
        .optional()?;
    Ok(row)
}

pub fn get_track_id_by_metadata(
    track_name: &str,
    artist_name: &str,
    album_name: &str,
    duration: f64,
    conn: &mut Connection,
) -> Result<Option<i64>> {
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
    let mut statement = conn.prepare_cached(query)?;
    let row = statement
        .query_row(
            (
                track_name_lower,
                artist_name_lower,
                album_name_lower,
                duration - 2.0,
                duration + 2.0,
            ),
            |row| Ok(row.get("id")?),
        )
        .optional()?;
    Ok(row)
}

pub fn get_track_id_by_metadata_tx(
    track_name: &str,
    artist_name: &str,
    album_name: &str,
    duration: f64,
    conn: &mut Transaction,
) -> Result<Option<i64>> {
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
    let mut statement = conn.prepare_cached(query)?;
    let row = statement
        .query_row(
            (
                track_name_lower,
                artist_name_lower,
                album_name_lower,
                duration - 2.0,
                duration + 2.0,
            ),
            |row| Ok(row.get("id")?),
        )
        .optional()?;
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
      lyrics.synced_lyrics,
      lyrics.lyricsfile,
      lyrics.has_word_sync
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
        where_clauses.push("tracks.duration >= ?".to_string());
        where_clauses.push("tracks.duration <= ?".to_string());
        params.push((dur - 2.0).into());
        params.push((dur + 2.0).into());
    }

    // Conditionally add album_name to the query
    if let Some(album_name_lower) = album_name_lower {
        where_clauses.push("tracks.album_name_lower = ?".to_string());
        params.push(album_name_lower.to_string().into());
    }

    // Combine all parts of the query
    // Prefer tracks with synced lyrics
    let order_by = "tracks.id";

    let query = format!(
        "{select} WHERE {where_clause} ORDER BY {order_by} LIMIT 1",
        select = select_query,
        where_clause = where_clauses.join(" AND "),
        order_by = order_by
    );

    // Prepare and execute the statement
    let mut statement = conn.prepare_cached(&query)?;
    let row = statement
        .query_row(
            params_from_iter(params.iter().map(|v| v as &dyn rusqlite::ToSql)),
            simple_track_from_row,
        )
        .optional()?;

    Ok(row)
}

/// Fetches tracks (joined with their current lyrics) by id, returning them in
/// the same order as `track_ids`.
///
/// This backs the Tantivy search path, where the id order carries the ranking.
/// Callers pass at most `search_index::SEARCH_RESULT_LIMIT` ids, so the
/// per-length statement cache stays small.
pub fn get_tracks_by_ids(track_ids: &[i64], conn: &mut Connection) -> Result<Vec<SimpleTrack>> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; track_ids.len()].join(", ");
    let query = format!(
        "SELECT
      tracks.id,
      tracks.name,
      tracks.artist_name,
      tracks.album_name,
      tracks.duration,
      lyrics.instrumental,
      lyrics.plain_lyrics,
      lyrics.synced_lyrics,
      lyrics.lyricsfile,
      lyrics.has_word_sync
    FROM
      tracks
      LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
    WHERE
      tracks.id IN ({placeholders})"
    );

    let mut statement = conn.prepare_cached(&query)?;
    let rows = statement.query_map(params_from_iter(track_ids.iter()), simple_track_from_row)?;
    let mut tracks = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let rank_by_id: HashMap<i64, usize> = track_ids
        .iter()
        .enumerate()
        .map(|(rank, track_id)| (*track_id, rank))
        .collect();
    tracks.sort_by_key(|track| rank_by_id.get(&track.id).copied().unwrap_or(usize::MAX));

    Ok(tracks)
}

/// Fetches track metadata and lyrics availability flags by id, without
/// reading the lyrics text. The v2 search path uses this to keep result
/// payloads and SQL rows small.
pub fn get_track_summaries_by_ids(
    track_ids: &[i64],
    conn: &mut Connection,
) -> Result<Vec<SimpleTrackSummary>> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; track_ids.len()].join(", ");
    let query = format!(
        "SELECT
      tracks.id,
      tracks.name,
      tracks.artist_name,
      tracks.album_name,
      tracks.duration,
      lyrics.instrumental,
      lyrics.has_word_sync,
      lyrics.has_plain_lyrics,
      lyrics.has_synced_lyrics,
      lyrics.has_lyricsfile
    FROM
      tracks
      LEFT JOIN lyrics ON tracks.last_lyrics_id = lyrics.id
    WHERE
      tracks.id IN ({placeholders})"
    );

    let mut statement = conn.prepare_cached(&query)?;
    let rows = statement.query_map(params_from_iter(track_ids.iter()), |row| {
        Ok(SimpleTrackSummary {
            id: row.get("id")?,
            name: row.get("name")?,
            artist_name: row.get("artist_name")?,
            album_name: row.get("album_name")?,
            duration: row.get("duration")?,
            instrumental: row.get::<_, Option<bool>>("instrumental")?.unwrap_or(false),
            has_word_sync: row
                .get::<_, Option<bool>>("has_word_sync")?
                .unwrap_or(false),
            has_plain_lyrics: row
                .get::<_, Option<bool>>("has_plain_lyrics")?
                .unwrap_or(false),
            has_synced_lyrics: row
                .get::<_, Option<bool>>("has_synced_lyrics")?
                .unwrap_or(false),
            has_lyricsfile: row
                .get::<_, Option<bool>>("has_lyricsfile")?
                .unwrap_or(false),
        })
    })?;
    let mut summaries = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let rank_by_id: HashMap<i64, usize> = track_ids
        .iter()
        .enumerate()
        .map(|(rank, track_id)| (*track_id, rank))
        .collect();
    summaries.sort_by_key(|track| rank_by_id.get(&track.id).copied().unwrap_or(usize::MAX));

    Ok(summaries)
}

/// Returns tracks with `id > after_id` in ascending id order, limited to `limit` rows.
///
/// This is the tailing query used by the search indexer. It relies on
/// `tracks.id` being monotonically increasing (AUTOINCREMENT) and the indexed
/// name columns being immutable after insert.
pub fn get_track_search_docs_after(
    after_id: i64,
    limit: usize,
    conn: &mut Connection,
) -> Result<Vec<TrackSearchDoc>> {
    let query = indoc! {"
    SELECT
      id,
      name_lower,
      artist_name_lower,
      album_name_lower
    FROM
      tracks
    WHERE
      id > ?
    ORDER BY
      id
    LIMIT ?
  "};
    let mut statement = conn.prepare_cached(query)?;
    let rows = statement.query_map((after_id, limit as i64), |row| {
        Ok(TrackSearchDoc {
            id: row.get("id")?,
            name_lower: row.get("name_lower")?,
            artist_name_lower: row.get("artist_name_lower")?,
            album_name_lower: row.get("album_name_lower")?,
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Returns tracks with missing normalized metadata in id order.
///
/// Candidates have at least one empty/NULL `*_lower` column and a non-blank
/// raw value for that column. The OR conditions use the per-column indexes.
pub fn get_track_normalization_candidates(
    after_id: i64,
    batch_size: usize,
    conn: &mut Connection,
) -> Result<Vec<TrackNormalizationCandidate>> {
    ensure!(batch_size > 0, "batch size must be greater than zero");
    let batch_size = i64::try_from(batch_size).context("batch size is too large")?;
    let mut statement = conn.prepare_cached(
        "
        SELECT
          id,
          name,
          name_lower,
          artist_name,
          artist_name_lower,
          album_name,
          album_name_lower
        FROM
          tracks
        WHERE
          id > ?
          AND (
            ((name_lower = '' OR name_lower IS NULL) AND name IS NOT NULL AND trim(name) <> '')
            OR ((artist_name_lower = '' OR artist_name_lower IS NULL) AND artist_name IS NOT NULL AND trim(artist_name) <> '')
            OR ((album_name_lower = '' OR album_name_lower IS NULL) AND album_name IS NOT NULL AND trim(album_name) <> '')
          )
        ORDER BY
          id
        LIMIT ?
        ",
    )?;
    let mut rows = statement.query((after_id, batch_size))?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        let id = row
            .get("id")
            .context("failed to read id for track normalization candidate")?;
        candidates.push(TrackNormalizationCandidate {
            id,
            name: row
                .get("name")
                .with_context(|| format!("failed to read name for track {id}"))?,
            name_lower: row
                .get("name_lower")
                .with_context(|| format!("failed to read name_lower for track {id}"))?,
            artist_name: row
                .get("artist_name")
                .with_context(|| format!("failed to read artist_name for track {id}"))?,
            artist_name_lower: row
                .get("artist_name_lower")
                .with_context(|| format!("failed to read artist_name_lower for track {id}"))?,
            album_name: row
                .get("album_name")
                .with_context(|| format!("failed to read album_name for track {id}"))?,
            album_name_lower: row
                .get("album_name_lower")
                .with_context(|| format!("failed to read album_name_lower for track {id}"))?,
        });
    }

    Ok(candidates)
}

/// Applies guarded normalization updates, leaving timestamps untouched.
///
/// Each update only succeeds when all three current normalized values still
/// match the values read for the candidate, so a concurrent edit cannot be
/// overwritten.
pub fn apply_track_normalization_backfill_batch(
    updates: &[TrackNormalizationUpdate],
    conn: &mut Connection,
) -> Result<usize> {
    if updates.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction()?;
    let updated = {
        let mut statement = tx.prepare_cached(
            "
            UPDATE tracks
            SET
              name_lower = ?1,
              artist_name_lower = ?2,
              album_name_lower = ?3
            WHERE
              id = ?4
              AND name_lower IS ?5
              AND artist_name_lower IS ?6
              AND album_name_lower IS ?7
              AND (
                name_lower = '' OR name_lower IS NULL
                OR artist_name_lower = '' OR artist_name_lower IS NULL
                OR album_name_lower = '' OR album_name_lower IS NULL
              )
            ",
        )?;
        let mut updated = 0usize;
        for update in updates {
            updated += statement.execute((
                &update.name_lower,
                &update.artist_name_lower,
                &update.album_name_lower,
                update.id,
                &update.expected_name_lower,
                &update.expected_artist_name_lower,
                &update.expected_album_name_lower,
            ))?;
        }
        updated
    };
    tx.commit()?;

    Ok(updated)
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
    let mut statement = conn.prepare_cached(query)?;
    let row_id = statement.insert((
        track_name,
        track_name_lower,
        artist_name,
        artist_name_lower,
        album_name,
        album_name_lower,
        duration,
        now,
        now,
    ))?;
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
    let mut statement = conn.prepare_cached(query)?;
    let row_id = statement.insert((
        track_name,
        track_name_lower,
        artist_name,
        artist_name_lower,
        album_name,
        album_name_lower,
        duration,
        now,
        now,
    ))?;
    Ok(row_id)
}

pub fn flag_track_last_lyrics(track_id: i64, content: &str, conn: &mut Connection) -> Result<()> {
    let now = Utc::now();

    let query = indoc! {"
    INSERT INTO flags (lyrics_id, content, created_at)
    SELECT last_lyrics_id, ?, ? FROM tracks WHERE id = ?
  "};
    let mut statement = conn.prepare_cached(query)?;
    statement.execute((content, now, track_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn test_pool() -> (r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("lrclib-repo-test-{}.sqlite3", Uuid::new_v4()));
        let pool = init_db(&path).expect("failed to initialize test database");
        (pool, path)
    }

    fn remove_test_database(path: &std::path::Path) {
        for path in [
            path.to_path_buf(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn get_tracks_by_ids_preserves_requested_order() {
        let (pool, path) = test_pool();
        let mut conn = pool.get().unwrap();
        let first = add_one("First Song", "Artist", "Album", 180.0, &mut conn).unwrap();
        let second = add_one("Second Song", "Artist", "Album", 200.0, &mut conn).unwrap();
        let third = add_one("Third Song", "Artist", "Album", 210.0, &mut conn).unwrap();

        let tracks = get_tracks_by_ids(&[third, first, second], &mut conn).unwrap();

        assert_eq!(
            tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![third, first, second]
        );
        assert_eq!(tracks[0].name.as_deref(), Some("Third Song"));

        drop(conn);
        drop(pool);
        remove_test_database(&path);
    }

    #[test]
    fn get_tracks_by_ids_skips_missing_ids_and_handles_empty_input() {
        let (pool, path) = test_pool();
        let mut conn = pool.get().unwrap();
        let track_id = add_one("Only Song", "Artist", "Album", 180.0, &mut conn).unwrap();

        let tracks = get_tracks_by_ids(&[9999, track_id], &mut conn).unwrap();
        assert_eq!(
            tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![track_id]
        );

        assert!(get_tracks_by_ids(&[], &mut conn).unwrap().is_empty());

        drop(conn);
        drop(pool);
        remove_test_database(&path);
    }

    #[test]
    fn track_normalization_backfill_finds_and_updates_empty_fields() {
        let (pool, path) = test_pool();
        let mut conn = pool.get().unwrap();
        let track_id = add_one("?", "Outkast", "Stankonia", 89.0, &mut conn).unwrap();
        conn.execute(
            "UPDATE tracks SET name_lower = NULL WHERE id = ?",
            [track_id],
        )
        .unwrap();

        let candidates = get_track_normalization_candidates(0, 10, &mut conn).unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.name_lower, None);

        let updated = apply_track_normalization_backfill_batch(
            &[TrackNormalizationUpdate {
                id: candidate.id,
                expected_name_lower: candidate.name_lower.clone(),
                expected_artist_name_lower: candidate.artist_name_lower.clone(),
                expected_album_name_lower: candidate.album_name_lower.clone(),
                name_lower: Some("?".to_owned()),
                artist_name_lower: Some("outkast".to_owned()),
                album_name_lower: Some("stankonia".to_owned()),
            }],
            &mut conn,
        )
        .unwrap();
        assert_eq!(updated, 1);
        assert!(get_track_normalization_candidates(0, 10, &mut conn)
            .unwrap()
            .is_empty());

        let normalized: (String, String, String) = conn
            .query_row(
                "SELECT name_lower, artist_name_lower, album_name_lower FROM tracks WHERE id = ?",
                [track_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            normalized,
            ("?".to_owned(), "outkast".to_owned(), "stankonia".to_owned())
        );

        drop(conn);
        drop(pool);
        remove_test_database(&path);
    }

    #[test]
    fn punctuation_only_metadata_round_trips() {
        let (pool, path) = test_pool();
        let mut conn = pool.get().unwrap();
        let track_id = add_one("?", "Outkast", "Stankonia", 89.0, &mut conn).unwrap();

        let track = get_track_by_metadata("?", "outkast", Some("stankonia"), None, &mut conn)
            .unwrap()
            .expect("punctuation-only track should be found");

        assert_eq!(track.id, track_id);

        drop(conn);
        drop(pool);
        remove_test_database(&path);
    }
}
