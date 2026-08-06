use hdrhistogram::Histogram;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};
use std::time::Duration;

const MAX_RESPONSE_LATENCY_US: u64 = 60 * 1_000_000;

#[derive(Clone, Copy)]
pub(crate) enum MonitoredRoute {
    Search,
    GetMetadata,
}

pub(crate) struct HttpMetrics {
    requests_total: AtomicU64,
    responses_5xx: AtomicU64,
    response_latency_us: Mutex<Histogram<u64>>,
    search: EndpointMetrics,
    get_metadata: EndpointMetrics,
}

struct EndpointMetrics {
    requests: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    failures: AtomicU64,
    overloads: AtomicU64,
    route_latency_us: Mutex<Histogram<u64>>,
    blocking_wait_latency_us: Mutex<Histogram<u64>>,
    tantivy_latency_us: Mutex<Histogram<u64>>,
    db_pool_wait_latency_us: Mutex<Histogram<u64>>,
    sql_latency_us: Mutex<Histogram<u64>>,
}

pub(crate) struct EndpointMetricsSnapshot {
    pub requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub failures: u64,
    pub overloads: u64,
    pub route_latency_p50_ms: f64,
    pub route_latency_p95_ms: f64,
    pub route_latency_p99_ms: f64,
    pub blocking_wait_latency_p50_ms: f64,
    pub blocking_wait_latency_p95_ms: f64,
    pub blocking_wait_latency_p99_ms: f64,
    pub tantivy_latency_p50_ms: f64,
    pub tantivy_latency_p95_ms: f64,
    pub tantivy_latency_p99_ms: f64,
    pub db_pool_wait_latency_p50_ms: f64,
    pub db_pool_wait_latency_p95_ms: f64,
    pub db_pool_wait_latency_p99_ms: f64,
    pub sql_latency_p50_ms: f64,
    pub sql_latency_p95_ms: f64,
    pub sql_latency_p99_ms: f64,
}

pub(crate) struct HttpMetricsSnapshot {
    pub requests_total: u64,
    pub responses_5xx: u64,
    pub response_latency_p50_ms: f64,
    pub response_latency_p95_ms: f64,
    pub response_latency_p99_ms: f64,
    pub search: EndpointMetricsSnapshot,
    pub get_metadata: EndpointMetricsSnapshot,
}

