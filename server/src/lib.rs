use axum::{
    body::Body,
    http::{header, Request},
    response::Response,
    routing::{get, post},
    Router,
};
use crossbeam_queue::ArrayQueue;
use dashmap::DashMap;
use db::init_db;
use entities::missing_track::MissingTrack;
use mcp::LrclibMcpServer;
use moka::future::Cache;
use r2d2::Pool;
use r2d2::PooledConnection;
use r2d2_sqlite::SqliteConnectionManager;
use repositories::lyrics_repository::get_last_10_mins_lyrics_count;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use routes::get_lyrics_by_metadata::TrackResponse as GetLyricsByMetadataResponse;
use routes::search_lyrics::CachedResult;
use routes::{
    flag_lyrics, get_lyrics_by_metadata, get_lyrics_by_track_id, manage, publish_lyrics,
    request_challenge, search_lyrics,
};
use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{path::PathBuf, time::Duration};
use tokio::sync::Mutex;
use tokio::{signal, sync::watch};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{self, TraceLayer},
};
use tracing::Span;
use tracing_subscriber::EnvFilter;

pub mod db;
pub mod entities;
pub mod errors;
pub mod mcp;
#[path = "queue_stub.rs"]
pub mod queue;
pub mod repositories;
pub mod routes;
pub mod utils;

pub struct AppState {
    pool: Pool<SqliteConnectionManager>,
    challenge_cache: Cache<String, String>,
    get_cache: Cache<String, String>,
    get_metadata_cache: Cache<String, GetLyricsByMetadataResponse>,
    get_metadata_index: DashMap<i64, HashSet<String>>,
    search_cache: Cache<String, CachedResult>,
    queue: ArrayQueue<MissingTrack>,
    request_counter: AtomicUsize,
    recent_lyrics_count: AtomicUsize,
    workers_count: AtomicU8,
    workers_tx: Mutex<Option<watch::Sender<u8>>>,
}

impl AppState {
    pub fn db_connection(&self) -> Result<PooledConnection<SqliteConnectionManager>, r2d2::Error> {
        self.pool.get()
    }

    pub fn enqueue_missing_track(&self, missing_track: MissingTrack) -> Result<(), MissingTrack> {
        self.queue.push(missing_track)
    }

    pub fn dequeue_missing_track(&self) -> Option<MissingTrack> {
        self.queue.pop()
    }

    pub fn missing_tracks_len(&self) -> usize {
        self.queue.len()
    }
}

/// Spawns a background task that reports request metrics every minute.
fn spawn_metrics_reporter(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let count = state.request_counter.swap(0, Ordering::Relaxed);
            tracing::info!(
                message = "requests in the last minute",
                requests_count = count
            );
        }
    });
}

/// Spawns a background task that updates the recent lyrics count every minute.
fn spawn_lyrics_counter(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut conn = state.pool.get().unwrap();
            let count = get_last_10_mins_lyrics_count(&mut conn).unwrap();
            state
                .recent_lyrics_count
                .store(count as usize, Ordering::Relaxed);
        }
    });
}

/// Builds the application router with all routes, middleware, and CORS configuration.
fn build_router(state: Arc<AppState>, state_for_logging: Arc<AppState>) -> Router {
    // Build API routes
    let api_routes = Router::new()
        .route("/get", get(get_lyrics_by_metadata::route))
        .route("/get/:track_id", get(get_lyrics_by_track_id::route))
        .route("/search", get(search_lyrics::route))
        .route("/request-challenge", post(request_challenge::route))
        .route("/publish", post(publish_lyrics::route))
        .route("/flag", post(flag_lyrics::route));

    // Build management routes
    let manage_routes = Router::new().route("/set-config", post(manage::set_config::route));

    // Add management routes to the API router
    let api_routes = api_routes.nest("/manage", manage_routes);

    // Build the MCP (Model Context Protocol) service, sharing the same AppState
    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(LrclibMcpServer::new(state.clone()))
        },
        LocalSessionManager::default().into(),
        // Same trust model as the rest of the public API (open CORS, no Host allowlist):
        // this server is designed to run behind arbitrary reverse proxies, so the
        // default loopback-only `Host` allowlist (DNS-rebinding protection) is disabled.
        StreamableHttpServerConfig::default().disable_allowed_hosts(),
    );

    Router::new()
        .nest("/api", api_routes)
        .nest_service("/mcp", mcp_service)
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<Body>| {
                    let headers = request.headers();
                    let user_agent = headers
                        .get("Lrclib-Client")
                        .and_then(|value| value.to_str().ok())
                        .or_else(|| {
                            headers
                                .get("X-User-Agent")
                                .and_then(|value| value.to_str().ok())
                        })
                        .or_else(|| {
                            headers
                                .get(header::USER_AGENT)
                                .and_then(|value| value.to_str().ok())
                        })
                        .unwrap_or("");
                    let method = request.method().to_string();
                    let uri = request.uri().to_string();

                    tracing::debug_span!("request", method, uri, user_agent)
                })
                .on_response(|response: &Response, latency: Duration, _span: &Span| {
                    let status_code = response.status().as_u16();
                    let latency = latency.as_millis();

                    if latency > 500 {
                        tracing::info!(
                            message = "finished processing request",
                            slow = true,
                            latency = latency,
                            status_code = status_code,
                        )
                    } else {
                        tracing::debug!(
                            message = "finished processing request",
                            latency = latency,
                            status_code = status_code,
                        )
                    }
                })
                .on_failure(trace::DefaultOnFailure::new().level(tracing::Level::ERROR))
                .on_request(move |_request: &Request<Body>, _span: &Span| {
                    state_for_logging
                        .request_counter
                        .fetch_add(1, Ordering::Relaxed);
                }),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers([
                    header::CONTENT_TYPE,
                    "X-User-Agent".parse().unwrap(),
                    "Lrclib-Client".parse().unwrap(),
                ]),
        )
}

