use super::{build_document, SearchIndex};
use crate::repositories::track_repository::get_track_search_docs_after;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tantivy::{IndexWriter, TantivyDocument};

const INDEXER_THREAD_NAME: &str = "lrclib-search-indexer";
const ERROR_RETRY_DELAY: Duration = Duration::from_secs(60);

/// The indexer tails the `tracks` table instead of hooking into the publish
/// path: `tracks.id` is monotonic (AUTOINCREMENT) and the indexed columns are
/// immutable after insert, so "index everything with `id > cursor`" read from
/// committed SQLite state is race-free by construction. The cursor is stored
/// as the Tantivy commit payload, making it atomic with the commit.
#[derive(Clone)]
pub struct IndexerConfig {
    /// How long to sleep between polls once the index has caught up.
    pub poll_interval: Duration,
    /// Maximum rows fetched from SQLite per batch.
    pub batch_size: usize,
    /// Commit at least every this many indexed rows during catch-up.
    pub rows_per_commit: usize,
    /// Memory budget handed to the Tantivy writer.
    pub writer_memory_bytes: usize,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(300),
            batch_size: 10_000,
            rows_per_commit: 500_000,
            writer_memory_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Owns the indexer thread; dropping it requests shutdown and joins.
pub struct IndexerHandle {
    shutdown_tx: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl IndexerHandle {
    /// Requests shutdown, waits for a final commit, and joins the thread.
    pub fn shutdown(self) {
        drop(self);
    }

    fn request_shutdown_and_join(&mut self) {
        let _ = self.shutdown_tx.send(());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::error!("search indexer thread panicked");
            }
        }
    }
}

impl Drop for IndexerHandle {
    fn drop(&mut self) {
        self.request_shutdown_and_join();
    }
}

pub fn spawn_indexer(
    search_index: Arc<SearchIndex>,
    pool: Pool<SqliteConnectionManager>,
    config: IndexerConfig,
) -> IndexerHandle {
    let (shutdown_tx, shutdown_rx) = channel();
    let thread = std::thread::Builder::new()
        .name(INDEXER_THREAD_NAME.to_owned())
        .spawn(move || run(search_index, pool, config, shutdown_rx))
        .expect("failed to spawn search indexer thread");

    IndexerHandle {
        shutdown_tx,
        thread: Some(thread),
    }
}

#[derive(PartialEq)]
enum Flow {
    Continue,
    Shutdown,
}

enum BatchOutcome {
    /// A batch of rows was indexed; more rows may be waiting.
    Progressed,
    /// The index has caught up with the `tracks` table.
    Drained,
}

fn run(
    search_index: Arc<SearchIndex>,
    pool: Pool<SqliteConnectionManager>,
    config: IndexerConfig,
    shutdown_rx: Receiver<()>,
) {
    let mut writer = match search_index
        .index()
        .writer::<TantivyDocument>(config.writer_memory_bytes)
    {
        Ok(writer) => writer,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to create search index writer; search indexing disabled"
            );
            return;
        }
    };

    let mut cursor = match search_index.last_indexed_track_id() {
        Ok(cursor) => cursor,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to read search index cursor; search indexing disabled"
            );
            return;
        }
    };

    tracing::info!(last_indexed_track_id = cursor, "search indexer started");
    let mut pending_rows = 0usize;

    loop {
        let outcome = index_next_batch(
            &search_index,
            &pool,
            &config,
            &mut writer,
            &mut cursor,
            &mut pending_rows,
        );

        let flow = match outcome {
            Ok(BatchOutcome::Progressed) => poll_shutdown(&shutdown_rx),
            Ok(BatchOutcome::Drained) => {
                if !search_index.is_ready() {
                    search_index.mark_ready();
                    tracing::info!(
                        last_indexed_track_id = cursor,
                        "search index caught up with the database"
                    );
                }
                wait_shutdown(&shutdown_rx, config.poll_interval)
            }
            Err(error) => {
                tracing::error!(error = %error, "search indexing failed; will retry");
                wait_shutdown(&shutdown_rx, ERROR_RETRY_DELAY)
            }
        };

        if flow == Flow::Shutdown {
            break;
        }
    }

    if pending_rows > 0 {
        if let Err(error) = commit_with_cursor(&mut writer, cursor) {
            tracing::error!(
                error = %error,
                "failed to commit search index during shutdown"
            );
        }
    }
    tracing::info!(last_indexed_track_id = cursor, "search indexer stopped");
}

fn index_next_batch(
    search_index: &SearchIndex,
    pool: &Pool<SqliteConnectionManager>,
    config: &IndexerConfig,
    writer: &mut IndexWriter,
    cursor: &mut i64,
    pending_rows: &mut usize,
) -> anyhow::Result<BatchOutcome> {
    let batch = {
        let mut conn = pool.get()?;
        get_track_search_docs_after(*cursor, config.batch_size, &mut conn)?
    };

    if batch.is_empty() {
        if *pending_rows > 0 {
            commit_with_cursor(writer, *cursor)?;
            tracing::debug!(last_indexed_track_id = *cursor, "search index commit");
            *pending_rows = 0;
        }
        return Ok(BatchOutcome::Drained);
    }

    for track in &batch {
        writer.add_document(build_document(search_index.fields(), track))?;
        *cursor = track.id;
    }
    *pending_rows += batch.len();

    if *pending_rows >= config.rows_per_commit {
        commit_with_cursor(writer, *cursor)?;
        tracing::info!(
            last_indexed_track_id = *cursor,
            "search index catch-up commit"
        );
        *pending_rows = 0;
    }

    Ok(BatchOutcome::Progressed)
}

