//! Autoexec test endpoint'i — editör bir autoexec tanımını gerçek çalıştırır.
//! v2.2: AutoexecDef {type, timeout_seconds, config} formatı.

use crate::{error::AppError, state::AppState};
use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wf_wfe::LiveAutoexecRunner;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::AutoexecDef;
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/test", post(test_autoexec))
        .with_state(state)
}

#[derive(Deserialize)]
pub struct TestAutoexecBody {
    /// v2.2 autoexec tanımı: {"type": "rest|sql|calc", "config": {...}, "timeout_seconds"?}
    autoexec: Value,
    #[serde(default)]
    dynctx: Value,
}

#[derive(serde::Serialize)]
pub struct TestAutoexecResponse {
    success: bool,
    result: Option<Value>,
    error: Option<String>,
}

async fn test_autoexec(
    State(s): State<AppState>,
    Json(body): Json<TestAutoexecBody>,
) -> Result<Json<TestAutoexecResponse>, AppError> {
    let def: AutoexecDef = serde_json::from_value(body.autoexec).map_err(|e| {
        AppError(
            format!("geçersiz autoexec tanımı: {e}"),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        )
    })?;

    let runner = LiveAutoexecRunner::new(Some(s.pool.clone()));
    let env = ExecEnv {
        wfe_id: Uuid::new_v4(),
        ctx: body.dynctx,
        node: None,
        actor: Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "system".into(),
        },
    };

    let timeout = std::time::Duration::from_secs(def.timeout_seconds as u64);
    match tokio::time::timeout(timeout, runner.run(&def, &env)).await {
        Ok(Ok(result)) => Ok(Json(TestAutoexecResponse {
            success: true,
            result: Some(result),
            error: None,
        })),
        Ok(Err(f)) => Ok(Json(TestAutoexecResponse {
            success: false,
            result: None,
            error: Some(format!("{}: {}", f.error, f.message)),
        })),
        Err(_) => Ok(Json(TestAutoexecResponse {
            success: false,
            result: None,
            error: Some("WFD.Timeout: timeout_seconds aşıldı".into()),
        })),
    }
}
