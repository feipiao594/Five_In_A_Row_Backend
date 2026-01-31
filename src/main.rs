use axum::{routing::get, Router};
use server::{api, config::Config, db, rooms, ws};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  dotenvy::dotenv().ok();
  tracing_subscriber::fmt()
      .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
      .init();

  let cfg = Config::from_env()?;
  let bind_addr = cfg.bind_addr;
  let pool = db::connect(
    &cfg.database_url,
    cfg.db_max_connections,
    cfg.db_connect_timeout_secs,
    cfg.db_acquire_timeout_secs,
  )
  .await?;
  db::migrate(&pool).await?;

  let hub = ws::Hub::default();
  let rooms = rooms::RoomService::default();

  let app_state = api::AppState { cfg, pool, hub, rooms };

  let app = Router::new()
      .route("/healthz", get(api::healthz))
      .merge(api::router(app_state))
      .layer(CorsLayer::permissive())
      .layer(TraceLayer::new_for_http());

  let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
  tracing::info!("listening on {}", bind_addr);
  axum::serve(listener, app).await?;
  Ok(())
}
