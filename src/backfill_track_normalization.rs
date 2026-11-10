use crate::backfill_lock::BackfillLock;
use anyhow::{bail, ensure, Context, Result};
use lrclib_core::{
    entities::track::{TrackNormalizationCandidate, TrackNormalizationUpdate},
    repositories::track_repository,
    text::prepare_input,
};
use std::{
    io::{self, IsTerminal, Write},
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
    pub yes: bool,
}

#[derive(Debug)]
pub struct BackfillSummary {
    pub scanned: usize,
    pub changed: usize,
    pub fields_changed: usize,
    pub updated: usize,
    pub raced: usize,
    pub last_id: i64,
    pub elapsed: Duration,
    pub dry_run: bool,
}

#[derive(Debug)]
struct PlannedUpdate {
    update: TrackNormalizationUpdate,
    fields_changed: usize,
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
    let sample_size = options
        .limit
        .map_or(options.sample_size, |limit| options.sample_size.min(limit));

    if options.dry_run {
        let mut conn = lrclib_core::db::open_db_read_only(&database)
            .with_context(|| format!("failed to open database {} read-only", database.display()))?;
        let candidates = track_repository::get_track_normalization_candidates(
            options.start_after_id,
            sample_size,
            &mut conn,
        )
        .context(
            "failed to query track normalization candidates; start the server once to apply migrations before running a dry run",
        )?;
        let (planned, stats) = plan_candidates(&candidates);
        print_preview(&candidates, &planned);
        let summary = BackfillSummary {
            scanned: candidates.len(),
            changed: planned.len(),
            fields_changed: stats.fields_changed,
            updated: 0,
            raced: 0,
            last_id: stats.last_id,
            elapsed: started.elapsed(),
            dry_run: true,
        };
        print_summary(&summary);
        return Ok(summary);
    }

    let _lock = BackfillLock::acquire(
        &database,
        ".track-normalization-backfill.lock",
        "track normalization backfill",
    )?;
    let pool = lrclib_core::db::init_db(&database)
        .with_context(|| format!("failed to open database {}", database.display()))?;
    let mut conn = pool.get()?;

    let preview_candidates = track_repository::get_track_normalization_candidates(
        options.start_after_id,
        sample_size,
        &mut conn,
    )?;
    let (preview_planned, _) = plan_candidates(&preview_candidates);
    if preview_candidates.is_empty() {
        println!("Track normalization backfill: no candidate rows found");
    } else {
        print_preview(&preview_candidates, &preview_planned);
        confirm_write(&options)?;
    }

    let max_rows = options.limit.unwrap_or(usize::MAX);
    let mut scanned = 0usize;
    let mut changed = 0usize;
    let mut fields_changed = 0usize;
    let mut updated = 0usize;
    let mut raced = 0usize;
    let mut last_id = options.start_after_id;
    let mut next_progress = options.progress_every;
    let mut cursor = options.start_after_id;

    while scanned < max_rows {
        let batch_size = options.batch_size.min(max_rows - scanned);
        let candidates =
            track_repository::get_track_normalization_candidates(cursor, batch_size, &mut conn)?;
        if candidates.is_empty() {
            break;
        }
        let batch_last_id = candidates
            .last()
            .map(|candidate| candidate.id)
            .expect("non-empty candidate batch must have a last row");

        let (planned, stats) = plan_candidates(&candidates);
        let updates = planned
            .iter()
            .map(|planned| planned.update.clone())
            .collect::<Vec<_>>();
        let actual_updated =
            track_repository::apply_track_normalization_backfill_batch(&updates, &mut conn)?;

        scanned += candidates.len();
        changed += planned.len();
        fields_changed += stats.fields_changed;
        updated += actual_updated;
        raced += planned.len() - actual_updated;
        last_id = batch_last_id;
        cursor = batch_last_id;

        if scanned >= next_progress {
            println!(
                "Track normalization backfill progress: scanned={}, changed={}, updated={}",
                scanned, changed, updated
            );
            while next_progress <= scanned {
                next_progress = next_progress.saturating_add(options.progress_every);
                if next_progress == usize::MAX {
                    break;
                }
            }
        }

        if options.sleep_ms > 0 && scanned < max_rows {
            tokio::time::sleep(Duration::from_millis(options.sleep_ms)).await;
        }
    }

