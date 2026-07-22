use super::auth::{require_can_design, require_can_manage_project, AppAuth, MaybeAppAuth};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", post(upload_wfd).get(list_wfd))
        .route("/validate", post(validate_wfd))
        .route("/usage-summary", get(usage_summary))
        .route("/execution-stats", get(execution_stats))
        .route("/unit-workload", get(unit_workload))
        .route("/node-load", get(node_load))
        .route("/aging-executions", get(aging_executions))
        .route("/escalation-forecast", get(escalation_forecast))
        .route("/dashboard-summary", get(dashboard_summary))
        .route("/draft", post(create_draft))
        .route(
            "/draft/:id/:version",
            get(get_draft).put(save_draft).delete(delete_draft),
        )
        .route("/draft/:id/:version/publish", post(publish_draft))
        .route("/draft/:id/:version/submit", post(submit_draft))
        .route("/draft/:id/:version/approve", post(approve_draft))
        .route("/draft/:id/:version/reject", post(reject_draft))
        .route("/:id/:version/meta", patch(update_wfd_meta))
        .route("/:id/:version", get(get_wfd))
        .route("/:id/:version/new-draft", post(new_draft))
        .route("/:id/:version/usage", get(wfe_usage))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery {
    orgtnt_id: Uuid,
    /// Verilirse liste bu projeyle sınırlanır.
    project_id: Option<Uuid>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_wfd(
    State(s): State<AppState>,
    MaybeAppAuth(auth): MaybeAppAuth,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    // Token'lı istekte tenant token'dan doğrulanır; üye yalnız atandığı
    // projelerin akışlarını görür. Token'sız okuma (sim/araçlar) eski davranış.
    if let Some(auth) = &auth {
        if auth.orgtnt_id != q.orgtnt_id {
            return Err(AppError("Tenant uyuşmuyor".into(), StatusCode::FORBIDDEN));
        }
    }
    let mut rows = wf_wfd::repo::list(&s.pool, q.orgtnt_id, q.project_id, limit, offset)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    if let Some(auth) = &auth {
        if auth.role != "admin" {
            let member_of: Vec<Uuid> =
                sqlx::query_scalar("SELECT project_id FROM wf.project_member WHERE user_id = $1")
                    .bind(auth.user_id)
                    .fetch_all(&s.pool)
                    .await
                    .map_err(internal_error)?;
            rows.retain(|w| member_of.contains(&w.project_id));
        }
    }
    Ok(Json(rows))
}

/// Yazma uçlarının ortak kapısı: hedef WFD'nin projesinde tasarım yetkisi.
async fn require_design_on_wfd(
    s: &AppState,
    auth: &AppAuth,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    let meta = wf_wfd::repo::get_meta_any(&s.pool, wfd_id, version)
        .await
        .map_err(map_wfd_err)?;
    if meta.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_design(&s.pool, auth, meta.project_id).await
}

/// Onay/yayın kapısı: tenant admin veya hedef projenin admini.
async fn require_approver_on_wfd(
    s: &AppState,
    auth: &AppAuth,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    let meta = wf_wfd::repo::get_meta_any(&s.pool, wfd_id, version)
        .await
        .map_err(map_wfd_err)?;
    if meta.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_manage_project(&s.pool, auth, meta.project_id).await
}

/// Yeni doküman yaratırken proje çözümü + yetki: body'de proje verilmişse o,
/// verilmemişse tenant'ın varsayılanı. Dönen id adapter'a AYNEN geçilir ki
/// yetki verilen proje ile yazılan proje ayrışamasın.
async fn resolve_project_for_write(
    s: &AppState,
    auth: &AppAuth,
    body_tenant: Uuid,
    project_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if body_tenant != auth.orgtnt_id {
        return Err(AppError("Tenant uyuşmuyor".into(), StatusCode::FORBIDDEN));
    }
    let project_id = match project_id {
        Some(id) => {
            wf_wfd::project::assert_in_tenant(&s.pool, id, auth.orgtnt_id)
                .await
                .map_err(map_wfd_err)?;
            id
        }
        None => wf_wfd::project::resolve_default(&s.pool, auth.orgtnt_id)
            .await
            .map_err(map_wfd_err)?,
    };
    require_can_design(&s.pool, auth, project_id).await?;
    Ok(project_id)
}

#[derive(Deserialize)]
struct UploadBody {
    orgtnt_id: Uuid,
    /// Verilmezse tenant'ın varsayılan projesi kullanılır (eski istemci uyumu).
    #[serde(default)]
    project_id: Option<Uuid>,
    /// v2.2 WFD dokümanı — yükleme kapısı + custom validator uygulanır (M14).
    wfd: Value,
}

async fn upload_wfd(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(body): Json<UploadBody>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_project_for_write(&s, &auth, body.orgtnt_id, body.project_id).await?;
    let (wfd_id, version) = s
        .wfd
        .upload(body.orgtnt_id, Some(project_id), &body.wfd)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    Ok(Json(
        serde_json::json!({ "wfd_id": wfd_id, "version": version }),
    ))
}

/// Editör için: kaydetmeden doğrula — hata/uyarı listesi döner.
async fn validate_wfd(Json(wfd_json): Json<Value>) -> Result<Json<Value>, AppError> {
    let wfd = match wfe_core::types::wfd_v22::Wfd::from_value(wfd_json) {
        Ok(w) => w,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "valid": false,
                "errors": [{"code": "parse", "path": "$", "message": e.to_string()}],
                "warnings": [],
            })))
        }
    };
    let report = wfe_core::validator::validate(&wfd);
    let issue = |i: &wfe_core::validator::ValidationIssue| serde_json::json!({"code": i.code, "path": i.path, "message": i.message});
    Ok(Json(serde_json::json!({
        "valid": report.is_valid(),
        "errors": report.errors.iter().map(issue).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(issue).collect::<Vec<_>>(),
    })))
}

