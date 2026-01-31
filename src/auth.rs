use std::time::{SystemTime, UNIX_EPOCH};

use argon2::{
  password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
  Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{config::Config, error::ApiError};

#[derive(Debug, Clone)]
pub struct Tokens {
  pub access_token: String,
  pub access_expires_in: i64,
  pub refresh_token: String,
  pub refresh_expires_in: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
  pub sub: String, // username
  pub uid: String, // internal user id (uuid as string)
  pub exp: usize,
  pub iat: usize,
}

fn now_ts() -> usize {
  SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_secs() as usize
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
  let salt = SaltString::generate(&mut OsRng);
  let argon2 = Argon2::default();
  let hash = argon2
      .hash_password(password.as_bytes(), &salt)
      .map_err(|_| ApiError::Internal)?
      .to_string();
  Ok(hash)
}

pub fn verify_password(password: &str, password_hash: &str) -> Result<bool, ApiError> {
  let parsed = PasswordHash::new(password_hash).map_err(|_| ApiError::Internal)?;
  let argon2 = Argon2::default();
  Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
}

fn gen_refresh_token() -> String {
  let mut buf = [0u8; 32];
  OsRng.fill_bytes(&mut buf);
  URL_SAFE_NO_PAD.encode(buf)
}

fn hash_refresh_token(token: &str) -> String {
  let mut hasher = Sha256::new();
  hasher.update(token.as_bytes());
  let out = hasher.finalize();
  hex::encode(out)
}

pub fn mint_access_token(cfg: &Config, username: &str, uid: Uuid) -> Result<String, ApiError> {
  let iat = now_ts();
  let exp = (Utc::now() + Duration::seconds(cfg.access_token_ttl_secs)).timestamp() as usize;
  let claims = Claims {
    sub: username.to_string(),
    uid: uid.to_string(),
    exp,
    iat,
  };
  let header = Header::new(Algorithm::HS256);
  encode(
    &header,
    &claims,
    &EncodingKey::from_secret(cfg.jwt_secret.as_bytes()),
  )
  .map_err(|_| ApiError::Internal)
}

pub fn verify_access_token(cfg: &Config, token: &str) -> Result<Claims, ApiError> {
  let mut validation = Validation::new(Algorithm::HS256);
  validation.validate_exp = true;
  let data = decode::<Claims>(
    token,
    &DecodingKey::from_secret(cfg.jwt_secret.as_bytes()),
    &validation,
  )
  .map_err(|e| match e.kind() {
    jsonwebtoken::errors::ErrorKind::ExpiredSignature => ApiError::TokenExpired,
    _ => ApiError::Unauthorized,
  })?;
  Ok(data.claims)
}

pub async fn create_user(pool: &PgPool, username: &str, password: &str) -> Result<(), ApiError> {
  if username.is_empty() || password.len() < 6 {
    return Err(ApiError::BadRequest);
  }

  let password_hash = hash_password(password)?;
  let user_id = Uuid::new_v4();

  let res = sqlx::query(
    r#"
    INSERT INTO users (id, username, password_hash)
    VALUES ($1, $2, $3)
    "#,
  )
  .bind(user_id)
  .bind(username)
  .bind(password_hash)
  .execute(pool)
  .await;

  match res {
    Ok(_) => Ok(()),
    Err(e) => {
      if let Some(db_err) = e.as_database_error() {
        if db_err.code().as_deref() == Some("23505") {
          return Err(ApiError::UsernameTaken);
        }
      }
      Err(ApiError::Internal)
    }
  }
}

pub async fn login(pool: &PgPool, cfg: &Config, username: &str, password: &str) -> Result<Tokens, ApiError> {
  let row = sqlx::query(
    r#"
    SELECT id, password_hash
    FROM users
    WHERE username = $1
    "#,
  )
  .bind(username)
  .fetch_optional(pool)
  .await
  .map_err(|_| ApiError::Internal)?;

  let Some(row) = row else { return Err(ApiError::InvalidCredentials); };
  let user_id: Uuid = row.get("id");
  let password_hash: String = row.get("password_hash");
  if !verify_password(password, &password_hash)? {
    return Err(ApiError::InvalidCredentials);
  }

  let refresh_token = gen_refresh_token();
  let refresh_hash = hash_refresh_token(&refresh_token);
  let refresh_expires_at = Utc::now() + Duration::seconds(cfg.refresh_token_ttl_secs);

  // Single session: overwrite (revoke) previous by upserting unique(user_id).
  let session_id = Uuid::new_v4();
  sqlx::query(
    r#"
    INSERT INTO refresh_sessions (id, user_id, refresh_token_hash, expires_at, revoked_at)
    VALUES ($1, $2, $3, $4, NULL)
    ON CONFLICT (user_id) DO UPDATE SET
      id = EXCLUDED.id,
      refresh_token_hash = EXCLUDED.refresh_token_hash,
      expires_at = EXCLUDED.expires_at,
      revoked_at = NULL,
      created_at = now()
    "#,
  )
  .bind(session_id)
  .bind(user_id)
  .bind(refresh_hash)
  .bind(refresh_expires_at)
  .execute(pool)
  .await
  .map_err(|_| ApiError::Internal)?;

  let access_token = mint_access_token(cfg, username, user_id)?;

  Ok(Tokens {
    access_token,
    access_expires_in: cfg.access_token_ttl_secs,
    refresh_token,
    refresh_expires_in: cfg.refresh_token_ttl_secs,
  })
}

pub async fn refresh(pool: &PgPool, cfg: &Config, refresh_token: &str) -> Result<Tokens, ApiError> {
  let token_hash = hash_refresh_token(refresh_token);

  let row = sqlx::query(
    r#"
    SELECT rs.user_id, u.username, rs.expires_at, rs.revoked_at
    FROM refresh_sessions rs
    JOIN users u ON u.id = rs.user_id
    WHERE rs.refresh_token_hash = $1
    "#,
  )
  .bind(token_hash)
  .fetch_optional(pool)
  .await
  .map_err(|_| ApiError::Internal)?;

  let Some(row) = row else { return Err(ApiError::Unauthorized); };
  let user_id: Uuid = row.get("user_id");
  let username: String = row.get("username");
  let expires_at: DateTime<Utc> = row.get("expires_at");
  let revoked_at: Option<DateTime<Utc>> = row.get("revoked_at");
  if revoked_at.is_some() {
    return Err(ApiError::Unauthorized);
  }
  let now = Utc::now();
  if expires_at < now {
    return Err(ApiError::TokenExpired);
  }

  let remaining_secs = (expires_at - now).num_seconds();
  let rotate_threshold_secs = cfg
    .refresh_token_rotate_threshold_secs
    .clamp(0, cfg.refresh_token_ttl_secs);
  let should_rotate = remaining_secs <= rotate_threshold_secs;

  // Rotation: only rotate when near expiry (still single session per user).
  let new_refresh = gen_refresh_token();
  let new_hash = hash_refresh_token(&new_refresh);
  let new_expires_at = Utc::now() + Duration::seconds(cfg.refresh_token_ttl_secs);

  let access_token = mint_access_token(cfg, &username, user_id)?;

  if !should_rotate {
    return Ok(Tokens {
      access_token,
      access_expires_in: cfg.access_token_ttl_secs,
      refresh_token: refresh_token.to_string(),
      refresh_expires_in: remaining_secs.max(0),
    });
  }

  let new_session_id = Uuid::new_v4();
  sqlx::query(
    r#"
    UPDATE refresh_sessions
    SET id = $1,
        refresh_token_hash = $2,
        expires_at = $3,
        revoked_at = NULL,
        created_at = now()
    WHERE user_id = $4
    "#,
  )
  .bind(new_session_id)
  .bind(new_hash)
  .bind(new_expires_at)
  .bind(user_id)
  .execute(pool)
  .await
  .map_err(|_| ApiError::Internal)?;

  Ok(Tokens {
    access_token,
    access_expires_in: cfg.access_token_ttl_secs,
    refresh_token: new_refresh,
    refresh_expires_in: cfg.refresh_token_ttl_secs,
  })
}

pub async fn logout(pool: &PgPool, refresh_token: &str) -> Result<(), ApiError> {
  let token_hash = hash_refresh_token(refresh_token);
  sqlx::query(
    r#"
    UPDATE refresh_sessions
    SET revoked_at = now()
    WHERE refresh_token_hash = $1
    "#,
  )
  .bind(token_hash)
  .execute(pool)
  .await
  .map_err(|_| ApiError::Internal)?;
  Ok(())
}
