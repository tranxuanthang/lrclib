use crate::backfill_lock::BackfillLock;
use anyhow::{ensure, Context, Result};
use server::{
    entities::lyrics::{LyricsfileBackfillCandidate, LyricsfileBackfillUpdate},
    lyricsfile::{
        lyricsfile_has_word_sync, prepare_legacy_lyrics_preserving_instrumental,
        LyricsfileTrackMetadata,
    },
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
    pub limit: Option<usize>,
    pub dry_run: bool,
    pub sample_size: usize,
    pub progress_every: usize,
}

#[derive(Debug)]
pub struct BackfillSummary {
    pub initial_candidates: u64,
    pub selected: usize,
    pub updated: usize,
    pub generated: usize,
    pub preserved: usize,
    pub raced: usize,
    pub lyricsfile_bytes: u64,
    pub remaining: u64,
    pub elapsed: Duration,
    pub dry_run: bool,
}

struct PreparedUpdate {
    update: LyricsfileBackfillUpdate,
    generated: bool,
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
        let initial_candidates =
            lyrics_repository::count_lyricsfile_backfill_candidates(&mut conn).context(
                "database is missing the Lyricsfile schema; start the server once to apply migrations before running a dry run",
            )?;
        println!("Lyricsfile backfill candidates: {initial_candidates}");
        let sample_size = options
            .limit
            .map_or(options.sample_size, |limit| options.sample_size.min(limit));
        let candidates =
            lyrics_repository::get_lyricsfile_backfill_candidates(0, sample_size, &mut conn)?;
        let prepared = candidates
            .into_iter()
            .map(prepare_update)
            .collect::<Result<Vec<_>>>()?;
        let generated = prepared.iter().filter(|item| item.generated).count();
        let preserved = prepared.len() - generated;
        let lyricsfile_bytes = prepared
            .iter()
            .map(|item| item.update.lyricsfile.len() as u64)
            .sum();
        let summary = BackfillSummary {
            initial_candidates,
            selected: prepared.len(),
            updated: 0,
            generated,
            preserved,
            raced: 0,
            lyricsfile_bytes,
            remaining: initial_candidates,
            elapsed: started.elapsed(),
            dry_run: true,
        };
        print_summary(&summary);
        if summary.selected > 0 {
            let projected_bytes = (u128::from(summary.lyricsfile_bytes)
                * u128::from(summary.initial_candidates))
                / summary.selected as u128;
            println!(
                "Projected Lyricsfile payload if this sample is representative: {projected_bytes} bytes"
            );
        }
        return Ok(summary);
    }

    let _lock = BackfillLock::acquire(
        &database,
        ".lyricsfile-backfill.lock",
        "Lyricsfile backfill",
    )?;
    let pool = server::db::init_db(&database)
        .with_context(|| format!("failed to open database {}", database.display()))?;
    let mut conn = pool.get()?;
    let initial_candidates = lyrics_repository::count_lyricsfile_backfill_candidates(&mut conn)?;
    println!("Lyricsfile backfill candidates: {initial_candidates}");

    let max_rows = options.limit.unwrap_or(usize::MAX);
    let mut selected = 0usize;
    let mut updated = 0usize;
    let mut generated = 0usize;
    let mut preserved = 0usize;
    let mut raced = 0usize;
    let mut lyricsfile_bytes = 0u64;
    let mut next_progress = options.progress_every;
    let mut cursor = 0i64;

    while selected < max_rows {
        let batch_size = options.batch_size.min(max_rows - selected);
        let candidates =
            lyrics_repository::get_lyricsfile_backfill_candidates(cursor, batch_size, &mut conn)?;
        if candidates.is_empty() {
            break;
        }
        let batch_cursor = candidates
            .last()
            .map(|candidate| candidate.id)
            .expect("non-empty candidate batch must have a last row");

        let prepared = candidates
            .into_iter()
            .map(prepare_update)
            .collect::<Result<Vec<_>>>()?;
        let batch_generated = prepared.iter().filter(|item| item.generated).count();
        let batch_preserved = prepared.len() - batch_generated;
        let batch_bytes = prepared
            .iter()
            .map(|item| item.update.lyricsfile.len() as u64)
            .sum::<u64>();
        let updates = prepared
            .into_iter()
            .map(|item| item.update)
            .collect::<Vec<_>>();
        let batch_updated =
            lyrics_repository::apply_lyricsfile_backfill_batch(&updates, &mut conn)?;

        selected += updates.len();
        updated += batch_updated;
        generated += batch_generated;
        preserved += batch_preserved;
        raced += updates.len() - batch_updated;
        lyricsfile_bytes += batch_bytes;
        cursor = batch_cursor;

        if selected >= next_progress {
            println!(
                "Backfill progress: selected={selected}, updated={updated}, generated={generated}, preserved={preserved}"
            );
            while next_progress <= selected {
                next_progress = next_progress.saturating_add(options.progress_every);
                if next_progress == usize::MAX {
                    break;
                }
            }
        }

        if options.sleep_ms > 0 && selected < max_rows {
            tokio::time::sleep(Duration::from_millis(options.sleep_ms)).await;
        }
    }

    let remaining = lyrics_repository::count_lyricsfile_backfill_candidates(&mut conn)?;
    let summary = BackfillSummary {
        initial_candidates,
        selected,
        updated,
        generated,
        preserved,
        raced,
        lyricsfile_bytes,
        remaining,
        elapsed: started.elapsed(),
        dry_run: false,
    };
    print_summary(&summary);
    if options.limit.is_none() {
        ensure!(
            summary.remaining == 0,
            "backfill stopped with {} rows still eligible; rerun the command to process rows that changed concurrently",
            summary.remaining
        );
    }
    Ok(summary)
}