async fn get_wfd(
    State(s): State<AppState>,
    Path((wfd_id, version)): Path<(Uuid, i32)>,
) -> Result<Json<wfe_core::types::wfd_v22::Wfd>, AppError> {
    s.wfd
        .fetch(wfd_id, version)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::NOT_FOUND))
}

#[derive(Deserialize)]
struct CreateDraftBody {
    orgtnt_id: Uuid,
    /// Verilmezse tenant'ın varsayılan projesi kullanılır (eski istemci uyumu).
    #[serde(default)]
    project_id: Option<Uuid>,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    /// Editörün ürettiği başlangıç dokümanı; yoksa engine iskelet yazar.
    #[serde(default)]
    wfd: Option<Value>,
    /// Taslağın türetildiği predefined şablon versiyonu (galeri akışı doldurur).
    #[serde(default)]
    source_template_id: Option<Uuid>,
}

async fn create_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(b): Json<CreateDraftBody>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_project_for_write(&s, &auth, b.orgtnt_id, b.project_id).await?;
    if let Some(tid) = b.source_template_id {
        // İz güvenilir olsun: şablon var ve aynı tenant'ta olmalı.
        let tpl = wf_wfd::template::get(&s.pool, tid)
            .await
            .map_err(map_wfd_err)?;
        if tpl.orgtnt_id != auth.orgtnt_id {
            return Err(AppError("Şablon bulunamadı".into(), StatusCode::NOT_FOUND));
        }
    }
    let (wfd_id, version) = s
        .wfd
        .create_draft(
            b.orgtnt_id,
            Some(project_id),
            &b.name,
            b.description.as_deref(),
            &b.tags,
            b.wfd.as_ref(),
            b.source_template_id,
        )
        .await
        .map_err(map_wfd_err)?;
    Ok(Json(
        serde_json::json!({ "wfd_id": wfd_id, "version": version }),
    ))
}

async fn get_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .fetch_draft_json(id, ver)
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct SaveDraftBody {
    wfd: Value,
    #[serde(default)]
    description: Option<String>,
    /// Verilmezse (None) mevcut tags korunur; boş `[]` gönderilirse temizlenir.
    #[serde(default)]
    tags: Option<Vec<String>>,
}