fn commit_with_cursor(writer: &mut IndexWriter, cursor: i64) -> tantivy::Result<()> {
    let mut prepared = writer.prepare_commit()?;
    prepared.set_payload(&cursor.to_string());
    prepared.commit()?;
    Ok(())
}

fn poll_shutdown(shutdown_rx: &Receiver<()>) -> Flow {
    match shutdown_rx.try_recv() {
        Ok(()) | Err(TryRecvError::Disconnected) => Flow::Shutdown,
        Err(TryRecvError::Empty) => Flow::Continue,
    }
}

fn wait_shutdown(shutdown_rx: &Receiver<()>, timeout: Duration) -> Flow {
    match shutdown_rx.recv_timeout(timeout) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => Flow::Shutdown,
        Err(RecvTimeoutError::Timeout) => Flow::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::repositories::track_repository;
    use std::path::PathBuf;
    use std::time::Instant;
    use uuid::Uuid;

    fn test_paths() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("lrclib-indexer-{}", Uuid::new_v4()));
        (
            PathBuf::from(format!("{}.sqlite3", base.display())),
            PathBuf::from(format!("{}.tantivy", base.display())),
        )
    }

    fn test_config() -> IndexerConfig {
        IndexerConfig {
            poll_interval: Duration::from_millis(25),
            batch_size: 2,
            rows_per_commit: 1000,
            writer_memory_bytes: 32 * 1024 * 1024,
        }
    }

    fn wait_until(description: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {}", description);
    }

    fn search_ids(search_index: &SearchIndex, q: &str) -> Vec<i64> {
        search_index
            .reload_reader()
            .expect("failed to reload reader");
        search_index
            .search(Some(q), None, None, None)
            .expect("search failed")
    }

    fn cleanup(db_path: &std::path::Path, index_dir: &std::path::Path) {
        for path in [
            db_path.to_path_buf(),
            PathBuf::from(format!("{}-wal", db_path.display())),
            PathBuf::from(format!("{}-shm", db_path.display())),
        ] {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(index_dir);
    }

    #[test]
    fn indexes_existing_tracks_and_marks_ready() {
        let (db_path, index_dir) = test_paths();
        let pool = init_db(&db_path).unwrap();
        {
            let mut conn = pool.get().unwrap();
            track_repository::add_one("First Song", "Artist A", "Album", 180.0, &mut conn).unwrap();
            track_repository::add_one("Second Song", "Artist B", "Album", 200.0, &mut conn)
                .unwrap();
            track_repository::add_one("Third Song", "Artist C", "Album", 210.0, &mut conn).unwrap();
        }

        let search_index = Arc::new(SearchIndex::open_or_create(&index_dir).unwrap());
        let indexer = spawn_indexer(Arc::clone(&search_index), pool, test_config());

        wait_until("indexer to catch up", || search_index.is_ready());
        wait_until("tracks to become searchable", || {
            search_ids(&search_index, "second song") == vec![2]
        });
        assert_eq!(search_index.num_docs(), 3);

        indexer.shutdown();
        cleanup(&db_path, &index_dir);
    }

    #[test]
    fn indexes_tracks_published_after_startup() {
        let (db_path, index_dir) = test_paths();
        let pool = init_db(&db_path).unwrap();

        let search_index = Arc::new(SearchIndex::open_or_create(&index_dir).unwrap());
        let indexer = spawn_indexer(Arc::clone(&search_index), pool.clone(), test_config());
        wait_until("indexer to catch up", || search_index.is_ready());

        {
            let mut conn = pool.get().unwrap();
            track_repository::add_one("Late Arrival", "Artist", "Album", 180.0, &mut conn).unwrap();
        }

        wait_until("new track to become searchable", || {
            search_ids(&search_index, "late arrival") == vec![1]
        });

        indexer.shutdown();
        cleanup(&db_path, &index_dir);
    }

    #[test]
    fn resumes_from_cursor_after_restart() {
        let (db_path, index_dir) = test_paths();
        let pool = init_db(&db_path).unwrap();
        {
            let mut conn = pool.get().unwrap();
            track_repository::add_one("First Song", "Artist A", "Album", 180.0, &mut conn).unwrap();
            track_repository::add_one("Second Song", "Artist B", "Album", 200.0, &mut conn)
                .unwrap();
        }

        {
            let search_index = Arc::new(SearchIndex::open_or_create(&index_dir).unwrap());
            let indexer = spawn_indexer(Arc::clone(&search_index), pool.clone(), test_config());
            wait_until("first indexer to catch up", || search_index.is_ready());
            wait_until("initial tracks to become searchable", || {
                search_ids(&search_index, "first song") == vec![1]
            });
            indexer.shutdown();
            assert_eq!(search_index.last_indexed_track_id().unwrap(), 2);
        }

        {
            let mut conn = pool.get().unwrap();
            track_repository::add_one("Third Song", "Artist C", "Album", 210.0, &mut conn).unwrap();
        }

        let search_index = Arc::new(SearchIndex::open_or_create(&index_dir).unwrap());
        let indexer = spawn_indexer(Arc::clone(&search_index), pool, test_config());

        wait_until("second indexer to catch up", || search_index.is_ready());
        wait_until("delta track to become searchable", || {
            search_ids(&search_index, "third song") == vec![3]
        });

        // Rows indexed before the restart are not re-indexed.
        assert_eq!(search_index.num_docs(), 3);
        assert_eq!(search_ids(&search_index, "first song"), vec![1]);

        indexer.shutdown();
        assert_eq!(search_index.last_indexed_track_id().unwrap(), 3);
        cleanup(&db_path, &index_dir);
    }
}
