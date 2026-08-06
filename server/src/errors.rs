use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::fmt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorResponse {
    message: String,
    name: String,
    status_code: u16,
}

#[derive(Debug)]
pub struct OverloadedError;

impl fmt::Display for OverloadedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the server is busy")
    }
}

impl std::error::Error for OverloadedError {}

pub enum ApiError {
    TrackNotFoundError,
    IncorrectPublishTokenError,
    ValidationError(String),
    UnknownError(anyhow::Error),
    OverloadedError,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::TrackNotFoundError => (
                StatusCode::NOT_FOUND,
                Json(ApiErrorResponse {
                    message: "Failed to find specified track".to_owned(),
                    name: "TrackNotFound".to_owned(),
                    status_code: StatusCode::NOT_FOUND.as_u16(),
                }),
            )
                .into_response(),
            ApiError::IncorrectPublishTokenError => (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorResponse {
                    message: "The provided publish token is incorrect".to_owned(),
                    name: "IncorrectPublishTokenError".to_owned(),
                    status_code: StatusCode::BAD_REQUEST.as_u16(),
                }),
            )
                .into_response(),
            ApiError::ValidationError(err_msg) => (
                StatusCode::BAD_REQUEST,
                Json(ApiErrorResponse {
                    message: err_msg,
                    name: "ValidationError".to_owned(),
                    status_code: StatusCode::BAD_REQUEST.as_u16(),
                }),
            )
                .into_response(),
            ApiError::UnknownError(_err) => {
                // tracing::error!(message = "unknown error happened", error = err.to_string());
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiErrorResponse {
                        message: "Something bad happened when processing your request".to_owned(),
                        name: "UnknownError".to_owned(),
                        status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    }),
                )
                    .into_response()
            }
            ApiError::OverloadedError => {
                let mut response = (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ApiErrorResponse {
                        message: "The server is busy, please retry in a moment".to_owned(),
                        name: "ServerOverloaded".to_owned(),
                        status_code: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                    }),
                )
                    .into_response();
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, "1".parse().unwrap());
                response
            }
        }
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        ApiError::UnknownError(err.into())
    }
}
