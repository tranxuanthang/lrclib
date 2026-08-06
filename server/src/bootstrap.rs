use crate::{
    background::{spawn_lyrics_counter, spawn_metrics_reporter, spawn_optimize_pragma},
    db::init_db,
    router::build_router,
    search_index,
    state::build_app_state,
};
use std::{path::PathBuf, sync::Arc};
use tokio::signal;
use tracing_subscriber::EnvFilter;

/// Start the LRCLIB server on the specified port with the given database.
pub async fn serve(port: u16, database: &PathBuf) {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(EnvFilter::from_env("LRCLIB_LOG"))
        .init();

    let pool = init_db(database).expect("Cannot initialize connection to SQLite database!");
    let (search_index, search_indexer) = search_index::bootstrap_from_env(database, &pool);
    let state = build_app_state(pool, search_index);

    spawn_metrics_reporter(Arc::clone(&state));
    spawn_lyrics_counter(Arc::clone(&state));
    spawn_optimize_pragma(Arc::clone(&state));

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();

    println!(
        "LRCLIB server v{} is listening on {}!",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr().unwrap()
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    if let Some(indexer) = search_indexer {
        indexer.shutdown();
    }
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
