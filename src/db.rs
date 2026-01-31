use std::time::Duration;

use sqlx::{postgres::PgPoolOptions, PgPool};

pub async fn connect(
  database_url: &str,
  max_connections: u32,
  connect_timeout_secs: u64,
  acquire_timeout_secs: u64,
) -> anyhow::Result<PgPool> {
  let connect_fut = PgPoolOptions::new()
      .max_connections(max_connections)
      .acquire_timeout(Duration::from_secs(acquire_timeout_secs))
      .test_before_acquire(true)
      .connect(database_url);

  let pool = tokio::time::timeout(Duration::from_secs(connect_timeout_secs), connect_fut)
      .await
      .map_err(|_| anyhow::anyhow!("database connect timeout"))??;
  Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
  sqlx::migrate!("./migrations").run(pool).await?;
  Ok(())
}