    let summary = BackfillSummary {
        scanned,
        changed,
        fields_changed,
        updated,
        raced,
        last_id,
        elapsed: started.elapsed(),
        dry_run: false,
    };
    print_summary(&summary);
    Ok(summary)
}

#[derive(Default)]
struct PlanStats {
    fields_changed: usize,
    last_id: i64,
}

fn plan_candidates(candidates: &[TrackNormalizationCandidate]) -> (Vec<PlannedUpdate>, PlanStats) {
    let mut stats = PlanStats {
        last_id: candidates.last().map_or(0, |candidate| candidate.id),
        ..PlanStats::default()
    };
    let mut planned = Vec::new();

    for candidate in candidates {
        let Some(planned_update) = plan_update(candidate) else {
            continue;
        };
        stats.fields_changed += planned_update.fields_changed;
        planned.push(planned_update);
    }

    (planned, stats)
}

fn plan_update(candidate: &TrackNormalizationCandidate) -> Option<PlannedUpdate> {
    let name_lower = normalized_or_current(&candidate.name, &candidate.name_lower);
    let artist_name_lower =
        normalized_or_current(&candidate.artist_name, &candidate.artist_name_lower);
    let album_name_lower =
        normalized_or_current(&candidate.album_name, &candidate.album_name_lower);

    let mut fields_changed = 0usize;
    if name_lower != candidate.name_lower {
        fields_changed += 1;
    }
    if artist_name_lower != candidate.artist_name_lower {
        fields_changed += 1;
    }
    if album_name_lower != candidate.album_name_lower {
        fields_changed += 1;
    }
    if fields_changed == 0 {
        return None;
    }

    Some(PlannedUpdate {
        update: TrackNormalizationUpdate {
            id: candidate.id,
            expected_name_lower: candidate.name_lower.clone(),
            expected_artist_name_lower: candidate.artist_name_lower.clone(),
            expected_album_name_lower: candidate.album_name_lower.clone(),
            name_lower,
            artist_name_lower,
            album_name_lower,
        },
        fields_changed,
    })
}

fn normalized_or_current(raw: &Option<String>, current: &Option<String>) -> Option<String> {
    let Some(raw) = raw.as_deref() else {
        return current.clone();
    };
    let normalized = prepare_input(raw);
    if normalized.is_empty() {
        current.clone()
    } else {
        Some(normalized)
    }
}

fn print_preview(candidates: &[TrackNormalizationCandidate], planned: &[PlannedUpdate]) {
    println!(
        "Track normalization backfill preview: candidates={}, changed={}",
        candidates.len(),
        planned.len()
    );
    let planned_by_id = planned
        .iter()
        .map(|planned| (planned.update.id, planned))
        .collect::<std::collections::HashMap<_, _>>();
    for candidate in candidates {
        let Some(planned) = planned_by_id.get(&candidate.id) else {
            continue;
        };
        let mut fields = Vec::new();
        if planned.update.name_lower != candidate.name_lower {
            fields.push(format!(
                "name_lower {:?} -> {:?}",
                candidate.name_lower, planned.update.name_lower
            ));
        }
        if planned.update.artist_name_lower != candidate.artist_name_lower {
            fields.push(format!(
                "artist_name_lower {:?} -> {:?}",
                candidate.artist_name_lower, planned.update.artist_name_lower
            ));
        }
        if planned.update.album_name_lower != candidate.album_name_lower {
            fields.push(format!(
                "album_name_lower {:?} -> {:?}",
                candidate.album_name_lower, planned.update.album_name_lower
            ));
        }
        println!("  id={}: {}", candidate.id, fields.join(", "));
    }
}

