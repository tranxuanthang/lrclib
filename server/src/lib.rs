use axum::{
  routing::{get, post},
  Router,
  http::header::CONTENT_TYPE,
};
use entities::missing_track::MissingTrack;
use std::{collections::VecDeque, path::PathBuf};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use routes::{
  get_lyrics_by_metadata,
  get_lyrics_by_track_id,
  search_lyrics,
  request_challenge,
  publish_lyrics,
};
use std::sync::Arc;
use db::init_db;
use tower_http::{
  trace::{self, TraceLayer},
  cors::{CorsLayer, Any},
};
use tracing::Level;
use ttl_cache::TtlCache;
use tokio::sync::Mutex;
use tokio::signal;
use queue::start_queue;

pub mod errors;
pub mod routes;
pub mod entities;
pub mod repositories;
pub mod utils;
pub mod db;
pub mod queue;
pub mod providers;

pub struct AppState {
  pool: Pool<SqliteConnectionManager>,
  cache: Mutex<TtlCache<String, String>>,
  queue: Mutex<VecDeque<MissingTrack>>,
}

pub async fn serve(port: u16, database: &PathBuf) {
  tracing_subscriber::fmt()
    .with_target(false)
    .json()
    .init();

  let pool = init_db(database).expect("Cannot initialize connection to SQLite database!");

  let state = Arc::new(
    AppState {
      pool,
      cache: TtlCache::<String, String>::new(1000).into(),
      queue: VecDeque::new().into(),
    }
  );

  let state_clone = state.clone();

  let api_routes = Router::new()
    .route("/get", get(get_lyrics_by_metadata::route))
    .route("/get/:track_id", get(get_lyrics_by_track_id::route))
    .route("/search", get(search_lyrics::route))
    .route("/request-challenge", post(request_challenge::route))
    .route("/publish", post(publish_lyrics::route));

  let app = Router::new()
    .nest("/api", api_routes)
    .with_state(state)
    .layer(
      TraceLayer::new_for_http()
        .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
        .on_response(trace::DefaultOnResponse::new().level(Level::INFO))
    )
    .layer(
      CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers([CONTENT_TYPE])
    );

  start_queue(state_clone).await;

  let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await.unwrap();
  tracing::info!("Listening on {}...", listener.local_addr().unwrap());
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
