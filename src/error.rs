use axum::{
  http::StatusCode,
  response::{IntoResponse, Response},
  Json,
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Serialize)]
pub struct ErrorBody {
  pub ok: bool,
  pub error: ErrorInfo,
}

#[derive(Debug, Serialize)]
pub struct ErrorInfo {
  pub code: &'static str,
  pub message: &'static str,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub details: Option<serde_json::Value>,
}

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ApiError {
  #[error("bad request")]
  BadRequest,
  #[error("unauthorized")]
  Unauthorized,
  #[error("forbidden")]
  Forbidden,
  #[error("username taken")]
  UsernameTaken,
  #[error("invalid credentials")]
  InvalidCredentials,
  #[error("token expired")]
  TokenExpired,
  #[error("rate limited")]
  RateLimited,
  #[error("internal error")]
  Internal,
}

impl ApiError {
  pub fn code_message(&self) -> (&'static str, &'static str) {
    match self {
      ApiError::BadRequest => ("bad_request", "请求参数错误"),
      ApiError::Unauthorized => ("unauthorized", "未登录或登录已失效"),
      ApiError::Forbidden => ("forbidden", "无权限执行该操作"),
      ApiError::UsernameTaken => ("username_taken", "用户名已存在"),
      ApiError::InvalidCredentials => ("invalid_credentials", "账号或密码错误"),
      ApiError::TokenExpired => ("token_expired", "登录已过期，请重新登录"),
      ApiError::RateLimited => ("rate_limited", "请求过于频繁，请稍后再试"),
      ApiError::Internal => ("internal_error", "服务器内部错误"),
    }
  }

  pub fn status(&self) -> StatusCode {
    match self {
      ApiError::BadRequest => StatusCode::BAD_REQUEST,
      ApiError::Unauthorized
      | ApiError::InvalidCredentials
      | ApiError::TokenExpired => StatusCode::UNAUTHORIZED,
      ApiError::Forbidden => StatusCode::FORBIDDEN,
      ApiError::UsernameTaken => StatusCode::CONFLICT,
      ApiError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
      ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
  }
}

impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    let (code, message) = self.code_message();
    let status = self.status();
    let body = ErrorBody {
      ok: false,
      error: ErrorInfo {
        code,
        message,
        details: None,
      },
    };
    (status, Json(body)).into_response()
  }
}

pub type ApiResult<T> = Result<T, ApiError>;