fn confirm_write(options: &BackfillOptions) -> Result<()> {
    if options.yes {
        return Ok(());
    }
    ensure!(
        io::stdin().is_terminal(),
        "refusing to write without confirmation because stdin is not a terminal; rerun with --yes"
    );
    print!("Apply the track normalization backfill to all matching rows? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!("aborted by user; no tracks changed")
    }
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
    ensure!(
        options.sample_size > 0,
        "sample size must be greater than zero"
    );
    if let Some(limit) = options.limit {
        ensure!(limit > 0, "row limit must be greater than zero");
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
        "Track normalization {mode} summary: scanned={}, changed={}, fields_changed={}, updated={}, raced={}, last_id={}, elapsed={:.2?}",
        summary.scanned,
        summary.changed,
        summary.fields_changed,
        summary.updated,
        summary.raced,
        summary.last_id,
        summary.elapsed,
    );
    if !summary.dry_run && summary.updated > 0 {
        println!(
            "Search index note: restart the server (index format v2 rebuilds on startup) before searching the updated tracks."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lrclib_core::repositories::track_repository;
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
            "lrclib-track-normalization-backfill-{}-{unique}.sqlite3",
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
            yes: true,
        }
    }

    fn cleanup(database: &Path) {
        for path in [
            database.to_path_buf(),
            PathBuf::from(format!("{}-wal", database.display())),
            PathBuf::from(format!("{}-shm", database.display())),
            PathBuf::from(format!(
                "{}.track-normalization-backfill.lock",
                database.display()
            )),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[tokio::test]
    async fn dry_run_and_backfill_fill_empty_normalized_metadata() {
        let database = test_database_path();
        let pool = lrclib_core::db::init_db(&database).unwrap();
        let (punctuation_name_id, punctuation_artist_id, punctuation_album_id, clean_id) = {
            let mut conn = pool.get().unwrap();
            let punctuation_name_id =
                track_repository::add_one("?", "Outkast", "Stankonia", 89.0, &mut conn).unwrap();
            conn.execute(
                "UPDATE tracks SET name_lower = '', updated_at = '2020-01-01' WHERE id = ?",
                [punctuation_name_id],
            )
            .unwrap();
            let punctuation_artist_id =
                track_repository::add_one("Artist Song", "!!!", "Album", 180.0, &mut conn).unwrap();
            conn.execute(
                "UPDATE tracks SET artist_name_lower = '' WHERE id = ?",
                [punctuation_artist_id],
            )
            .unwrap();
            let punctuation_album_id =
                track_repository::add_one("Album Song", "Artist", "???", 200.0, &mut conn).unwrap();
            conn.execute(
                "UPDATE tracks SET album_name_lower = '' WHERE id = ?",
                [punctuation_album_id],
            )
            .unwrap();
            let clean_id = track_repository::add_one(
                "Clean Song",
                "Clean Artist",
                "Clean Album",
                210.0,
                &mut conn,
            )
            .unwrap();
            (
                punctuation_name_id,
                punctuation_artist_id,
                punctuation_album_id,
                clean_id,
            )
        };
        drop(pool);

        let dry_run = run(options(&database, true, None)).await.unwrap();
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.scanned, 3);
        assert_eq!(dry_run.changed, 3);
        assert_eq!(dry_run.fields_changed, 3);
        assert_eq!(dry_run.updated, 0);
        assert!(!PathBuf::from(format!(
            "{}.track-normalization-backfill.lock",
            database.display()
        ))
        .exists());

        let completed = run(options(&database, false, None)).await.unwrap();
        assert_eq!(completed.scanned, 3);
        assert_eq!(completed.changed, 3);
        assert_eq!(completed.updated, 3);
        assert_eq!(completed.raced, 0);

        let pool = lrclib_core::db::init_db(&database).unwrap();
        let conn = pool.get().unwrap();
        for (id, name_lower, artist_name_lower, album_name_lower) in [
            (
                punctuation_name_id,
                Some("?".to_owned()),
                Some("outkast".to_owned()),
                Some("stankonia".to_owned()),
            ),
            (
                punctuation_artist_id,
                Some("artist song".to_owned()),
                Some("!!!".to_owned()),
                Some("album".to_owned()),
            ),
            (
                punctuation_album_id,
                Some("album song".to_owned()),
                Some("artist".to_owned()),
                Some("???".to_owned()),
            ),
        ] {
            let actual: (Option<String>, Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT name_lower, artist_name_lower, album_name_lower FROM tracks WHERE id = ?",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(actual, (name_lower, artist_name_lower, album_name_lower));
        }
        let clean: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT name_lower, artist_name_lower, album_name_lower FROM tracks WHERE id = ?",
                [clean_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            clean,
            (
                Some("clean song".to_owned()),
                Some("clean artist".to_owned()),
                Some("clean album".to_owned())
            )
        );
        let updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM tracks WHERE id = ?",
                [punctuation_name_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, "2020-01-01");
        drop(conn);
        drop(pool);

        let resumed = run(options(&database, false, None)).await.unwrap();
        assert_eq!(resumed.scanned, 0);
        assert_eq!(resumed.updated, 0);

        cleanup(&database);
    }
}
