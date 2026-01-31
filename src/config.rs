use std::{env, net::SocketAddr};

use anyhow::Context;

#[derive(Clone)]
pub struct Config {
  pub database_url: String,
  pub db_max_connections: u32,
  pub db_connect_timeout_secs: u64,
  pub db_acquire_timeout_secs: u64,
  pub jwt_secret: String,
  pub access_token_ttl_secs: i64,
  pub refresh_token_ttl_secs: i64,
  // If refresh token remaining lifetime is <= this threshold, rotate it on /refresh.
  // Otherwise keep the same refresh token and only mint a new access token.
  pub refresh_token_rotate_threshold_secs: i64,
  pub bind_addr: SocketAddr,
}

impl Config {
  pub fn from_env() -> anyhow::Result<Self> {
    let database_url =
        env::var("DATABASE_URL").context("missing env DATABASE_URL (see server/.env.example)")?;

    let db_max_connections = env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let db_connect_timeout_secs = env::var("DB_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let db_acquire_timeout_secs = env::var("DB_ACQUIRE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let jwt_secret =
        env::var("JWT_SECRET").context("missing env JWT_SECRET (see server/.env.example)")?;
    let access_token_ttl_secs = env::var("ACCESS_TOKEN_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let refresh_token_ttl_secs = env::var("REFRESH_TOKEN_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30 * 24 * 3600);
    let refresh_token_rotate_threshold_secs = env::var("REFRESH_TOKEN_ROTATE_THRESHOLD_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24 * 3600)
        .clamp(0, refresh_token_ttl_secs);
    let bind_addr: SocketAddr = env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()
        .context("invalid env BIND_ADDR (expected host:port)")?;

    Ok(Self {
      database_url,
      db_max_connections,
      db_connect_timeout_secs,
      db_acquire_timeout_secs,
      jwt_secret,
      access_token_ttl_secs,
      refresh_token_ttl_secs,
      refresh_token_rotate_threshold_secs,
      bind_addr,
    })
  }
}
