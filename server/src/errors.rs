use axum::{
  response::{IntoResponse, Response},
  http::StatusCode,
  Json,
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorResponse {
  message: String,
  name: String,
  status_code: u16
}

pub enum ApiError {
  TrackNotFoundError,
  IncorrectPublishTokenError,
  ValidationError(String),
  UnknownError(anyhow::Error),
}

impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    match self {
      ApiError::TrackNotFoundError => (
          StatusCode::NOT_FOUND,
          Json(
            ApiErrorResponse {
              message: "Failed to find specified track".to_owned(),
              name: "TrackNotFound".to_owned(),
              status_code: StatusCode::NOT_FOUND.as_u16(),
            }
          )
        ).into_response(),
      ApiError::IncorrectPublishTokenError => (
        StatusCode::BAD_REQUEST,
        Json(
          ApiErrorResponse {
            message: "The provided publish token is incorrect".to_owned(),
            name: "IncorrectPublishTokenError".to_owned(),
            status_code: StatusCode::BAD_REQUEST.as_u16(),
          }
        )
      ).into_response(),
      ApiError::ValidationError(err_msg) => (
        StatusCode::BAD_REQUEST,
        Json(ApiErrorResponse {
          message: err_msg,
          name: "ValidationError".to_owned(),
          status_code: StatusCode::BAD_REQUEST.as_u16(),
        }),
      ).into_response(),
      ApiError::UnknownError(err) => {
        tracing::error!(message = "unknown error happened", error = err.to_string());
        (
          StatusCode::INTERNAL_SERVER_ERROR,
          Json(
            ApiErrorResponse {
              message: "Something bad happened when processing your request".to_owned(),
              name: "UnknownError".to_owned(),
              status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            }
          )
        ).into_response()
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
