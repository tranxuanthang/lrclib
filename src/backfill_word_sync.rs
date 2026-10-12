use crate::backfill_lock::BackfillLock;
use anyhow::{ensure, Context, Result};
use server::{
    entities::lyrics::WordSyncBackfillCandidate, lyricsfile::parse_lyricsfile,
    repositories::lyrics_repository,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct BackfillOptions {
    pub database: PathBuf,
    pub batch_size: usize,
    pub sleep_ms: u64,
    pub start_after_id: i64,
    pub limit: Option<usize>,
    pub dry_run: bool,
    pub sample_size: usize,
    pub progress_every: usize,
}

#[derive(Debug)]
pub struct BackfillSummary {
    pub scanned: usize,
    pub hinted: usize,
    pub confirmed: usize,
    pub false_positives: usize,
    pub parse_failures: usize,
    pub updated: usize,
    pub raced: usize,
    pub last_id: i64,
    pub elapsed: Duration,
    pub dry_run: bool,
}

#[derive(Debug, Default)]
struct BatchStats {
    scanned: usize,
    hinted: usize,
    confirmed: usize,
    false_positives: usize,
    parse_failures: usize,
    last_id: i64,
}

pub async fn run(options: BackfillOptions) -> Result<BackfillSummary> {
    validate_options(&options)?;
    ensure!(
        options.database.is_file(),
        "database file does not exist: {}",
        options.database.display()
    );
    let database = std::fs::canonicalize(&options.database).with_context(|| {
        format!(
            "failed to resolve database path {}",
            options.database.display()
        )
    })?;
    let started = Instant::now();

    if options.dry_run {
        let mut conn = server::db::open_db_read_only(&database)
            .with_context(|| format!("failed to open database {} read-only", database.display()))?;
        let sample_size = options
            .limit
            .map_or(options.sample_size, |limit| options.sample_size.min(limit));
        let candidates = lyrics_repository::get_word_sync_backfill_candidates(
            options.start_after_id,
            sample_size,
            &mut conn,
        )
        .context(
            "database is missing the word-sync schema; start the server once to apply migrations before running a dry run",
        )?;
        let (_, stats) = analyze_candidates(candidates);
        let summary = BackfillSummary {
            scanned: stats.scanned,
            hinted: stats.hinted,
            confirmed: stats.confirmed,
            false_positives: stats.false_positives,
            parse_failures: stats.parse_failures,
            updated: 0,
            raced: 0,
            last_id: stats.last_id,
            elapsed: started.elapsed(),
            dry_run: true,
        };
        print_summary(&summary);
        return Ok(summary);
    }

    let _lock = BackfillLock::acquire(&database, ".word-sync-backfill.lock", "word-sync backfill")?;
    let pool = server::db::init_db(&database)
        .with_context(|| format!("failed to open database {}", database.display()))?;
    let mut conn = pool.get()?;

    let max_rows = options.limit.unwrap_or(usize::MAX);
    let mut stats = BatchStats::default();
    let mut updated = 0usize;
    let mut raced = 0usize;
    let mut next_progress = options.progress_every;
    let mut cursor = options.start_after_id;

    while stats.scanned < max_rows {
        let batch_size = options.batch_size.min(max_rows - stats.scanned);
        let candidates =
            lyrics_repository::get_word_sync_backfill_candidates(cursor, batch_size, &mut conn)?;
        if candidates.is_empty() {
            break;
        }
        let batch_last_id = candidates
            .last()
            .map(|candidate| candidate.id)
            .expect("non-empty candidate batch must have a last row");

        let (confirmed_ids, batch_stats) = analyze_candidates(candidates);
        let batch_updated =
            lyrics_repository::apply_word_sync_backfill_batch(&confirmed_ids, &mut conn)?;

        stats.scanned += batch_stats.scanned;
        stats.hinted += batch_stats.hinted;
        stats.confirmed += batch_stats.confirmed;
        stats.false_positives += batch_stats.false_positives;
        stats.parse_failures += batch_stats.parse_failures;
        stats.last_id = batch_stats.last_id;
        updated += batch_updated;
        raced += confirmed_ids.len() - batch_updated;
        cursor = batch_last_id;

        if stats.scanned >= next_progress {
            println!(
                "Word-sync backfill progress: scanned={}, hinted={}, confirmed={}, updated={}",
                stats.scanned, stats.hinted, stats.confirmed, updated
            );
            while next_progress <= stats.scanned {
                next_progress = next_progress.saturating_add(options.progress_every);
                if next_progress == usize::MAX {
                    break;
                }
            }
        }

        if options.sleep_ms > 0 && stats.scanned < max_rows {
            tokio::time::sleep(Duration::from_millis(options.sleep_ms)).await;
        }
    }

    let summary = BackfillSummary {
        scanned: stats.scanned,
        hinted: stats.hinted,
        confirmed: stats.confirmed,
        false_positives: stats.false_positives,
        parse_failures: stats.parse_failures,
        updated,
        raced,
        last_id: stats.last_id,
        elapsed: started.elapsed(),
        dry_run: false,
    };
    print_summary(&summary);
    Ok(summary)
}

fn analyze_candidates(candidates: Vec<WordSyncBackfillCandidate>) -> (Vec<i64>, BatchStats) {
    let mut stats = BatchStats {
        scanned: candidates.len(),
        ..BatchStats::default()
    };
    let mut confirmed_ids = Vec::new();

    for candidate in candidates {
        stats.last_id = candidate.id;
        let Some(lyricsfile) = candidate.lyricsfile else {
            continue;
        };
        stats.hinted += 1;
        match parse_lyricsfile(&lyricsfile) {
            Ok(parsed) if parsed.has_word_sync => {
                stats.confirmed += 1;
                confirmed_ids.push(candidate.id);
            }
            Ok(_) => stats.false_positives += 1,
            Err(_) => stats.parse_failures += 1,
        }
    }

    (confirmed_ids, stats)
}

fn validate_options(options: &BackfillOptions) -> Result<()> {
    ensure!(
        options.batch_size > 0,
        "batch size must be greater than zero"
    );
    ensure!(
        options.progress_every > 0,
        "progress interval must be greater than zero"
    );
    ensure!(
        options.start_after_id >= 0,
        "start after id must not be negative"
    );
    if let Some(limit) = options.limit {
        ensure!(limit > 0, "row limit must be greater than zero");
    }
    if options.dry_run {
        ensure!(
            options.sample_size > 0,
            "dry-run sample size must be greater than zero"
        );
    }
    Ok(())
}

fn print_summary(summary: &BackfillSummary) {
    let mode = if summary.dry_run {
        "dry run"
    } else {
        "backfill"
    };
    println!(
        "Word-sync {mode} summary: scanned={}, hinted={}, confirmed={}, false_positives={}, parse_failures={}, updated={}, raced={}, last_id={}, elapsed={:.2?}",
        summary.scanned,
        summary.hinted,
        summary.confirmed,
        summary.false_positives,
        summary.parse_failures,
        summary.updated,
        summary.raced,
        summary.last_id,
        summary.elapsed,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use server::repositories::track_repository;
    use std::{
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn test_database_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lrclib-word-sync-backfill-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn options(database: &Path, dry_run: bool, limit: Option<usize>) -> BackfillOptions {
        BackfillOptions {
            database: database.to_path_buf(),
            batch_size: 10,
            sleep_ms: 0,
            start_after_id: 0,
            limit,
            dry_run,
            sample_size: 10,
            progress_every: 1,
        }
    }

    fn cleanup(database: &Path) {
        for path in [
            database.to_path_buf(),
            PathBuf::from(format!("{}-wal", database.display())),
            PathBuf::from(format!("{}-shm", database.display())),
            PathBuf::from(format!("{}.word-sync-backfill.lock", database.display())),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[tokio::test]
    async fn dry_run_and_backfill_mark_only_word_synced_rows() {
        let database = test_database_path();
        let pool = server::db::init_db(&database).unwrap();
        {
            let mut connection = pool.get().unwrap();
            let mut insert_historical_row = |title: &str, lyricsfile: Option<&str>| {
                let track_id = track_repository::add_one(
                    title,
                    "CLI Artist",
                    "CLI Album",
                    180.125,
                    &mut connection,
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
                        ) VALUES ('Line', '[00:01.000]Line', ?, TRUE, TRUE, TRUE, FALSE, ?, 'test', '2020-01-01', '2021-01-01')
                        ",
                        (lyricsfile, track_id),
                    )
                    .unwrap();
            };
            insert_historical_row(
                "Word Song",
                Some(
                    "version: '1.0'\nmetadata:\n  title: Word Song\n  artist: CLI Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n    words:\n      - text: 'Hello '\n        start_ms: 1000\n      - text: world\n        start_ms: 1500\n",
                ),
            );
            insert_historical_row(
                "False Positive",
                Some(
                    "version: '1.0'\nmetadata:\n  title: False Positive\n  artist: CLI Artist\n  instrumental: false\nlines: []\nplain: |\n  talking about words: nothing timed\n",
                ),
            );
            insert_historical_row(
                "Line Song",
                Some(
                    "version: '1.0'\nmetadata:\n  title: Line Song\n  artist: CLI Artist\n  instrumental: false\nlines:\n  - text: Hello world\n    start_ms: 1000\n",
                ),
            );
            insert_historical_row("Missing Song", None);
        }
        drop(pool);

        let dry_run = run(options(&database, true, None)).await.unwrap();
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.scanned, 3);
        assert_eq!(dry_run.hinted, 2);
        assert_eq!(dry_run.confirmed, 1);
        assert_eq!(dry_run.false_positives, 1);
        assert_eq!(dry_run.updated, 0);
        assert!(!PathBuf::from(format!("{}.word-sync-backfill.lock", database.display())).exists());

        let completed = run(options(&database, false, None)).await.unwrap();
        assert_eq!(completed.scanned, 3);
        assert_eq!(completed.updated, 1);
        assert_eq!(completed.confirmed, 1);
        assert_eq!(completed.raced, 0);

        let pool = server::db::init_db(&database).unwrap();
        let connection = pool.get().unwrap();
        let word_flag: bool = connection
            .query_row(
                "SELECT has_word_sync FROM lyrics WHERE plain_lyrics = 'Line' AND lyricsfile LIKE '%Word Song%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(word_flag);
        let timestamps: (String, String) = connection
            .query_row(
                "SELECT created_at, updated_at FROM lyrics WHERE lyricsfile LIKE '%Word Song%'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            timestamps,
            ("2020-01-01".to_owned(), "2021-01-01".to_owned())
        );
        drop(connection);
        drop(pool);

        // The marked row drops out of the next sweep; the false positive is
        // re-parsed but never marked.
        let resumed = run(options(&database, false, None)).await.unwrap();
        assert_eq!(resumed.scanned, 2);
        assert_eq!(resumed.hinted, 1);
        assert_eq!(resumed.confirmed, 0);
        assert_eq!(resumed.updated, 0);

        cleanup(&database);
    }
}