impl HttpMetrics {
    pub(crate) fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            responses_5xx: AtomicU64::new(0),
            response_latency_us: Mutex::new(
                Histogram::new_with_bounds(1, MAX_RESPONSE_LATENCY_US, 3)
                    .expect("valid response latency histogram bounds"),
            ),
            search: EndpointMetrics::new(),
            get_metadata: EndpointMetrics::new(),
        }
    }

    pub(crate) fn record_request(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_response(&self, is_server_error: bool, latency: Duration) {
        if is_server_error {
            self.responses_5xx.fetch_add(1, Ordering::Relaxed);
        }

        let latency_us = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
        self.response_latency_us
            .lock()
            .expect("response latency histogram lock poisoned")
            .saturating_record(latency_us);
    }

    pub(crate) fn record_endpoint_request(&self, route: MonitoredRoute) {
        self.endpoint(route)
            .requests
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_endpoint_response(
        &self,
        route: MonitoredRoute,
        is_server_error: bool,
        latency: Duration,
    ) {
        let endpoint = self.endpoint(route);
        if is_server_error {
            endpoint.failures.fetch_add(1, Ordering::Relaxed);
        }
        record_latency(&endpoint.route_latency_us, latency);
    }

    pub(crate) fn record_cache_hit(&self, route: MonitoredRoute) {
        self.endpoint(route)
            .cache_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_cache_miss(&self, route: MonitoredRoute) {
        self.endpoint(route)
            .cache_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_tantivy_latency(&self, latency: Duration) {
        record_latency(&self.search.tantivy_latency_us, latency);
    }

    pub(crate) fn record_blocking_wait_latency(&self, route: MonitoredRoute, latency: Duration) {
        record_latency(&self.endpoint(route).blocking_wait_latency_us, latency);
    }

    pub(crate) fn record_db_pool_wait_latency(&self, route: MonitoredRoute, latency: Duration) {
        record_latency(&self.endpoint(route).db_pool_wait_latency_us, latency);
    }

    pub(crate) fn record_sql_latency(&self, route: MonitoredRoute, latency: Duration) {
        record_latency(&self.endpoint(route).sql_latency_us, latency);
    }

    pub(crate) fn record_overload(&self, route: MonitoredRoute) {
        self.endpoint(route)
            .overloads
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn take_snapshot(&self) -> HttpMetricsSnapshot {
        let mut latency = self
            .response_latency_us
            .lock()
            .expect("response latency histogram lock poisoned");
        let to_ms = |quantile| {
            if latency.is_empty() {
                0.0
            } else {
                latency.value_at_quantile(quantile) as f64 / 1_000.0
            }
        };
        let snapshot = HttpMetricsSnapshot {
            requests_total: self.requests_total.swap(0, Ordering::Relaxed),
            responses_5xx: self.responses_5xx.swap(0, Ordering::Relaxed),
            response_latency_p50_ms: to_ms(0.50),
            response_latency_p95_ms: to_ms(0.95),
            response_latency_p99_ms: to_ms(0.99),
            search: self.search.take_snapshot(),
            get_metadata: self.get_metadata.take_snapshot(),
        };
        latency.reset();
        snapshot
    }

    fn endpoint(&self, route: MonitoredRoute) -> &EndpointMetrics {
        match route {
            MonitoredRoute::Search => &self.search,
            MonitoredRoute::GetMetadata => &self.get_metadata,
        }
    }
}

impl EndpointMetrics {
    fn new() -> Self {
        Self {
            requests: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            overloads: AtomicU64::new(0),
            route_latency_us: new_latency_histogram(),
            blocking_wait_latency_us: new_latency_histogram(),
            tantivy_latency_us: new_latency_histogram(),
            db_pool_wait_latency_us: new_latency_histogram(),
            sql_latency_us: new_latency_histogram(),
        }
    }

    fn take_snapshot(&self) -> EndpointMetricsSnapshot {
        let (route_latency_p50_ms, route_latency_p95_ms, route_latency_p99_ms) =
            take_latency_snapshot(&self.route_latency_us);
        let (
            blocking_wait_latency_p50_ms,
            blocking_wait_latency_p95_ms,
            blocking_wait_latency_p99_ms,
        ) = take_latency_snapshot(&self.blocking_wait_latency_us);
        let (tantivy_latency_p50_ms, tantivy_latency_p95_ms, tantivy_latency_p99_ms) =
            take_latency_snapshot(&self.tantivy_latency_us);
        let (db_pool_wait_latency_p50_ms, db_pool_wait_latency_p95_ms, db_pool_wait_latency_p99_ms) =
            take_latency_snapshot(&self.db_pool_wait_latency_us);
        let (sql_latency_p50_ms, sql_latency_p95_ms, sql_latency_p99_ms) =
            take_latency_snapshot(&self.sql_latency_us);

        EndpointMetricsSnapshot {
            requests: self.requests.swap(0, Ordering::Relaxed),
            cache_hits: self.cache_hits.swap(0, Ordering::Relaxed),
            cache_misses: self.cache_misses.swap(0, Ordering::Relaxed),
            failures: self.failures.swap(0, Ordering::Relaxed),
            overloads: self.overloads.swap(0, Ordering::Relaxed),
            route_latency_p50_ms,
            route_latency_p95_ms,
            route_latency_p99_ms,
            blocking_wait_latency_p50_ms,
            blocking_wait_latency_p95_ms,
            blocking_wait_latency_p99_ms,
            tantivy_latency_p50_ms,
            tantivy_latency_p95_ms,
            tantivy_latency_p99_ms,
            db_pool_wait_latency_p50_ms,
            db_pool_wait_latency_p95_ms,
            db_pool_wait_latency_p99_ms,
            sql_latency_p50_ms,
            sql_latency_p95_ms,
            sql_latency_p99_ms,
        }
    }
}

fn new_latency_histogram() -> Mutex<Histogram<u64>> {
    Mutex::new(
        Histogram::new_with_bounds(1, MAX_RESPONSE_LATENCY_US, 3)
            .expect("valid latency histogram bounds"),
    )
}

fn record_latency(histogram: &Mutex<Histogram<u64>>, latency: Duration) {
    let latency_us = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
    histogram
        .lock()
        .expect("latency histogram lock poisoned")
        .saturating_record(latency_us);
}

fn take_latency_snapshot(histogram: &Mutex<Histogram<u64>>) -> (f64, f64, f64) {
    let mut histogram = histogram.lock().expect("latency histogram lock poisoned");
    let to_ms = |quantile| {
        if histogram.is_empty() {
            0.0
        } else {
            histogram.value_at_quantile(quantile) as f64 / 1_000.0
        }
    };
    let snapshot = (to_ms(0.50), to_ms(0.95), to_ms(0.99));
    histogram.reset();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reports_and_resets_interval_metrics() {
        let metrics = HttpMetrics::new();
        for latency_ms in 1..=100 {
            metrics.record_request();
            metrics.record_response(latency_ms == 100, Duration::from_millis(latency_ms));
        }

        let snapshot = metrics.take_snapshot();
        assert_eq!(snapshot.requests_total, 100);
        assert_eq!(snapshot.responses_5xx, 1);
        assert!((49.0..=51.0).contains(&snapshot.response_latency_p50_ms));
        assert!((94.0..=96.0).contains(&snapshot.response_latency_p95_ms));
        assert!((98.0..=100.0).contains(&snapshot.response_latency_p99_ms));

        let empty = metrics.take_snapshot();
        assert_eq!(empty.requests_total, 0);
        assert_eq!(empty.responses_5xx, 0);
        assert_eq!(empty.response_latency_p50_ms, 0.0);
        assert_eq!(empty.response_latency_p95_ms, 0.0);
        assert_eq!(empty.response_latency_p99_ms, 0.0);
    }

    #[test]
    fn snapshot_reports_and_resets_endpoint_metrics() {
        let metrics = HttpMetrics::new();
        metrics.record_endpoint_request(MonitoredRoute::Search);
        metrics.record_cache_miss(MonitoredRoute::Search);
        metrics.record_endpoint_response(MonitoredRoute::Search, true, Duration::from_millis(30));
        metrics.record_blocking_wait_latency(MonitoredRoute::Search, Duration::from_millis(8));
        metrics.record_tantivy_latency(Duration::from_millis(10));
        metrics.record_db_pool_wait_latency(MonitoredRoute::Search, Duration::from_millis(12));
        metrics.record_sql_latency(MonitoredRoute::Search, Duration::from_millis(20));

        metrics.record_endpoint_request(MonitoredRoute::GetMetadata);
        metrics.record_cache_hit(MonitoredRoute::GetMetadata);
        metrics.record_endpoint_response(
            MonitoredRoute::GetMetadata,
            false,
            Duration::from_millis(5),
        );
        metrics.record_sql_latency(MonitoredRoute::GetMetadata, Duration::from_millis(2));

        let snapshot = metrics.take_snapshot();
        assert_eq!(snapshot.search.requests, 1);
        assert_eq!(snapshot.search.cache_misses, 1);
        assert_eq!(snapshot.search.failures, 1);
        assert!((29.0..=31.0).contains(&snapshot.search.route_latency_p99_ms));
        assert!((7.0..=9.0).contains(&snapshot.search.blocking_wait_latency_p99_ms));
        assert!((9.0..=11.0).contains(&snapshot.search.tantivy_latency_p99_ms));
        assert!((11.0..=13.0).contains(&snapshot.search.db_pool_wait_latency_p99_ms));
        assert!((19.0..=21.0).contains(&snapshot.search.sql_latency_p99_ms));

        assert_eq!(snapshot.get_metadata.requests, 1);
        assert_eq!(snapshot.get_metadata.cache_hits, 1);
        assert_eq!(snapshot.get_metadata.failures, 0);
        assert!((4.0..=6.0).contains(&snapshot.get_metadata.route_latency_p99_ms));
        assert!((1.0..=3.0).contains(&snapshot.get_metadata.sql_latency_p99_ms));

        let empty = metrics.take_snapshot();
        assert_eq!(empty.search.requests, 0);
        assert_eq!(empty.search.cache_misses, 0);
        assert_eq!(empty.search.route_latency_p99_ms, 0.0);
        assert_eq!(empty.search.blocking_wait_latency_p99_ms, 0.0);
        assert_eq!(empty.search.db_pool_wait_latency_p99_ms, 0.0);
        assert_eq!(empty.get_metadata.requests, 0);
        assert_eq!(empty.get_metadata.cache_hits, 0);
        assert_eq!(empty.get_metadata.sql_latency_p99_ms, 0.0);
    }
}
