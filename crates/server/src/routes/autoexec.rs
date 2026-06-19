use axum::{
    routing::post,
    Json, Router,
    extract::State,
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;
use crate::{error::AppError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/test", post(test_autoexec))
        .with_state(state)
}

#[derive(Deserialize)]
pub struct TestAutoexecBody {
    kind: String,
    params: serde_json::Value,
    dynctx: serde_json::Value,
}

#[derive(serde::Serialize)]
pub struct TestAutoexecResponse {
    success: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
    request_info: Option<serde_json::Value>,
}

async fn test_autoexec(
    State(_s): State<AppState>,
    Json(body): Json<TestAutoexecBody>,
) -> Result<Json<TestAutoexecResponse>, AppError> {
    use wf_wfe::{AutoexecExecutor, RestExecutor, SqlExecutor, CalcExecutor};
    use wf_wfe::autoexec::AutoexecContext;
    use wfe_core::types::wfd::AutoexecDef;

    let executor = AutoexecExecutor::new(
        RestExecutor::new(),
        SqlExecutor::new(None),
        CalcExecutor::new(),
    );

    let ctx = AutoexecContext {
        wfe_id: Uuid::new_v4(),
        dynctx: body.dynctx.clone(),
        params: serde_json::json!({}),
    };

    let def = AutoexecDef {
        kind: body.kind.clone(),
        params: body.params.clone(),
    };

    match executor.execute(&def, &ctx).await {
        Ok(result) => {
            Ok(Json(TestAutoexecResponse {
                success: true,
                result: Some(result),
                error: None,
                request_info: Some(serde_json::json!({
                    "type": body.kind,
                    "context_fields": body.dynctx
                })),
            }))
        }
        Err(e) => {
            Ok(Json(TestAutoexecResponse {
                success: false,
                result: None,
                error: Some(e.to_string()),
                request_info: Some(serde_json::json!({
                    "type": body.kind,
                    "context_fields": body.dynctx
                })),
            }))
        }
    }
}
