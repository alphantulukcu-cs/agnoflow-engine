mod config;
mod error;
mod routes;
mod state;

use std::sync::Arc;
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wf_wfe::{OrgAdapter, WfeAdapter, WfeExecutor};

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
        .after_connect(|conn, _meta| Box::pin(async move {
            conn.execute("SET search_path TO org, public").await?;
            Ok(())
        }))
        .connect(&cfg.database_url)
        .await
        .expect("db connect failed");

    let storage = wf_wfd::build_operator(&cfg.storage).expect("storage init failed");

    let org_adapter = Arc::new(OrgAdapter::new(pool.clone()));
    let wfd_adapter = Arc::new(wf_wfd::WfdAdapter::new(pool.clone(), storage));
    let wfe_adapter = Arc::new(WfeAdapter::new(pool.clone()));

    let executor = Arc::new(WfeExecutor::new(
        org_adapter.clone(),
        wfd_adapter.clone(),
        wfe_adapter,
    ));

    let state = state::AppState {
        pool:     pool.clone(),
        executor,
        wfd:      wfd_adapter,
        cfg:      cfg.clone(),
    };

    let app = Router::new()
        .nest("/org",          routes::org::router(pool.clone()))
        .nest("/wfd",          routes::wfd::router(state.clone()))
        .nest("/wfe/simulate", routes::simulate::router(state.clone()))
        .nest("/wfe",          routes::wfe::router(state.clone()))
        .nest("/autoexec",     routes::autoexec::router(state.clone()))
        .nest("/portal",       routes::portal::router(state.clone()))
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
