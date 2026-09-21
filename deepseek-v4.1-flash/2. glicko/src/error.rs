//! Error type shared by every handler.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = match &self {
            AppError::BadRequest(message)
            | AppError::NotFound(message)
            | AppError::Internal(message) => message.clone(),
        };
        (self.status(), Json(ErrorResponse { error: message })).into_response()
    }
}

impl From<crate::store::StoreError> for AppError {
    fn from(error: crate::store::StoreError) -> Self {
        match error {
            crate::store::StoreError::Invalid(message) => AppError::BadRequest(message),
            crate::store::StoreError::Internal(message) => AppError::Internal(message),
        }
    }
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        AppError::Internal(message)
    }
}
