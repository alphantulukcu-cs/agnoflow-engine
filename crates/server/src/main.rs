mod config;
mod error;
mod routes;
mod state;

use axum::Router;
use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wf_wfe::{LiveAutoexecRunner, OrgAdapter, WfeAdapter, WfeExecutor};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = Arc::new(config::Config::from_env().expect("config error"));

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                conn.execute("SET search_path TO org, public").await?;
                Ok(())
            })
        })
        .connect(&cfg.database_url)
        .await
        .expect("db connect failed");

    let storage = wf_wfd::build_operator(&cfg.storage).expect("storage init failed");

    let org_adapter = Arc::new(OrgAdapter::new(pool.clone()));
    let wfd_adapter = Arc::new(wf_wfd::WfdAdapter::new(pool.clone(), storage));
    let wfe_adapter = Arc::new(WfeAdapter::new(pool.clone()));
    let runner = Arc::new(LiveAutoexecRunner::new(Some(pool.clone())));

    let executor = Arc::new(WfeExecutor::new(
        org_adapter.clone(),
        wfd_adapter.clone(),
        wfe_adapter,
        runner,
    ));

    // M5/M6 — escalation & root-timeout süpürücüsü (WOR-46/47)
    let sweeper_executor = executor.clone();
    let sweeper_pool = pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let ids = match wf_wfe::repo::wfe::list_active_ids(&sweeper_pool).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("timer sweep: aktif WFE listesi alınamadı: {e}");
                    continue;
                }
            };
            for wfe_id in ids {
                match sweeper_executor.tick_timers(wfe_id).await {
                    Ok(true) => tracing::info!("timer fired for wfe {wfe_id}"),
                    Ok(false) => {}
                    Err(e) => tracing::warn!("timer sweep {wfe_id}: {e}"),
                }
            }
        }
    });

    let state = state::AppState {
        pool: pool.clone(),
        executor,
        wfd: wfd_adapter,
        cfg: cfg.clone(),
    };

    let app = Router::new()
        .nest("/org", routes::org::router(pool.clone()))
        .nest("/wfd", routes::wfd::router(state.clone()))
        .nest("/wfe/simulate", routes::simulate::router(state.clone()))
        .nest("/wfe", routes::wfe::router(state.clone()))
        .nest("/autoexec", routes::autoexec::router(state.clone()))
        .nest("/portal", routes::portal::router(state.clone()))
        .layer(cors_layer(&cfg));

    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// WOR-11: CORS origin'leri env'den alınır; verilmemişse yalnızca localhost
/// geliştirme origin'lerine izin verilir — production'da permissive YOK.
fn cors_layer(cfg: &config::Config) -> CorsLayer {
    use axum::http::{HeaderValue, Method};
    let origins: Vec<HeaderValue> = cfg
        .cors_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(tower_http::cors::Any)
}
