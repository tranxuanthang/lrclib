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
            let mut conn = match state.pool.get() {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to get DB connection for lyrics counter");
                    continue;
                }
            };
            let count = match get_last_10_mins_lyrics_count(&mut conn) {
                Ok(count) => count,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to query lyrics count");
                    continue;
                }
            };
            state
                .recent_lyrics_count
                .store(count as usize, Ordering::Relaxed);
        }
    });
}

/// Spawns a background task that runs PRAGMA optimize once a day.
pub(crate) fn spawn_optimize_pragma(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60 * 24));
        loop {
            interval.tick().await;
            if let Ok(conn) = state.pool.get() {
                if let Err(e) = conn.execute_batch("PRAGMA optimize;") {
                    tracing::warn!(error = %e, "PRAGMA optimize failed");
                }
            }
        }
    });
}
