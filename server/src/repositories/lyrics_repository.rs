use crate::entities::lyrics::{
    LyricsfileBackfillCandidate, LyricsfileBackfillUpdate, WordSyncBackfillCandidate,
};
use anyhow::{ensure, Context, Result};
use chrono::prelude::*;
use indoc::indoc;
use rusqlite::{Connection, Transaction};

pub fn add_one(
    plain_lyrics: &Option<String>,
    synced_lyrics: &Option<String>,
    lyricsfile: &str,
    track_id: i64,
    instrumental: bool,
    has_word_sync: bool,
    source: &Option<String>,
    conn: &mut Connection,
) -> Result<i64> {
    ensure!(
        !lyricsfile.trim().is_empty(),
        "lyricsfile must not be empty"
    );
    let plain_lyrics = plain_lyrics.as_ref().filter(|s| !s.trim().is_empty());
    let synced_lyrics = synced_lyrics.as_ref().filter(|s| !s.trim().is_empty());

    let now = Utc::now();
    let query = indoc! {"
    INSERT INTO lyrics (
      plain_lyrics,
      synced_lyrics,
      lyricsfile,
      has_plain_lyrics,
      has_synced_lyrics,
      has_lyricsfile,
      has_word_sync,
      instrumental,
      track_id,
      source,
      created_at,
      updated_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
    let mut statement = conn.prepare(query)?;
    let row_id = statement.insert((
        plain_lyrics,
        synced_lyrics,
        lyricsfile,
        plain_lyrics.is_some(),
        synced_lyrics.is_some(),
        true,
        has_word_sync,
        instrumental,
        track_id,
        source,
        now,
        now,
    ))?;
    Ok(row_id)
}

pub fn add_one_tx(
    plain_lyrics: &Option<String>,
    synced_lyrics: &Option<String>,
    lyricsfile: &str,
    track_id: i64,
    instrumental: bool,
    has_word_sync: bool,
    source: &Option<String>,
    conn: &mut Transaction,
) -> Result<i64> {
    ensure!(
        !lyricsfile.trim().is_empty(),
        "lyricsfile must not be empty"
    );
    let plain_lyrics = plain_lyrics.as_ref().filter(|s| !s.trim().is_empty());
    let synced_lyrics = synced_lyrics.as_ref().filter(|s| !s.trim().is_empty());

    let now = Utc::now();
    let query = indoc! {"
    INSERT INTO lyrics (
      plain_lyrics,
      synced_lyrics,
      lyricsfile,
      has_plain_lyrics,
      has_synced_lyrics,
      has_lyricsfile,
      has_word_sync,
      instrumental,
      track_id,
      source,
      created_at,
      updated_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
    let mut statement = conn.prepare(query)?;
    let row_id = statement.insert((
        plain_lyrics,
        synced_lyrics,
        lyricsfile,
        plain_lyrics.is_some(),
        synced_lyrics.is_some(),
        true,
        has_word_sync,
        instrumental,
        track_id,
        source,
        now,
        now,
    ))?;
    Ok(row_id)
}

pub fn get_last_10_mins_lyrics_count(conn: &mut Connection) -> Result<i64> {
    let query = indoc! {"
    SELECT COUNT(*) FROM lyrics
    WHERE created_at > DATETIME('now', '-10 minute')
    AND source = 'lrclib'
  "};
    let mut statement = conn.prepare(query)?;
    let count = statement.query_row([], |row| row.get(0))?;
    Ok(count)
}

pub fn count_lyricsfile_backfill_candidates(conn: &mut Connection) -> Result<u64> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM lyrics WHERE has_lyricsfile = FALSE",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn get_lyricsfile_backfill_candidates(
    after_id: i64,
    batch_size: usize,
    conn: &mut Connection,
) -> Result<Vec<LyricsfileBackfillCandidate>> {
    ensure!(batch_size > 0, "batch size must be greater than zero");
    let batch_size = i64::try_from(batch_size).context("batch size is too large")?;
    let mut statement = conn.prepare_cached(
        "
        SELECT
          lyrics.id,
          lyrics.plain_lyrics,
          lyrics.synced_lyrics,
          lyrics.lyricsfile,
          COALESCE(lyrics.instrumental, FALSE) AS instrumental,
          COALESCE(tracks.name, '') AS track_name,
          COALESCE(tracks.artist_name, '') AS artist_name,
          COALESCE(tracks.album_name, '') AS album_name,
          COALESCE(tracks.duration, 0) AS duration
        FROM lyrics INDEXED BY idx_lyrics_has_lyricsfile
        LEFT JOIN tracks ON tracks.id = lyrics.track_id
        WHERE lyrics.has_lyricsfile = FALSE AND lyrics.id > ?
        ORDER BY lyrics.id
        LIMIT ?
        ",
    )?;
    let mut rows = statement.query((after_id, batch_size))?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        let id = row
            .get("id")
            .context("failed to read id for Lyricsfile backfill candidate")?;
        candidates.push(LyricsfileBackfillCandidate {
            id,
            plain_lyrics: row
                .get("plain_lyrics")
                .with_context(|| format!("failed to read plain_lyrics for lyrics row {id}"))?,
            synced_lyrics: row
                .get("synced_lyrics")
                .with_context(|| format!("failed to read synced_lyrics for lyrics row {id}"))?,
            lyricsfile: row
                .get("lyricsfile")
                .with_context(|| format!("failed to read lyricsfile for lyrics row {id}"))?,
            instrumental: row
                .get("instrumental")
                .with_context(|| format!("failed to read instrumental for lyrics row {id}"))?,
            track_name: row
                .get("track_name")
                .with_context(|| format!("failed to read track name for lyrics row {id}"))?,
            artist_name: row
                .get("artist_name")
                .with_context(|| format!("failed to read artist name for lyrics row {id}"))?,
            album_name: row
                .get("album_name")
                .with_context(|| format!("failed to read album name for lyrics row {id}"))?,
            duration: row
                .get("duration")
                .with_context(|| format!("failed to read duration for lyrics row {id}"))?,
        });
    }

    Ok(candidates)
}

pub fn apply_lyricsfile_backfill_batch(
    updates: &[LyricsfileBackfillUpdate],
    conn: &mut Connection,
) -> Result<usize> {
    for update in updates {
        ensure!(
            !update.lyricsfile.trim().is_empty(),
            "generated lyricsfile for lyrics row {} must not be empty",
            update.id
        );
    }

    let tx = conn.transaction()?;
    let updated = {
        // Deliberately leave created_at and updated_at untouched during backfill.
        let mut statement = tx.prepare_cached(
            "
            UPDATE lyrics
            SET lyricsfile = ?, has_lyricsfile = TRUE, has_word_sync = ?
            WHERE id = ?
              AND has_lyricsfile = FALSE
              AND lyricsfile IS ?
            ",
        )?;
        let mut updated = 0usize;
        for update in updates {
            updated += statement.execute((
                &update.lyricsfile,
                update.has_word_sync,
                update.id,
                &update.expected_lyricsfile,
            ))?;
        }
        updated
    };
    tx.commit()?;

    Ok(updated)
}

pub fn get_word_sync_backfill_candidates(
    after_id: i64,
    batch_size: usize,
    conn: &mut Connection,
) -> Result<Vec<WordSyncBackfillCandidate>> {
    ensure!(batch_size > 0, "batch size must be greater than zero");
    let batch_size = i64::try_from(batch_size).context("batch size is too large")?;
    let mut statement = conn.prepare_cached(
        "
        SELECT
          id,
          CASE WHEN instr(lyricsfile, 'words') > 0 THEN lyricsfile END AS maybe_lyricsfile
        FROM lyrics
        WHERE has_word_sync = FALSE
          AND lyricsfile IS NOT NULL
          AND id > ?
        ORDER BY id
        LIMIT ?
        ",
    )?;
    let mut rows = statement.query((after_id, batch_size))?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        let id = row
            .get("id")
            .context("failed to read id for word-sync backfill candidate")?;
        candidates.push(WordSyncBackfillCandidate {
            id,
            lyricsfile: row
                .get("maybe_lyricsfile")
                .with_context(|| format!("failed to read lyricsfile for lyrics row {id}"))?,
        });
    }

    Ok(candidates)
}

/// Marks candidate rows as word-synced. Callers pass only rows whose Lyricsfile
/// parsed with a non-empty `words` array; the `has_word_sync = FALSE` guard
/// keeps concurrent changes from being overwritten.
pub fn apply_word_sync_backfill_batch(ids: &[i64], conn: &mut Connection) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction()?;
    let updated = {
        // Deliberately leave created_at and updated_at untouched during backfill.
        let mut statement = tx.prepare_cached(
            "
            UPDATE lyrics
            SET has_word_sync = TRUE
            WHERE id = ?
              AND has_word_sync = FALSE
            ",
        )?;
        let mut updated = 0usize;
        for id in ids {
            updated += statement.execute([id])?;
        }
        updated
    };
    tx.commit()?;

    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::migrate,
        entities::lyrics::LyricsfileBackfillUpdate,
        lyricsfile::{parse_lyricsfile, prepare_legacy_lyrics, LyricsfileTrackMetadata},
        repositories::track_repository,
    };

    fn connection() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
    }

    fn insert_historical_lyrics(
        connection: &mut Connection,
        track_name: &str,
        lyricsfile: Option<&str>,
    ) -> (i64, i64) {
        let track_id = track_repository::add_one(
            track_name,
            "Backfill Artist",
            "Backfill Album",
            95.492,
            connection,
        )
        .unwrap();
        connection
            .execute(
                "
                INSERT INTO lyrics (
                  plain_lyrics,
                  synced_lyrics,
                  lyricsfile,
                  has_plain_lyrics,
                  has_synced_lyrics,
                  has_lyricsfile,
                  instrumental,
                  track_id,
                  source,
                  created_at,
                  updated_at
                ) VALUES (?, ?, ?, TRUE, TRUE, FALSE, FALSE, ?, ?, ?, ?)
                ",
                (
                    "First\nSecond",
                    "[00:01.500]First\n[00:03.250]Second",
                    lyricsfile,
                    track_id,
                    "historical-source",
                    "2020-01-02 03:04:05",
                    "2021-02-03 04:05:06",
                ),
            )
            .unwrap();
        (track_id, connection.last_insert_rowid())
    }

    #[test]
    fn backfill_updates_only_lyricsfile_columns_and_is_resumable() {
        let mut connection = connection();
        let (track_id, lyrics_id) =
            insert_historical_lyrics(&mut connection, "Historical Song", None);
        let before = connection
            .query_row(
                "SELECT plain_lyrics, synced_lyrics, instrumental, source, created_at, updated_at FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .unwrap();
        let last_lyrics_id_before: i64 = connection
            .query_row(
                "SELECT last_lyrics_id FROM tracks WHERE id = ?",
                [track_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            count_lyricsfile_backfill_candidates(&mut connection).unwrap(),
            1
        );
        let candidates = get_lyricsfile_backfill_candidates(0, 10, &mut connection).unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.id, lyrics_id);
        assert_eq!(candidate.track_name, "Historical Song");
        let metadata = LyricsfileTrackMetadata::new(
            &candidate.track_name,
            &candidate.album_name,
            &candidate.artist_name,
            candidate.duration,
        );
        let prepared = prepare_legacy_lyrics(
            &metadata,
            candidate.plain_lyrics.as_deref(),
            candidate.synced_lyrics.as_deref(),
            candidate.instrumental,
        )
        .unwrap();
        let update = LyricsfileBackfillUpdate {
            id: lyrics_id,
            lyricsfile: prepared.lyricsfile,
            expected_lyricsfile: None,
            has_word_sync: prepared.has_word_sync,
        };

        assert_eq!(
            apply_lyricsfile_backfill_batch(&[update], &mut connection).unwrap(),
            1
        );

        let after = connection
            .query_row(
                "SELECT plain_lyrics, synced_lyrics, instrumental, source, created_at, updated_at, lyricsfile, has_lyricsfile FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, bool>(7)?,
                    ))
                },
            )
            .unwrap();
        let last_lyrics_id_after: i64 = connection
            .query_row(
                "SELECT last_lyrics_id FROM tracks WHERE id = ?",
                [track_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            (after.0, after.1, after.2, after.3, after.4, after.5),
            before
        );
        assert!(after.7);
        assert_eq!(
            parse_lyricsfile(&after.6).unwrap().plain_lyrics.as_deref(),
            Some("First\nSecond")
        );
        assert_eq!(last_lyrics_id_after, last_lyrics_id_before);
        assert_eq!(
            count_lyricsfile_backfill_candidates(&mut connection).unwrap(),
            0
        );
        assert!(get_lyricsfile_backfill_candidates(0, 10, &mut connection)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn backfill_preserves_existing_lyricsfile_and_rejects_blank_updates() {
        let mut connection = connection();
        let existing = "version: '1.0'\nmetadata:\n  title: Existing\n  artist: Artist\n  instrumental: false\nlines: []\n";
        let (_, lyrics_id) =
            insert_historical_lyrics(&mut connection, "Existing Song", Some(existing));

        let updated = apply_lyricsfile_backfill_batch(
            &[LyricsfileBackfillUpdate {
                id: lyrics_id,
                lyricsfile: existing.to_owned(),
                expected_lyricsfile: Some(existing.to_owned()),
                has_word_sync: false,
            }],
            &mut connection,
        )
        .unwrap();
        assert_eq!(updated, 1);
        let stored: String = connection
            .query_row(
                "SELECT lyricsfile FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, existing);

        let (_, blank_id) = insert_historical_lyrics(&mut connection, "Blank Song", None);
        let result = apply_lyricsfile_backfill_batch(
            &[
                LyricsfileBackfillUpdate {
                    id: blank_id,
                    lyricsfile: "valid".to_owned(),
                    expected_lyricsfile: None,
                    has_word_sync: false,
                },
                LyricsfileBackfillUpdate {
                    id: lyrics_id,
                    lyricsfile: "  ".to_owned(),
                    expected_lyricsfile: Some(existing.to_owned()),
                    has_word_sync: false,
                },
            ],
            &mut connection,
        );
        assert!(result.is_err());
        let has_lyricsfile: bool = connection
            .query_row(
                "SELECT has_lyricsfile FROM lyrics WHERE id = ?",
                [blank_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_lyricsfile);
    }

    #[test]
    fn backfill_uses_indexed_keyset_pagination() {
        let mut connection = connection();
        let (_, first_id) = insert_historical_lyrics(&mut connection, "First Page", None);
        let (_, second_id) = insert_historical_lyrics(&mut connection, "Second Page", None);
        connection.execute_batch("ANALYZE").unwrap();

        let page = get_lyricsfile_backfill_candidates(first_id, 10, &mut connection).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, second_id);

        let mut statement = connection
            .prepare(
                "
                EXPLAIN QUERY PLAN
                SELECT lyrics.id
                FROM lyrics INDEXED BY idx_lyrics_has_lyricsfile
                LEFT JOIN tracks ON tracks.id = lyrics.track_id
                WHERE lyrics.has_lyricsfile = FALSE AND lyrics.id > 0
                ORDER BY lyrics.id
                LIMIT 10
                ",
            )
            .unwrap();
        let details = statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(details
            .iter()
            .any(|detail| detail.contains("idx_lyrics_has_lyricsfile")));
    }

    #[test]
    fn backfill_compare_and_set_does_not_overwrite_concurrent_changes() {
        let mut connection = connection();
        let (_, lyrics_id) = insert_historical_lyrics(&mut connection, "Concurrent Song", None);
        let concurrent = "version: '1.0'\nmetadata:\n  title: Concurrent\n  artist: Artist\n  instrumental: false\nlines: []\n";
        connection
            .execute(
                "UPDATE lyrics SET lyricsfile = ? WHERE id = ?",
                (concurrent, lyrics_id),
            )
            .unwrap();

        let updated = apply_lyricsfile_backfill_batch(
            &[LyricsfileBackfillUpdate {
                id: lyrics_id,
                lyricsfile: "stale generated content".to_owned(),
                expected_lyricsfile: None,
                has_word_sync: false,
            }],
            &mut connection,
        )
        .unwrap();
        assert_eq!(updated, 0);
        let stored: String = connection
            .query_row(
                "SELECT lyricsfile FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, concurrent);
        let has_lyricsfile: bool = connection
            .query_row(
                "SELECT has_lyricsfile FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_lyricsfile);
    }

    #[test]
    fn backfill_replaces_every_kind_of_whitespace_only_lyricsfile() {
        let mut connection = connection();
        let whitespace = " \r\n\t";
        let (_, lyrics_id) =
            insert_historical_lyrics(&mut connection, "Whitespace Song", Some(whitespace));
        let generated = "version: '1.0'\nmetadata:\n  title: Whitespace Song\n  artist: Artist\n  instrumental: false\nlines: []\n";

        let updated = apply_lyricsfile_backfill_batch(
            &[LyricsfileBackfillUpdate {
                id: lyrics_id,
                lyricsfile: generated.to_owned(),
                expected_lyricsfile: Some(whitespace.to_owned()),
                has_word_sync: false,
            }],
            &mut connection,
        )
        .unwrap();
        assert_eq!(updated, 1);
        let stored: String = connection
            .query_row(
                "SELECT lyricsfile FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, generated);
    }

    #[test]
    fn add_one_persists_has_word_sync() {
        let mut connection = connection();
        let track_id = track_repository::add_one(
            "Word Sync Song",
            "Backfill Artist",
            "Backfill Album",
            95.492,
            &mut connection,
        )
        .unwrap();
        let lyricsfile = "version: '1.0'\nmetadata:\n  title: Word Sync Song\n  artist: Backfill Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n    words:\n      - text: Hello\n        start_ms: 1000\n      - text: world\n        start_ms: 1500\n";
        let lyrics_id = add_one(
            &None,
            &None,
            lyricsfile,
            track_id,
            false,
            true,
            &Some("test".to_owned()),
            &mut connection,
        )
        .unwrap();

        let stored: bool = connection
            .query_row(
                "SELECT has_word_sync FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored);
    }

    #[test]
    fn lyricsfile_backfill_copies_preserved_word_sync_state() {
        let mut connection = connection();
        let existing = "version: '1.0'\nmetadata:\n  title: Preserved Words\n  artist: Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n    words:\n      - text: Hello\n        start_ms: 1000\n      - text: world\n        start_ms: 1500\n";
        let (_, lyrics_id) =
            insert_historical_lyrics(&mut connection, "Preserved Words", Some(existing));

        let updated = apply_lyricsfile_backfill_batch(
            &[LyricsfileBackfillUpdate {
                id: lyrics_id,
                lyricsfile: existing.to_owned(),
                expected_lyricsfile: Some(existing.to_owned()),
                has_word_sync: true,
            }],
            &mut connection,
        )
        .unwrap();
        assert_eq!(updated, 1);

        let (has_lyricsfile, has_word_sync): (bool, bool) = connection
            .query_row(
                "SELECT has_lyricsfile, has_word_sync FROM lyrics WHERE id = ?",
                [lyrics_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(has_lyricsfile);
        assert!(has_word_sync);
    }

    #[test]
    fn word_sync_backfill_prefilters_confirms_and_marks_candidates() {
        let mut connection = connection();
        let word_synced = "version: '1.0'\nmetadata:\n  title: Word\n  artist: Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n    words:\n      - text: Hello\n        start_ms: 1000\n      - text: world\n        start_ms: 1500\n";
        let false_positive = "version: '1.0'\nmetadata:\n  title: Plain\n  artist: Artist\n  instrumental: false\nlines: []\nplain: |\n  talking about words: nothing timed\n";
        let line_synced = "version: '1.0'\nmetadata:\n  title: Line\n  artist: Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n";
        let (_, word_id) =
            insert_historical_lyrics(&mut connection, "Word Song", Some(word_synced));
        let (_, plain_id) =
            insert_historical_lyrics(&mut connection, "Plain Song", Some(false_positive));
        let (_, line_id) =
            insert_historical_lyrics(&mut connection, "Line Song", Some(line_synced));
        let (_, missing_id) = insert_historical_lyrics(&mut connection, "Missing Song", None);

        let candidates = get_word_sync_backfill_candidates(0, 10, &mut connection).unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            vec![word_id, plain_id, line_id]
        );
        assert!(candidates[0].lyricsfile.is_some());
        assert!(candidates[1].lyricsfile.is_some());
        assert!(candidates[2].lyricsfile.is_none());

        // Only the confirmed word-synced row is marked; the hinted plain-only
        // row parses without words and stays FALSE.
        assert_eq!(
            apply_word_sync_backfill_batch(&[word_id], &mut connection).unwrap(),
            1
        );
        assert_eq!(
            apply_word_sync_backfill_batch(&[word_id], &mut connection).unwrap(),
            0
        );

        let flags = connection
            .query_row(
                "SELECT SUM(has_word_sync), COUNT(*) FROM lyrics",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(flags, (1, 4));
        let missing_flag: bool = connection
            .query_row(
                "SELECT has_word_sync FROM lyrics WHERE id = ?",
                [missing_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!missing_flag);

        // The marked row drops out of later pages.
        let next = get_word_sync_backfill_candidates(word_id, 10, &mut connection).unwrap();
        assert_eq!(
            next.iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            vec![plain_id, line_id]
        );
        assert!(
            get_word_sync_backfill_candidates(line_id, 10, &mut connection)
                .unwrap()
                .is_empty()
        );
    }
}
