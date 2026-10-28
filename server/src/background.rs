use crate::{repositories::lyrics_repository::get_last_10_mins_lyrics_count, AppState};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

/// Spawns a background task that reports request metrics every minute.
pub(crate) fn spawn_metrics_reporter(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let metrics = state.http_metrics.take_snapshot();
            tracing::info!(
                event = "http_metrics",
                interval_seconds = 60,
                requests_total = metrics.requests_total,
                responses_5xx = metrics.responses_5xx,
                response_latency_p50_ms = metrics.response_latency_p50_ms,
                response_latency_p95_ms = metrics.response_latency_p95_ms,
                response_latency_p99_ms = metrics.response_latency_p99_ms,
                "HTTP metrics for the last minute"
            );
            tracing::info!(
                event = "search_metrics",
                interval_seconds = 60,
                requests = metrics.search.requests,
                cache_hits = metrics.search.cache_hits,
                cache_misses = metrics.search.cache_misses,
                failures = metrics.search.failures,
                overloads = metrics.search.overloads,
                route_latency_p50_ms = metrics.search.route_latency_p50_ms,
                route_latency_p95_ms = metrics.search.route_latency_p95_ms,
                route_latency_p99_ms = metrics.search.route_latency_p99_ms,
                blocking_wait_latency_p50_ms = metrics.search.blocking_wait_latency_p50_ms,
                blocking_wait_latency_p95_ms = metrics.search.blocking_wait_latency_p95_ms,
                blocking_wait_latency_p99_ms = metrics.search.blocking_wait_latency_p99_ms,
                tantivy_latency_p50_ms = metrics.search.tantivy_latency_p50_ms,
                tantivy_latency_p95_ms = metrics.search.tantivy_latency_p95_ms,
                tantivy_latency_p99_ms = metrics.search.tantivy_latency_p99_ms,
                db_pool_wait_latency_p50_ms = metrics.search.db_pool_wait_latency_p50_ms,
                db_pool_wait_latency_p95_ms = metrics.search.db_pool_wait_latency_p95_ms,
                db_pool_wait_latency_p99_ms = metrics.search.db_pool_wait_latency_p99_ms,
                sql_latency_p50_ms = metrics.search.sql_latency_p50_ms,
                sql_latency_p95_ms = metrics.search.sql_latency_p95_ms,
                sql_latency_p99_ms = metrics.search.sql_latency_p99_ms,
                "Search metrics for the last minute"
            );
            tracing::info!(
                event = "get_metadata_metrics",
                interval_seconds = 60,
                requests = metrics.get_metadata.requests,
                cache_hits = metrics.get_metadata.cache_hits,
                cache_misses = metrics.get_metadata.cache_misses,
                failures = metrics.get_metadata.failures,
                overloads = metrics.get_metadata.overloads,
                route_latency_p50_ms = metrics.get_metadata.route_latency_p50_ms,
                route_latency_p95_ms = metrics.get_metadata.route_latency_p95_ms,
                route_latency_p99_ms = metrics.get_metadata.route_latency_p99_ms,
                blocking_wait_latency_p50_ms = metrics.get_metadata.blocking_wait_latency_p50_ms,
                blocking_wait_latency_p95_ms = metrics.get_metadata.blocking_wait_latency_p95_ms,
                blocking_wait_latency_p99_ms = metrics.get_metadata.blocking_wait_latency_p99_ms,
                db_pool_wait_latency_p50_ms = metrics.get_metadata.db_pool_wait_latency_p50_ms,
                db_pool_wait_latency_p95_ms = metrics.get_metadata.db_pool_wait_latency_p95_ms,
                db_pool_wait_latency_p99_ms = metrics.get_metadata.db_pool_wait_latency_p99_ms,
                sql_latency_p50_ms = metrics.get_metadata.sql_latency_p50_ms,
                sql_latency_p95_ms = metrics.get_metadata.sql_latency_p95_ms,
                sql_latency_p99_ms = metrics.get_metadata.sql_latency_p99_ms,
                "Get-by-metadata metrics for the last minute"
            );
        }
    });
}

/// Spawns a background task that updates the recent lyrics count every minute.
pub(crate) fn spawn_lyrics_counter(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let pool = state.pool.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
                let mut conn = pool.get()?;
                get_last_10_mins_lyrics_count(&mut conn)
            })
            .await;

            match result {
                Ok(Ok(count)) => {
                    state
                        .recent_lyrics_count
                        .store(count as usize, Ordering::Relaxed);
                }
                Ok(Err(e)) => tracing::warn!(error = %e, "Failed to query lyrics count"),
                Err(e) => tracing::warn!(error = %e, "Lyrics counter task failed"),
            }
        }
    });
}

const OPTIMIZE_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);
const OPTIMIZE_ANALYSIS_LIMIT: i64 = 400;

/// Spawns a background task that runs PRAGMA optimize once a day.
pub(crate) fn spawn_optimize_pragma(state: Arc<AppState>) {
    tokio::spawn(async move {
        // `init_db` already runs `PRAGMA optimize=0x10002` before the server
        // starts listening, so the first periodic pass waits a full day.
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + OPTIMIZE_INTERVAL,
            OPTIMIZE_INTERVAL,
        );
        loop {
            interval.tick().await;
            let pool = state.pool.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Duration> {
                let started = std::time::Instant::now();
                let conn = pool.get()?;
                optimize_pragma(&conn)?;
                Ok(started.elapsed())
            })
            .await;

            match result {
                Ok(Ok(elapsed)) => tracing::info!(
                    event = "optimize_pragma",
                    elapsed_ms = elapsed.as_millis() as u64,
                    "PRAGMA optimize completed"
                ),
                Ok(Err(e)) => tracing::warn!(error = %e, "PRAGMA optimize failed"),
                Err(e) => tracing::warn!(error = %e, "PRAGMA optimize task failed"),
            }
        }
    });
}

fn optimize_pragma(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    // Bound the work per index so the daily pass stays short on large databases.
    conn.pragma_update(None, "analysis_limit", OPTIMIZE_ANALYSIS_LIMIT)?;
    conn.execute_batch("PRAGMA optimize;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn optimize_pragma_bounds_analysis_work() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE optimize_test (id INTEGER PRIMARY KEY, value TEXT);
             CREATE INDEX optimize_test_value ON optimize_test(value);
             INSERT INTO optimize_test (value) VALUES ('a'), ('b'), ('c');",
        )
        .unwrap();
        // SQLite 3.45 only re-analyzes tables whose statistics were used by
        // the query planner on this connection.
        conn.query_row(
            "SELECT id FROM optimize_test INDEXED BY optimize_test_value WHERE value = 'a'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

        optimize_pragma(&conn).unwrap();

        let analysis_limit: i64 = conn
            .query_row("PRAGMA analysis_limit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(analysis_limit, OPTIMIZE_ANALYSIS_LIMIT);

        let statistics: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |row| row.get(0))
            .unwrap();
        assert!(statistics > 0);
    }
}
