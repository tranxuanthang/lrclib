//! HTTP runtime for the LRCLIB API server.

mod background;
mod bootstrap;
mod metrics;
mod router;
mod state;

pub(crate) use lrclib_core::{db, entities, lyricsfile, repositories};

pub mod errors;
pub mod routes;
pub mod search_index;
pub mod utils;

pub use bootstrap::serve;
pub use state::AppState;
