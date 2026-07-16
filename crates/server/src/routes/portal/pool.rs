//! Portal havuz endpoint'leri (WOR-44 — v2.2 uyumu).
//! Listeleme denormalize current_c_a cache'i ile SQL'de; can-claim/claim
//! kararı engine matcher'ı ile verilir (c_u kuralları dahil), yazım CAS'tır.

use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_pool))
        .route("/:wfe_id/can-claim", get(can_claim))
        .route("/:wfe_id/claim", post(claim))
        .with_state(state)
}

fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

/// One item in the pool list. Bir SQL row'u — `wfd_version` yalnızca
/// `claim_deadline` hesabı için tutulur, response'a serialize edilmez.
#[derive(Debug, sqlx::FromRow)]
struct PoolRow {
    id: Uuid,
    title: String,
    workflow_id: Uuid,
    wfd_version: i32,
    status: String,
    current_node: Option<String>,
    created_at: DateTime<Utc>,
    claimed_by: Option<Value>,
    deadline: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
}

/// Response şekli — SLA görünüm alanları (2026-07-16): `priority` (1-10,
/// otomatik) ve `claim_deadline` (claimed_at + node.claim_timeout).
#[derive(Debug, Serialize)]
pub struct PoolTask {
    pub id: Uuid,
    pub title: String,
    pub workflow_id: Uuid,
    pub status: String,
    pub current_node: Option<String>,
    pub created_at: DateTime<Utc>,
    pub claimed_by: Option<Value>,
    pub deadline: Option<DateTime<Utc>>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub claim_deadline: Option<DateTime<Utc>>,
    pub priority: i32,
}

async fn list_pool(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<PoolTask>>, AppError> {
    // WOR-44: pool = role match OR user match (c_u-resolved candidates,
    // including listable[] grants folded into current_c_a) OR owner match
    // (already claimed by this actor). role/user checks are containment on
    // the denormalized current_c_a cache; owner check is on claimed_by.
    let role_filter = serde_json::json!([{
        "orgu_id": actor.orgu_id.to_string(),
        "role":    actor.role
    }]);
    let user_filter = serde_json::json!([{
        "orgu_id": actor.orgu_id.to_string(),
        "user_id": actor.user_id.to_string()
    }]);
    // c_u can also be a non-UUID identifier (username) — mirror matcher.rs's
    // identity channel by resolving the actor's own ident, if any exists.
    let ident_filter = s
        .executor
        .org
        .user_ident(actor.user_id)
        .await
        .map_err(AppError::from)?
        .map(|ident| {
            serde_json::json!([{
                "orgu_id":    actor.orgu_id.to_string(),
                "user_ident": ident
            }])
        });
    let owner_filter = serde_json::json!({ "user_id": actor.user_id.to_string() });

    let rows = sqlx::query_as::<_, PoolRow>(
        "SELECT e.wfe_id       AS id,
                m.name         AS title,
                e.wfd_id       AS workflow_id,
                e.wfd_version,
                e.status,
                e.current_node,
                e.created_at,
                e.claimed_by,
                e.deadline,
                e.claimed_at
         FROM wf.wfe e
         JOIN wf.wfd_meta m
           ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.status     = 'active'
           AND e.orgtnt_id  = $1
           AND (
                 e.current_c_a @> $2::jsonb
              OR e.current_c_a @> $3::jsonb
              OR ($4::jsonb IS NOT NULL AND e.current_c_a @> $4::jsonb)
              OR e.claimed_by @> $5::jsonb
           )
         ORDER BY e.created_at ASC",
    )
    .bind(actor.orgtnt_id)
    .bind(role_filter)
    .bind(user_filter)
    .bind(ident_filter)
    .bind(owner_filter)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    // priority kolon DEĞİL — okuma anında hesaplanır (2026-07-16 sözleşmesi).
    // Basitlik için Rust'ta hesaplanır ve sıralanır (SQL CASE/window ifadesi
    // yerine): pool boyutu tek aktörle sınırlı, N+1 sıralama maliyeti ihmal
    // edilebilir düzeyde.
    let now = Utc::now();
    let mut wfd_cache: std::collections::HashMap<(Uuid, i32), wfe_core::types::wfd_v22::Wfd> =
        std::collections::HashMap::new();
    let mut tasks = Vec::with_capacity(rows.len());
    for row in rows {
        let priority = wf_wfe::priority::compute_priority(row.created_at, row.deadline, now);
        let claim_deadline = if row.claimed_at.is_some() && row.current_node.is_some() {
            let key = (row.workflow_id, row.wfd_version);
            let wfd = match wfd_cache.get(&key) {
                Some(w) => Some(w),
                None => match s.wfd.fetch(row.workflow_id, row.wfd_version).await {
                    Ok(w) => {
                        wfd_cache.insert(key, w);
                        wfd_cache.get(&key)
                    }
                    Err(_) => None,
                },
            };
            wfd.and_then(|wfd| {
                wf_wfe::executor::compute_claim_deadline(
                    wfd,
                    row.current_node.as_deref(),
                    row.claimed_at,
                )
            })
        } else {
            None
        };
        tasks.push(PoolTask {
            id: row.id,
            title: row.title,
            workflow_id: row.workflow_id,
            status: row.status,
            current_node: row.current_node,
            created_at: row.created_at,
            claimed_by: row.claimed_by,
            deadline: row.deadline,
            claimed_at: row.claimed_at,
            claim_deadline,
            priority,
        });
    }

    // priority DESC, deadline ASC NULLS LAST, created_at ASC (sözleşme sırası).
    tasks.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| match (a.deadline, b.deadline) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    Ok(Json(tasks))
}

#[derive(Serialize)]
struct CanClaimResponse {
    can_claim: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

async fn can_claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<CanClaimResponse>, AppError> {
    let (can_claim, reason) = s
        .executor
        .can_claim(wfe_id, &to_actor(&actor))
        .await
        .map_err(AppError::from)?;
    Ok(Json(CanClaimResponse { can_claim, reason }))
}

#[derive(Serialize)]
struct ClaimResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

async fn claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<ClaimResponse>, AppError> {
    let outcome = s
        .executor
        .claim(wfe_id, &to_actor(&actor))
        .await
        .map_err(AppError::from)?;
    if !outcome.success {
        let status = match outcome.reason.as_deref() {
            Some("already_claimed") => StatusCode::CONFLICT,
            _ => StatusCode::FORBIDDEN,
        };
        return Err(AppError(
            outcome
                .reason
                .unwrap_or_else(|| "Bu görevi almak için yetkiniz yok.".into()),
            status,
        ));
    }
    Ok(Json(ClaimResponse {
        success: true,
        reason: None,
    }))
}