async fn save_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(b): Json<SaveDraftBody>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .save_draft(id, ver, &b.wfd, b.description.as_deref(), b.tags.as_deref())
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

/// Doğrudan yayın: onaycı (tenant admin | proje admini) VEYA admin'in
/// "doğrudan yayınlayabilir" bayrağını verdiği proje üyesi. Diğerleri
/// /submit ile onaya gönderir.
async fn require_can_publish_wfd(
    s: &AppState,
    auth: &AppAuth,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    if require_approver_on_wfd(s, auth, wfd_id, version)
        .await
        .is_ok()
    {
        return Ok(());
    }
    // Onaycı değil: tasarım yetkisi + kullanıcı bayrağı gerekir.
    require_design_on_wfd(s, auth, wfd_id, version).await?;
    let flag: Option<bool> =
        sqlx::query_scalar("SELECT can_publish FROM wf.app_user WHERE user_id = $1")
            .bind(auth.user_id)
            .fetch_optional(&s.pool)
            .await
            .map_err(internal_error)?;
    if flag == Some(true) {
        return Ok(());
    }
    Err(AppError(
        "Doğrudan yayın yetkiniz yok — taslağı onaya gönderin".into(),
        StatusCode::FORBIDDEN,
    ))
}

async fn publish_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_can_publish_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .publish_draft(id, ver)
        .await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "published" })))
        .map_err(map_wfd_err)
}

/// Taslağı yayın onayına gönderir (tasarım yetkisi yeter). Validator kapısı
/// yayınla AYNIDIR — geçersiz doküman onaya giremez.
async fn submit_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    // Token minimal kimlik taşır — gönderenin görünen adı DB'den çözülür.
    let submitted_by: String =
        sqlx::query_scalar("SELECT display_name FROM wf.app_user WHERE user_id = $1")
            .bind(auth.user_id)
            .fetch_optional(&s.pool)
            .await
            .map_err(internal_error)?
            .unwrap_or_else(|| auth.user_id.to_string());
    s.wfd
        .submit_draft(id, ver, &submitted_by)
        .await
        .map(|_| {
            Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "pending_approval" }))
        })
        .map_err(map_wfd_err)
}

async fn approve_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_approver_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .approve_draft(id, ver)
        .await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "published" })))
        .map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

async fn reject_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(b): Json<RejectBody>,
) -> Result<Json<Value>, AppError> {
    require_approver_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .reject_draft(id, ver, b.reason.as_deref())
        .await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "draft" })))
        .map_err(map_wfd_err)
}

async fn delete_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .delete_draft(id, ver)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

async fn new_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let (wfd_id, version) = s.wfd.new_draft_from(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(
        serde_json::json!({ "wfd_id": wfd_id, "version": version }),
    ))
}