fn prepare_update(candidate: LyricsfileBackfillCandidate) -> Result<PreparedUpdate> {
    let expected_lyricsfile = candidate.lyricsfile.clone();
    if let Some(lyricsfile) = candidate
        .lyricsfile
        .as_ref()
        .filter(|content| !content.trim().is_empty())
    {
        return Ok(PreparedUpdate {
            update: LyricsfileBackfillUpdate {
                id: candidate.id,
                lyricsfile: lyricsfile.clone(),
                expected_lyricsfile,
                has_word_sync: lyricsfile_has_word_sync(lyricsfile),
            },
            generated: false,
        });
    }

    let metadata = LyricsfileTrackMetadata::new(
        &candidate.track_name,
        &candidate.album_name,
        &candidate.artist_name,
        candidate.duration,
    );
    let prepared = prepare_legacy_lyrics_preserving_instrumental(
        &metadata,
        candidate.plain_lyrics.as_deref(),
        candidate.synced_lyrics.as_deref(),
        candidate.instrumental,
    )
    .with_context(|| {
        format!(
            "failed to generate Lyricsfile for lyrics row {}",
            candidate.id
        )
    })?;

    Ok(PreparedUpdate {
        update: LyricsfileBackfillUpdate {
            id: candidate.id,
            lyricsfile: prepared.lyricsfile,
            expected_lyricsfile,
            has_word_sync: prepared.has_word_sync,
        },
        generated: true,
    })
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
        "Lyricsfile {mode} summary: initial={}, selected={}, updated={}, generated={}, preserved={}, raced={}, bytes={}, remaining={}, elapsed={:.2?}",
        summary.initial_candidates,
        summary.selected,
        summary.updated,
        summary.generated,
        summary.preserved,
        summary.raced,
        summary.lyricsfile_bytes,
        summary.remaining,
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
            "lrclib-lyricsfile-backfill-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn options(database: &Path, dry_run: bool, limit: Option<usize>) -> BackfillOptions {
        BackfillOptions {
            database: database.to_path_buf(),
            batch_size: 10,
            sleep_ms: 0,
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
            PathBuf::from(format!("{}.lyricsfile-backfill.lock", database.display())),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[tokio::test]
    async fn dry_run_limit_and_resume_work_end_to_end() {
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
                        ) VALUES ('Line', '[00:01.000]Line', ?, TRUE, TRUE, FALSE, FALSE, ?, 'test', '2020-01-01', '2021-01-01')
                        ",
                        (lyricsfile, track_id),
                    )
                    .unwrap();
            };
            insert_historical_row("Generate Me", None);
            insert_historical_row(
                "Preserve Me",
                Some("version: '1.0'\nmetadata:\n  title: Preserve Me\n  artist: CLI Artist\n  instrumental: false\nlines: []\n"),
            );
        }
        drop(pool);

        let dry_run = run(options(&database, true, Some(1))).await.unwrap();
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.initial_candidates, 2);
        assert_eq!(dry_run.selected, 1);
        assert_eq!(dry_run.updated, 0);
        assert_eq!(dry_run.remaining, 2);
        assert!(
            !PathBuf::from(format!("{}.lyricsfile-backfill.lock", database.display())).exists()
        );

        let canary = run(options(&database, false, Some(1))).await.unwrap();
        assert_eq!(canary.selected, 1);
        assert_eq!(canary.updated, 1);
        assert_eq!(canary.generated, 1);
        assert_eq!(canary.remaining, 1);

        let resumed = run(options(&database, false, None)).await.unwrap();
        assert_eq!(resumed.initial_candidates, 1);
        assert_eq!(resumed.updated, 1);
        assert_eq!(resumed.preserved, 1);
        assert_eq!(resumed.remaining, 0);

        let complete = run(options(&database, false, None)).await.unwrap();
        assert_eq!(complete.initial_candidates, 0);
        assert_eq!(complete.selected, 0);
        assert_eq!(complete.updated, 0);

        cleanup(&database);
    }
}