/// Spawns the worker supervisor for dynamic worker count adjustment.
async fn spawn_worker_supervisor<F, Fut>(
    state: Arc<AppState>,
    initial_workers: u8,
    queue_starter: F,
) where
    F: Fn(u8, watch::Receiver<u8>, Arc<AppState>) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    // Store the initial workers count in AppState
    state.workers_count.store(initial_workers, Ordering::SeqCst);

    // Channel used to instruct the queue supervisor about desired worker count.
    let (workers_tx, workers_rx) = watch::channel::<u8>(initial_workers);

    // Store the sender in AppState for later use by the API
    *state.workers_tx.lock().await = Some(workers_tx);

    // Spawn the supervisor (it will in turn spawn workers).
    tokio::spawn(queue_starter(initial_workers, workers_rx, state.clone()));
}

/// Build the application state with all caches and shared resources.
fn build_app_state(pool: Pool<SqliteConnectionManager>) -> Arc<AppState> {
    Arc::new(AppState {
        pool,
        challenge_cache: Cache::<String, String>::builder()
            .time_to_live(Duration::from_secs(60 * 5))
            .max_capacity(500000)
            .build(),
        get_cache: Cache::<String, String>::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 24 * 7))
            .max_capacity(10000000)
            .build(),
        get_metadata_cache: Cache::<String, GetLyricsByMetadataResponse>::builder()
            .max_capacity(5000000)
            .build(),
        get_metadata_index: DashMap::new(),
        search_cache: Cache::<String, CachedResult>::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 24))
            .time_to_idle(Duration::from_secs(60 * 60 * 4))
            .max_capacity(5000000)
            .build(),
        queue: ArrayQueue::new(600000),
        request_counter: AtomicUsize::new(0),
        recent_lyrics_count: AtomicUsize::new(0),
        workers_count: AtomicU8::new(0),
        workers_tx: Mutex::new(None),
    })
}

/// Start the LRCLIB server on the specified port with the given database and worker count.
pub async fn serve(port: u16, database: &PathBuf, workers_count: u8) {
    serve_with_queue(port, database, workers_count, queue::start_queue).await;
}

/// Start the LRCLIB server with a custom queue backend.
pub async fn serve_with_queue<F, Fut>(
    port: u16,
    database: &PathBuf,
    workers_count: u8,
    queue_starter: F,
) where
    F: Fn(u8, watch::Receiver<u8>, Arc<AppState>) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    // Initialize logging
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(EnvFilter::from_env("LRCLIB_LOG"))
        .init();

    // Initialize database and application state
    let pool = init_db(database).expect("Cannot initialize connection to SQLite database!");
    let state = build_app_state(pool);

    // Spawn background tasks
    let state_for_logging = state.clone();
    spawn_worker_supervisor(state.clone(), workers_count, queue_starter).await;
    spawn_metrics_reporter(state.clone());
    spawn_lyrics_counter(state.clone());

    // Build the router and start the server
    let app = build_router(state, state_for_logging);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();

     println!(
        "LRCLIB server v{} is listening on {}!",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr().unwrap()
    );

    // Run the server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