#[derive(Deserialize)]
struct UpdateWfdMetaBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn update_wfd_meta(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(body): Json<UpdateWfdMetaBody>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if body.name.is_some() && name.is_none() {
        return Err(AppError(
            "Workflow adı boş olamaz".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let description = body.description.as_deref();
    wf_wfd::repo::update_group_metadata(&s.pool, id, ver, name, description)
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

/// Bu published versiyonu kullanan WFE örneklerinin durum dağılımı.
/// `active` = anlık çalışan örnek sayısı.
async fn wfe_usage(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, count(*)::bigint FROM wf.wfe \
         WHERE wfd_id = $1 AND wfd_version = $2 GROUP BY status",
    )
    .bind(id)
    .bind(ver)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let (mut active, mut terminal, mut error, mut terminated) = (0i64, 0i64, 0i64, 0i64);
    for (status, n) in rows {
        match status.as_str() {
            "active" => active = n,
            "terminal" => terminal = n,
            "error" => error = n,
            "terminated" => terminated = n,
            _ => {}
        }
    }
    Ok(Json(serde_json::json!({
        "active": active,
        "terminal": terminal,
        "error": error,
        "terminated": terminated,
        "total": active + terminal + error + terminated,
    })))
}

/// Tenant genelinde wfd_id başına anlık aktif WFE sayısı — dashboard özeti için
/// tek istekte tüm sayımları döner (satır başına /usage çağırmaya gerek kalmaz).
async fn usage_summary(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let rows = load_usage_summary(&s.pool, q.orgtnt_id).await?;

    let arr: Vec<Value> = rows
        .into_iter()
        .map(|(wfd_id, active)| serde_json::json!({ "wfd_id": wfd_id, "active": active }))
        .collect();
    Ok(Json(serde_json::json!(arr)))
}

/// Tenant genelinde WFE execution durum dağılımı (dashboard grafiği için).
async fn execution_stats(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let stats = load_execution_stats(&s.pool, q.orgtnt_id).await?;
    Ok(Json(serde_json::json!(stats)))
}

#[derive(Default, serde::Serialize)]
struct ExecutionStatsRow {
    active: i64,
    terminal: i64,
    error: i64,
    /// SLA ihlali sonlanması (2026-07-16) — `terminal`/`error`'dan AYRI bucket.
    terminated: i64,
    total: i64,
}

async fn load_usage_summary(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
) -> Result<Vec<(Uuid, i64)>, AppError> {
    sqlx::query_as(
        "SELECT wfd_id, count(*)::bigint FROM wf.wfe \
         WHERE orgtnt_id = $1 AND status = 'active' GROUP BY wfd_id",
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)
}

async fn load_execution_stats(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
) -> Result<ExecutionStatsRow, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, count(*)::bigint FROM wf.wfe WHERE orgtnt_id = $1 GROUP BY status",
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let (mut active, mut terminal, mut error, mut terminated) = (0i64, 0i64, 0i64, 0i64);
    for (status, n) in rows {
        match status.as_str() {
            "active" => active = n,
            "terminal" => terminal = n,
            "error" => error = n,
            "terminated" => terminated = n,
            _ => {}
        }
    }
    Ok(ExecutionStatsRow {
        active,
        terminal,
        error,
        terminated,
        total: active + terminal + error + terminated,
    })
}

#[derive(serde::Serialize)]
struct UnitWorkloadRow {
    orgu_id: Uuid,
    orgu_name: String,
    active: i64,
    unclaimed: i64,
}

/// Tenant genelinde current_c_a'ya göre birim başına anlık iş yükü — en çok
/// işi olan org unit'ler (dashboard insight).
async fn unit_workload(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<UnitWorkloadRow>>, AppError> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    load_unit_workload(&s.pool, q.orgtnt_id, limit)
        .await
        .map(Json)
}

async fn load_unit_workload(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<UnitWorkloadRow>, AppError> {
    let rows: Vec<(Uuid, String, i64, i64)> = sqlx::query_as(
        "SELECT ou.orgu_id, ou.name,
                count(*)::bigint AS active,
                count(*) FILTER (WHERE w.claimed_by IS NULL)::bigint AS unclaimed
         FROM wf.wfe w
         CROSS JOIN LATERAL jsonb_array_elements(w.current_c_a) AS ca(elem)
         JOIN org.orgu ou ON ou.orgu_id = (ca.elem->>'orgu_id')::uuid
         WHERE w.orgtnt_id = $1 AND w.status = 'active'
         GROUP BY ou.orgu_id, ou.name
         ORDER BY active DESC
         LIMIT $2",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    Ok(rows
        .into_iter()
        .map(|(orgu_id, orgu_name, active, unclaimed)| UnitWorkloadRow {
            orgu_id,
            orgu_name,
            active,
            unclaimed,
        })
        .collect())
}

#[derive(serde::Serialize)]
struct NodeLoadRow {
    wfd_id: Uuid,
    node: String,
    active: i64,
}

/// Tenant genelinde (workflow, node) çifti başına aktif WFE sayısı — hangi
/// duraklarda yığılma var (dashboard insight).
async fn node_load(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<NodeLoadRow>>, AppError> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    load_node_load(&s.pool, q.orgtnt_id, limit).await.map(Json)
}

async fn load_node_load(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<NodeLoadRow>, AppError> {
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT wfd_id, current_node, count(*)::bigint AS active
         FROM wf.wfe
         WHERE orgtnt_id = $1 AND status = 'active' AND current_node IS NOT NULL
         GROUP BY wfd_id, current_node
         ORDER BY active DESC
         LIMIT $2",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    Ok(rows
        .into_iter()
        .map(|(wfd_id, node, active)| NodeLoadRow {
            wfd_id,
            node,
            active,
        })
        .collect())
}

#[derive(serde::Serialize)]
struct AgingRow {
    wfe_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    node: String,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// En uzun süredir güncellenmeyen aktif execution'lar — hareketsiz/"stuck"
/// iş akışları (dashboard insight).
async fn aging_executions(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<AgingRow>>, AppError> {
    let limit = q.limit.unwrap_or(8).clamp(1, 30);
    load_aging_executions(&s.pool, q.orgtnt_id, limit)
        .await
        .map(Json)
}

async fn load_aging_executions(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<AgingRow>, AppError> {
    let rows: Vec<(Uuid, Uuid, i32, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT wfe_id, wfd_id, wfd_version, current_node, updated_at
         FROM wf.wfe
         WHERE orgtnt_id = $1 AND status = 'active' AND current_node IS NOT NULL
         ORDER BY updated_at ASC
         LIMIT $2",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    Ok(rows
        .into_iter()
        .map(|(wfe_id, wfd_id, wfd_version, node, updated_at)| AgingRow {
            wfe_id,
            wfd_id,
            wfd_version,
            node,
            updated_at,
        })
        .collect())
}

#[derive(serde::Serialize)]
struct EscalationRow {
    wfe_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    node: String,
    current_c_a: Value,
    claimed_by: Option<Value>,
    step_idx: usize,
    deadline: chrono::DateTime<chrono::Utc>,
    overdue: bool,
}

/// Yaklaşan/geciken escalation deadline'ları — en yakın vadeden başlayarak
/// (dashboard insight). Node giriş anı gerçek WFAH kaydından hesaplanır
/// (yaklaşık değer değil); bkz. `Engine::next_escalation`.
async fn escalation_forecast(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<EscalationRow>>, AppError> {
    let limit = q.limit.unwrap_or(8).clamp(1, 30);
    load_escalation_forecast(&s, q.orgtnt_id, limit)
        .await
        .map(Json)
}

async fn load_escalation_forecast(
    s: &AppState,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<EscalationRow>, AppError> {
    let now = chrono::Utc::now();
    // Escalation hesabı WFE başına wfah+wfd yükler; taban havuzu makul bir
    // tavanla sınırlanır (en son güncellenen 300 aktif WFE).
    let candidates = wf_wfe::repo::wfe::list_active_by_tenant(&s.pool, orgtnt_id, 300)
        .await
        .map_err(internal_error)?;

    let mut out = Vec::new();
    for row in candidates {
        let forecast = match s.executor.escalation_forecast(row.wfe_id, now).await {
            Ok(f) => f,
            // Bozuk/eksik WFD dokümanı olan tek bir WFE tüm insight'ı düşürmesin.
            Err(_) => continue,
        };
        if let Some(f) = forecast {
            out.push(EscalationRow {
                wfe_id: row.wfe_id,
                wfd_id: row.wfd_id,
                wfd_version: row.wfd_version,
                node: row.current_node.unwrap_or_default(),
                current_c_a: row.current_c_a,
                claimed_by: row.claimed_by,
                step_idx: f.step_idx,
                deadline: f.deadline,
                overdue: f.overdue,
            });
        }
    }
    out.sort_by_key(|r| r.deadline);
    out.truncate(limit as usize);
    Ok(out)
}

#[derive(Default, serde::Serialize)]
struct OrgSummaryRow {
    tree_count: i64,
    unit_count: i64,
    user_count: i64,
    role_count: i64,
    leaf_count: i64,
    max_depth: i64,
    branch_count: i64,
    region_count: i64,
}

#[derive(serde::Serialize)]
struct DashboardSummary {
    wfds: Vec<wf_wfd::models::WfdMeta>,
    active_by_wfd: HashMap<String, i64>,
    exec_stats: ExecutionStatsRow,
    units: Vec<UnitWorkloadRow>,
    node_load_rows: Vec<NodeLoadRow>,
    aging: Vec<AgingRow>,
    escalations: Vec<EscalationRow>,
    org_summary: OrgSummaryRow,
}

async fn dashboard_summary(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<DashboardSummary>, AppError> {
    let wfds = wf_wfd::repo::list(&s.pool, q.orgtnt_id, q.project_id, 1_000, 0)
        .await
        .map_err(map_wfd_err)?;
    let active_by_wfd = load_usage_summary(&s.pool, q.orgtnt_id)
        .await?
        .into_iter()
        .map(|(wfd_id, active)| (wfd_id.to_string(), active))
        .collect();

    Ok(Json(DashboardSummary {
        wfds,
        active_by_wfd,
        exec_stats: load_execution_stats(&s.pool, q.orgtnt_id)
            .await
            .unwrap_or_default(),
        units: load_unit_workload(&s.pool, q.orgtnt_id, 10)
            .await
            .unwrap_or_default(),
        node_load_rows: load_node_load(&s.pool, q.orgtnt_id, 10)
            .await
            .unwrap_or_default(),
        aging: load_aging_executions(&s.pool, q.orgtnt_id, 8)
            .await
            .unwrap_or_default(),
        escalations: load_escalation_forecast(&s, q.orgtnt_id, 8)
            .await
            .unwrap_or_default(),
        org_summary: load_org_summary(&s.pool, q.orgtnt_id)
            .await
            .unwrap_or_default(),
    }))
}

async fn load_org_summary(pool: &sqlx::PgPool, orgtnt_id: Uuid) -> Result<OrgSummaryRow, AppError> {
    let (
        tree_count,
        unit_count,
        user_count,
        role_count,
        leaf_count,
        max_depth,
        branch_count,
        region_count,
    ): (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "WITH primary_tree AS (
             SELECT orgt_id
             FROM org.orgt
             WHERE orgtnt_id = $1
             ORDER BY name
             LIMIT 1
         ),
         tree_units AS (
             SELECT oo.orgu_id, oo.path, o.orgu_type
             FROM org.orgt_orgu oo
             JOIN org.orgu o ON o.orgu_id = oo.orgu_id
             JOIN primary_tree pt ON pt.orgt_id = oo.orgt_id
             WHERE o.is_active = true AND oo.is_active = true
         )
         SELECT
             (SELECT count(*)::bigint FROM org.orgt WHERE orgtnt_id = $1) AS tree_count,
             (SELECT count(*)::bigint FROM tree_units) AS unit_count,
             (SELECT count(*)::bigint FROM org.u WHERE orgtnt_id = $1 AND is_active = true) AS user_count,
             (SELECT count(*)::bigint FROM org.r WHERE orgtnt_id = $1 AND is_active = true) AS role_count,
             (SELECT count(*)::bigint
              FROM tree_units tu
              WHERE NOT EXISTS (
                  SELECT 1 FROM tree_units child
                  WHERE child.path <@ tu.path AND child.path <> tu.path
              )) AS leaf_count,
             COALESCE((SELECT max(nlevel(path))::bigint FROM tree_units), 0) AS max_depth,
             (SELECT count(*)::bigint
              FROM tree_units
              WHERE lower(orgu_type->>'type') IN ('sube', 'branch')) AS branch_count,
             (SELECT count(*)::bigint
              FROM tree_units
              WHERE lower(orgu_type->>'type') IN ('bolge', 'region')) AS region_count",
    )
    .bind(orgtnt_id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(OrgSummaryRow {
        tree_count,
        unit_count,
        user_count,
        role_count,
        leaf_count,
        max_depth,
        branch_count,
        region_count,
    })
}

fn internal_error(e: impl std::fmt::Display) -> AppError {
    AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}

/// WfdError → HTTP kodu eşlemesi.
fn map_wfd_err(e: wf_wfd::error::WfdError) -> AppError {
    use wf_wfd::error::WfdError as E;
    let code = match e {
        E::NotFound(_) => StatusCode::NOT_FOUND,
        E::Conflict(_) => StatusCode::CONFLICT,
        E::InvalidJson(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    AppError(e.to_string(), code)
}
