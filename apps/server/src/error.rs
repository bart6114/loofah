use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode};

tokio::task_local! { pub static REQUEST_ID: String; }

pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug)]
pub struct ApiError(pub ApplicationError);

impl ApiError {
    pub fn new(code: ErrorCode, message: &str) -> Self {
        Self(ApplicationError {
            code,
            message: message.into(),
            request_id: None,
            retryable: code == ErrorCode::Unavailable,
            current_revision: None,
            mutation_outcome: None,
        })
    }

    pub fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!(%cause, "server operation failed");
        Self::new(ErrorCode::Internal, "Server operation failed")
    }

    pub fn busy(message: &str) -> Self {
        let mut error = Self::new(ErrorCode::ResourceLimit, message);
        error.0.retryable = true;
        error
    }
}

impl From<ApplicationError> for ApiError {
    fn from(error: ApplicationError) -> Self {
        Self(error)
    }
}
impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}
impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::internal(error)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(mut self) -> Response {
        if self.0.request_id.is_none() {
            self.0.request_id = REQUEST_ID.try_with(Clone::clone).ok();
        }
        let status = match self.0.code {
            ErrorCode::Validation => StatusCode::BAD_REQUEST,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::IncompatibleVersion => StatusCode::UPGRADE_REQUIRED,
            ErrorCode::ResourceLimit => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Cancelled => StatusCode::REQUEST_TIMEOUT,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let retryable = self.0.retryable;
        let mut response = (status, Json(self.0)).into_response();
        if retryable {
            response
                .headers_mut()
                .insert("retry-after", "1".parse().unwrap());
        }
        response
            .headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        if status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert("www-authenticate", "Bearer".parse().unwrap());
        }
        response
    }
}
