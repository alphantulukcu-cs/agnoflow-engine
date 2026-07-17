mod config;
mod error;
mod routes;
mod state;

use axum::{response::IntoResponse, Router};
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

    // M5/M6 — escalation & root-timeout süpürücüsü (WOR-46/47).
    // Event-driven (2026-07-17): en yakın SLA vadesine kadar uyur; executor
    // create/commit/claim'de sinyal gönderir. 60 sn'lik FALLBACK_SWEEP kaçan
    // sinyallere karşı güvenlik ağı olarak timer.rs içinde korunur.
    tokio::spawn(wf_wfe::timer::run_timer_service(
        executor.clone(),
        Arc::new(pool.clone()),
    ));

    let state = state::AppState {
        pool: pool.clone(),
        executor,
        wfd: wfd_adapter,
        cfg: cfg.clone(),
    };

    // WOR-10: /org ve /db admin API'leri anahtar korumalı — ADMIN_API_KEY set
    // edilmemişse yalnızca dev için açık kalır ve yüksek sesle uyarılır.
    let admin_key = cfg.admin_api_key.clone();
    if admin_key.is_none() {
        tracing::warn!(
            "ADMIN_API_KEY tanımlı değil — /org ve /db admin API'leri KORUMASIZ (yalnızca dev için kabul edilebilir)"
        );
    }
    // Aynı X-Admin-Key kapısını hem /org hem /db router'ına uygular.
    let guard = |router: Router| -> Router {
        match admin_key.clone() {
            Some(key) => router.layer(axum::middleware::from_fn(
                move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let key = key.clone();
                    async move {
                        let provided = req
                            .headers()
                            .get("x-admin-key")
                            .and_then(|v| v.to_str().ok());
                        if provided == Some(key.as_str()) {
                            next.run(req).await
                        } else {
                            (axum::http::StatusCode::UNAUTHORIZED, "X-Admin-Key required")
                                .into_response()
                        }
                    }
                },
            )),
            None => router,
        }
    };

    let org_router = guard(routes::org::router(pool.clone()));
    let db_router = guard(routes::db::router(state.clone()));

    let app = Router::new()
        .nest("/org", org_router)
        .nest("/db", db_router)
        .nest("/auth", routes::auth::router(state.clone()))
        .nest("/users", routes::users::router(state.clone()))
        .nest("/project", routes::project::router(state.clone()))
        .nest("/templates", routes::templates::router(state.clone()))
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
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers(tower_http::cors::Any)
}
