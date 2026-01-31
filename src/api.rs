use axum::{
  extract::{FromRef, State},
  routing::{get, post},
  Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::{
  auth,
  config::Config,
  error::{ApiError, ApiResult},
  rooms,
  ws,
};

#[derive(Clone)]
pub struct AppState {
  pub cfg: Config,
  pub pool: PgPool,
  pub hub: ws::Hub,
  pub rooms: rooms::RoomService,
}

impl FromRef<AppState> for Config {
  fn from_ref(state: &AppState) -> Self {
    state.cfg.clone()
  }
}

impl FromRef<AppState> for PgPool {
  fn from_ref(state: &AppState) -> Self {
    state.pool.clone()
  }
}

impl FromRef<AppState> for ws::Hub {
  fn from_ref(state: &AppState) -> Self {
    state.hub.clone()
  }
}

impl FromRef<AppState> for rooms::RoomService {
  fn from_ref(state: &AppState) -> Self {
    state.rooms.clone()
  }
}

pub fn router(state: AppState) -> Router {
  Router::new()
      .nest(
        "/api/v1/auth",
        Router::new()
            .route("/register", post(register))
            .route("/login", post(login))
            .route("/refresh", post(refresh))
            .route("/me", get(me))
            .route("/logout", post(logout)),
      )
      .route("/ws", get(ws::ws_handler))
      .with_state(state)
}

pub async fn healthz() -> &'static str {
  "ok"
}

#[derive(Debug, Deserialize)]
struct RegisterReq {
  username: String,
  password: String,
}

#[derive(Debug, Serialize)]
struct RegisterResp {
  username: String,
}

async fn register(
  State(pool): State<PgPool>,
  Json(req): Json<RegisterReq>,
) -> ApiResult<Json<RegisterResp>> {
  auth::create_user(&pool, req.username.trim(), &req.password).await?;
  Ok(Json(RegisterResp {
    username: req.username.trim().to_string(),
  }))
}

#[derive(Debug, Deserialize)]
struct LoginReq {
  username: String,
  password: String,
}

#[derive(Debug, Serialize)]
struct LoginResp {
  username: String,
  #[serde(rename = "accessToken")]
  access_token: String,
  #[serde(rename = "accessTokenExpiresIn")]
  access_token_expires_in: i64,
  #[serde(rename = "refreshToken")]
  refresh_token: String,
  #[serde(rename = "refreshTokenExpiresIn")]
  refresh_token_expires_in: i64,
}

async fn login(
  State(cfg): State<Config>,
  State(pool): State<PgPool>,
  State(hub): State<ws::Hub>,
  Json(req): Json<LoginReq>,
) -> ApiResult<Json<LoginResp>> {
  let username = req.username.trim().to_string();
  let tokens = auth::login(&pool, &cfg, &username, &req.password).await?;

  // Single-session policy: kick any existing WS connection for this username.
  hub.kick(&username).await;

  Ok(Json(LoginResp {
    username,
    access_token: tokens.access_token,
    access_token_expires_in: tokens.access_expires_in,
    refresh_token: tokens.refresh_token,
    refresh_token_expires_in: tokens.refresh_expires_in,
  }))
}

#[derive(Debug, Deserialize)]
struct RefreshReq {
  #[serde(rename = "refreshToken")]
  refresh_token: String,
}

#[derive(Debug, Serialize)]
struct RefreshResp {
  #[serde(rename = "accessToken")]
  access_token: String,
  #[serde(rename = "accessTokenExpiresIn")]
  access_token_expires_in: i64,
  #[serde(rename = "refreshToken")]
  refresh_token: String,
  #[serde(rename = "refreshTokenExpiresIn")]
  refresh_token_expires_in: i64,
}

async fn refresh(
  State(cfg): State<Config>,
  State(pool): State<PgPool>,
  Json(req): Json<RefreshReq>,
) -> ApiResult<Json<RefreshResp>> {
  let tokens = auth::refresh(&pool, &cfg, &req.refresh_token).await?;
  Ok(Json(RefreshResp {
    access_token: tokens.access_token,
    access_token_expires_in: tokens.access_expires_in,
    refresh_token: tokens.refresh_token,
    refresh_token_expires_in: tokens.refresh_expires_in,
  }))
}

#[derive(Debug, Serialize)]
struct MeResp {
  username: String,
}

async fn me(
  State(cfg): State<Config>,
  headers: axum::http::HeaderMap,
) -> ApiResult<Json<MeResp>> {
  let authz = headers
      .get(axum::http::header::AUTHORIZATION)
      .and_then(|v| v.to_str().ok())
      .ok_or(ApiError::Unauthorized)?;

  let token = authz
      .strip_prefix("Bearer ")
      .ok_or(ApiError::Unauthorized)?;

  let claims = auth::verify_access_token(&cfg, token)?;
  Ok(Json(MeResp { username: claims.sub }))
}

#[derive(Debug, Deserialize)]
struct LogoutReq {
  #[serde(rename = "refreshToken")]
  refresh_token: String,
}

#[derive(Debug, Serialize)]
struct LogoutResp {
  ok: bool,
}

async fn logout(
  State(pool): State<PgPool>,
  Json(req): Json<LogoutReq>,
) -> ApiResult<Json<LogoutResp>> {
  auth::logout(&pool, &req.refresh_token).await?;
  Ok(Json(LogoutResp { ok: true }))
}
