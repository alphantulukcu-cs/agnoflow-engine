//! Portal WFD endpoint'leri — başlatılabilir WFD listesi + başlatma (v2.2).

use utoipa_axum::router::OpenApiRouter;
use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfah::Wfah;
use wfe_core::v22::matcher::{authorize, MatchEnv};
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_wfds))
        .routes(routes!(start_wfd))
        .with_state(state)
}

#[derive(Debug, sqlx::FromRow)]
struct WfdMetaRow {
    wfd_id: Uuid,
    name: String,
    version: i32,
}

#[derive(Debug, Serialize, ToSchema)]
struct WfdListItem {
    id: Uuid,
    name: String,
    version: i32,
}

#[utoipa::path(get, path = "/", tag = "portal",
    responses((status = 200, description = "Aktörün başlatabileceği published WFD listesi", body = Vec<WfdListItem>)),
    security(("bearer_jwt" = [])))]
async fn list_wfds(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<WfdListItem>>, AppError> {
    let metas = sqlx::query_as::<_, WfdMetaRow>(
        "SELECT DISTINCT ON (name) wfd_id, name, version
         FROM wf.wfd_meta
         WHERE orgtnt_id = $1 AND is_active = true AND status = 'published'
         ORDER BY name, version DESC",
    )
    .bind(actor.orgtnt_id)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let portal_actor = Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    };
    let empty_ctx = serde_json::json!({});
    let empty_wfah = Wfah::empty();

    let mut result = Vec::new();
    for meta in metas {
        // Eski formatta kalmış WFD'ler v2.2 kapısını geçemez — listeden düşer
        let Ok(wfd) = s.wfd.fetch(meta.wfd_id, meta.version).await else {
            continue;
        };

        let mut can_start = false;
        for rule in &wfd.start {
            // Simetrik start: initiator yetkisi `from` node'unun c_a'sında.
            let Some(node) = wfd.nodes.get(&rule.from) else {
                continue;
            };
            let env = MatchEnv {
                ctx: &empty_ctx,
                wfah: &empty_wfah,
                orgtnt_id: actor.orgtnt_id,
            };
            if authorize(&node.c_a, &portal_actor, env, &*s.executor.org)
                .await
                .unwrap_or(false)
            {
                can_start = true;
                break;
            }
        }

        if can_start {
            result.push(WfdListItem {
                id: meta.wfd_id,
                name: meta.name,
                version: meta.version,
            });
        }
    }

    Ok(Json(result))
}

#[derive(Deserialize, ToSchema)]
struct StartRequest {
    /// M16: verilirse yalnız bu action adını taşıyan start kuralları aday olur.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    initial_context: Value,
    /// SLA-3 (2026-07-16): opsiyonel "Tamamlanma süresi" — ISO 8601 duration,
    /// start anından itibaren. WFD `timeout` tavanına tabidir.
    #[serde(default)]
    deadline: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct StartResponse {
    wfe_id: Uuid,
    /// Varılan node — anahtar + gösterim (`{id, label}`); terminalde `None`.
    #[schema(value_type = Object)]
    current_node: Option<wf_wfe::executor::Ref>,
}

#[utoipa::path(post, path = "/{wfd_id}/start", tag = "portal",
    params(("wfd_id" = Uuid, Path, description = "WFD id")),
    request_body = StartRequest,
    responses((status = 200, description = "Başlatılan WFE", body = StartResponse)),
    security(("bearer_jwt" = [])))]
async fn start_wfd(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfd_id): Path<Uuid>,
    Json(body): Json<StartRequest>,
) -> Result<Json<StartResponse>, AppError> {
    let version: i32 = sqlx::query_scalar(
        "SELECT version FROM wf.wfd_meta
         WHERE wfd_id = $1 AND is_active = true AND status = 'published' AND orgtnt_id = $2
         ORDER BY version DESC LIMIT 1",
    )
    .bind(wfd_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFD bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    let portal_actor = Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    };

    // Portal ortam adı geçirmez: tenant'ın varsayılan ortamı kullanılır.
    let result = s
        .executor
        .start(
            wfd_id,
            version,
            &portal_actor,
            body.action.as_deref(),
            &body.initial_context,
            body.deadline.as_deref(),
        )
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?;

    Ok(Json(StartResponse {
        wfe_id: result.wfe_id,
        current_node: result.current_node,
    }))
}
