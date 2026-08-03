//! Autoexec test endpoint'i — editör bir autoexec tanımını gerçek çalıştırır.
//! v2.2: AutoexecDef {type, timeout_seconds, config} formatı.

use utoipa_axum::router::OpenApiRouter;
use crate::{error::AppError, state::AppState};
use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::routes;
use uuid::Uuid;
use wf_wfe::LiveAutoexecRunner;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::AutoexecDef;
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv};

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(test_autoexec))
        .with_state(state)
}

#[derive(Deserialize, ToSchema)]
pub struct TestAutoexecBody {
    /// v2.2 autoexec tanımı: {"type": "rest|sql|calc", "config": {...}, "timeout_seconds"?}
    autoexec: Value,
    #[serde(default)]
    dynctx: Value,
    /// WOR-84: `calc` ifadelerinde `$wfah`/`$prev`/`$first` için örnek geçmiş.
    /// Verilmezse boş — `$prev.*` null okur (patlamaz).
    #[serde(default)]
    #[schema(value_type = Vec<serde_json::Value>)]
    wfah: Wfah,
    /// WOR-84: `$action.input.*` için örnek ACT girdisi.
    #[serde(default)]
    action_input: Option<Value>,
}

#[derive(serde::Serialize, ToSchema)]
pub struct TestAutoexecResponse {
    success: bool,
    result: Option<Value>,
    error: Option<String>,
    /// $-string'leri dynctx ile çözülmüş config — editörde "ne gönderildi" görünümü.
    request_info: Option<Value>,
}

#[utoipa::path(post, path = "/test", tag = "autoexec",
    request_body = TestAutoexecBody,
    responses((status = 200, description = "Autoexec çalıştırma sonucu (success/result/error/request_info)", body = TestAutoexecResponse)))]
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
        wfah: body.wfah,
        action_input: body.action_input,
    };

    let request_info = Some(wf_wfe::runner::resolved_config(&def, &env));
    let timeout = std::time::Duration::from_secs(def.timeout_seconds as u64);
    match tokio::time::timeout(timeout, runner.run(&def, &env)).await {
        Ok(Ok(result)) => Ok(Json(TestAutoexecResponse {
            success: true,
            result: Some(result),
            error: None,
            request_info,
        })),
        Ok(Err(f)) => Ok(Json(TestAutoexecResponse {
            success: false,
            result: None,
            error: Some(format!("{}: {}", f.error, f.message)),
            request_info,
        })),
        Err(_) => Ok(Json(TestAutoexecResponse {
            success: false,
            result: None,
            error: Some("WFD.Timeout: timeout_seconds aşıldı".into()),
            request_info,
        })),
    }
}
