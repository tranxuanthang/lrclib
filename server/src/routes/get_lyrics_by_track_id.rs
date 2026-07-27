use crate::{
    errors::ApiError,
    repositories::track_repository::get_track_by_id, AppState,
    track_response::{TrackResponse, create_track_response},
};
use axum::{
    extract::{Path, State},
    Json,
};
use std::sync::Arc;


pub async fn route(
    Path(track_id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<TrackResponse>, ApiError> {
    let maybe_track = {
        let mut conn = state.pool.get()?;
        get_track_by_id(track_id, &mut conn)?
    };

    match maybe_track {
        Some(track) => Ok(Json(create_track_response(track))),
        None => Err(ApiError::TrackNotFoundError),
    }
}
