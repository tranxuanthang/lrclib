use crate::{
    errors::OverloadedError,
    metrics::HttpMetrics,
    routes::{
        get_lyrics_by_metadata::TrackResponse as GetLyricsByMetadataResponse,
        search_lyrics::CachedResult,
    },
    search_index::SearchIndex,
};
use ahash::RandomState as AHashRandomState;

use moka::future::Cache as MokaCache;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use std::{
    sync::{
        atomic::AtomicUsize,
        Arc,
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

type Cache<K, V> = MokaCache<K, V, AHashRandomState>;

pub struct AppState {
    pub(crate) pool: Pool<SqliteConnectionManager>,
    pub(crate) search_index: Option<Arc<SearchIndex>>,
    pub(crate) challenge_cache: Cache<String, String>,
    pub(crate) get_metadata_cache: Cache<String, Option<GetLyricsByMetadataResponse>>,
    pub(crate) search_cache: Cache<String, CachedResult>,

    #[cfg(test)]
    pub(crate) search_load_count: AtomicUsize,
    pub(crate) http_metrics: HttpMetrics,
    pub(crate) recent_lyrics_count: AtomicUsize,
    pub(crate) load_permits: Arc<Semaphore>,
    pub(crate) load_queue_timeout: Duration,
}

impl AppState {
    pub fn db_connection(&self) -> Result<PooledConnection<SqliteConnectionManager>, r2d2::Error> {
        self.pool.get()
    }

    /// The embedded Tantivy search index, when the Tantivy backend is enabled.
    pub fn search_index(&self) -> Option<&Arc<SearchIndex>> {
        self.search_index.as_ref()
    }

    pub(crate) async fn acquire_load_permit(&self) -> Result<OwnedSemaphorePermit, OverloadedError> {
        match tokio::time::timeout(
            self.load_queue_timeout,
            self.load_permits.clone().acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(OverloadedError),
        }
    }
}

/// Build the application state with all caches and shared resources.
pub(crate) fn build_app_state(
    pool: Pool<SqliteConnectionManager>,
    search_index: Option<Arc<SearchIndex>>,
) -> Arc<AppState> {
    let max_concurrent_loads = std::env::var("LRCLIB_MAX_CONCURRENT_LOADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(32);
    let load_queue_timeout_ms = std::env::var("LRCLIB_LOAD_QUEUE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(250);

    tracing::info!(
        max_concurrent_loads,
        load_queue_timeout_ms,
        "load admission control configured"
    );

    Arc::new(AppState {
        pool,
        search_index,
        challenge_cache: MokaCache::builder()
            .time_to_live(Duration::from_secs(60 * 5))
            .max_capacity(200000)
            .build_with_hasher(AHashRandomState::new()),
        get_metadata_cache: MokaCache::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 24))
            .time_to_idle(Duration::from_secs(60 * 60 * 4))
            .max_capacity(3000000)
            .build_with_hasher(AHashRandomState::new()),
        search_cache: MokaCache::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 24))
            .time_to_idle(Duration::from_secs(60 * 60 * 4))
            .max_capacity(1000000)
            .build_with_hasher(AHashRandomState::new()),

        #[cfg(test)]
        search_load_count: AtomicUsize::new(0),
        http_metrics: HttpMetrics::new(),
        recent_lyrics_count: AtomicUsize::new(0),
        load_permits: Arc::new(Semaphore::new(max_concurrent_loads)),
        load_queue_timeout: Duration::from_millis(load_queue_timeout_ms),
    })
}
