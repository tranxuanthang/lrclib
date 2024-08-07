use axum::{
  http::{
    header,
    Request,
  },
  body::Body,
  response::Response,
  routing::{get, post},
  Router,
};
use entities::missing_track::MissingTrack;
use tracing_subscriber::EnvFilter;
use std::{collections::VecDeque, path::PathBuf, time::Duration};
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
  cors::{Any, CorsLayer}, trace::{self, TraceLayer}
};
use tracing::Span;
use moka::future::Cache;
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
  challenge_cache: Cache<String, String>,
  get_cache: Cache<String, String>,
  search_cache: Cache<String, String>,
  queue: Mutex<VecDeque<MissingTrack>>,
}

pub async fn serve(port: u16, database: &PathBuf, workers_count: u8) {
  tracing_subscriber::fmt()
    .compact()
    .with_env_filter(EnvFilter::from_env("LRCLIB_LOG"))
    .init();

  let pool = init_db(database).expect("Cannot initialize connection to SQLite database!");

  let state = Arc::new(
    AppState {
      pool,
      challenge_cache: Cache::<String, String>::builder()
        .time_to_live(Duration::from_secs(60 * 5))
        .max_capacity(500000)
        .build(),
      get_cache: Cache::<String, String>::builder()
        .time_to_live(Duration::from_secs(60 * 60 * 72))
        .max_capacity(500000)
        .build(),
      search_cache: Cache::<String, String>::builder()
        .time_to_live(Duration::from_secs(60 * 60 * 24))
        .max_capacity(2000000)
        .build(),
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
        .make_span_with(|request: &Request<Body>| {
          let headers = request.headers();
          let user_agent = headers
            .get("Lrclib-Client")
            .and_then(|value| value.to_str().ok())
            .or_else(|| headers.get("X-User-Agent").and_then(|value| value.to_str().ok()))
            .or_else(|| headers.get(header::USER_AGENT).and_then(|value| value.to_str().ok()))
            .unwrap_or("");
          let method = request.method().to_string();
          let uri = request.uri().to_string();

          tracing::debug_span!("request", method, uri, user_agent)
        })
        .on_response(|response: &Response, latency: Duration, _span: &Span| {
          let status_code = response.status().as_u16();
          let latency = latency.as_millis();

          tracing::debug!(
            message = "finished processing request",
            latency = latency,
            status_code = status_code,
          )
        })
        .on_failure(trace::DefaultOnFailure::new().level(tracing::Level::ERROR))
    )
    .layer(
      CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers([
          header::CONTENT_TYPE,
          "X-User-Agent".parse().unwrap(),
          "Lrclib-Client".parse().unwrap()
        ])
    );

  tokio::spawn(async move {
    start_queue(workers_count, state_clone).await;
  });

  let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await.unwrap();
  println!("LRCLIB server is listening on {}!", listener.local_addr().unwrap());
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
