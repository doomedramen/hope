use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);
impl ApiError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Authentication required".into())
    }
    pub fn not_found() -> Self {
        Self(StatusCode::NOT_FOUND, "Not found".into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(value: E) -> Self {
        let error = value.into();
        tracing::error!(error=%error, "request failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error".into(),
        )
    }
}
pub type ApiResult<T> = Result<T, ApiError>;
