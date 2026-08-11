//! Tenant permission havuzu — YÖNETİM ağacı (`/org/...`, X-Admin-Key).
//!
//! Havuz tenant'ın KENDİ iş yetkilerini tutar; agnoflow anlamını bilmez (bkz.
//! `docs/superpowers/specs/2026-08-11-tenant-permission-rol-modeli-design.md`).
//! Dış uygulama okuması `routes/ext_permissions.rs` (X-Api-Key), kullanıcının
//! kendi kümesi `routes/portal/permissions.rs` (JWT) — üçü de aynı
//! `repo::permission` fonksiyonlarını çağırır.

use crate::{api_key, error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;
use wf_org::{
    models::{Permission, PermissionException, PermissionRoleUsage, TenantApiKey},
    permission::EffectivePermission,
    repo,
};

/// `/org` router'ına merge edilir (aynı X-Admin-Key kapısının arkasında).
pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_permissions, create_permission))
        .routes(routes!(patch_permission, delete_permission))
        .routes(routes!(permission_roles))
        .routes(routes!(role_permissions, set_role_permissions))
        .routes(routes!(user_permissions))
        .routes(routes!(user_exceptions, set_user_exceptions))
        .routes(routes!(list_api_keys, create_api_key))
        .routes(routes!(revoke_api_key))
        .with_state(state)
}

#[derive(Deserialize, IntoParams)]
struct SearchQuery {
    /// Kod VE görünen ad üzerinde arama.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

// ── Havuz CRUD ──────────────────────────────────────────────────────────────

#[utoipa::path(get, path = "/orgtnt/{id}/permissions", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), SearchQuery),
    responses((status = 200, description = "Yetki havuzu", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_permissions(
    State(s): State<AppState>,
    Path(orgtnt_id): Path<Uuid>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<Permission>>, AppError> {
    repo::permission::list(
        &s.pool,
        orgtnt_id,
        q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        q.limit.unwrap_or(100).clamp(1, 500),
        q.offset.unwrap_or(0).max(0),
    )
    .await
    .map(Json)
    .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct CreatePermissionBody {
    /// Makine kimliği: numara ("1043") ya da isim ("KREDI_ONAY").
    /// ASCII harf/rakam + `. _ : -`, en çok 128 karakter.
    code: String,
    display_name: String,
    description: Option<String>,
}

#[utoipa::path(post, path = "/orgtnt/{id}/permissions", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    request_body = CreatePermissionBody,
    responses(
        (status = 200, description = "Oluşturulan yetki", body = serde_json::Value),
        (status = 400, description = "code biçimi geçersiz (permission.code_format)"),
        (status = 409, description = "code tenant'ta zaten tanımlı (permission.code_conflict)"),
    ),
    security(("x_admin_key" = [])))]
async fn create_permission(
    State(s): State<AppState>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreatePermissionBody>,
) -> Result<Json<Permission>, AppError> {
    repo::permission::create(
        &s.pool,
        orgtnt_id,
        &body.code,
        &body.display_name,
        body.description.as_deref(),
    )
    .await
    .map(Json)
    .map_err(Into::into)
}

/// PATCH: alan gönderilmezse DEĞİŞMEZ, `description` boş string ile TEMİZLENİR
/// (`PATCH /org/orgtnt/{id}` ile aynı semantik).
#[derive(Deserialize, ToSchema)]
struct PatchPermissionBody {
    code: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    is_active: Option<bool>,
}

#[utoipa::path(patch, path = "/orgtnt/{id}/permissions/{pid}", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("pid" = Uuid, Path, description = "Permission id"),
    ),
    request_body = PatchPermissionBody,
    responses((status = 200, description = "Güncellenen yetki", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn patch_permission(
    State(s): State<AppState>,
    Path((orgtnt_id, p_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchPermissionBody>,
) -> Result<Json<Permission>, AppError> {
    repo::permission::patch(
        &s.pool,
        orgtnt_id,
        p_id,
        body.code.as_deref(),
        body.display_name.as_deref(),
        body.description.as_deref(),
        body.is_active,
    )
    .await
    .map(Json)
    .map_err(Into::into)
}

#[utoipa::path(delete, path = "/orgtnt/{id}/permissions/{pid}", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("pid" = Uuid, Path, description = "Permission id"),
    ),
    responses(
        (status = 204, description = "Silindi"),
        (status = 409, description = "Rol/ıskarta referansı var (permission.in_use) — is_active=false kullan"),
    ),
    security(("x_admin_key" = [])))]
async fn delete_permission(
    State(s): State<AppState>,
    Path((orgtnt_id, p_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    repo::permission::delete(&s.pool, orgtnt_id, p_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/orgtnt/{id}/permissions/{pid}/roles", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("pid" = Uuid, Path, description = "Permission id"),
    ),
    responses((status = 200, description = "Yetkiyi taşıyan roller + ulaşılan kullanıcı sayısı",
        body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn permission_roles(
    State(s): State<AppState>,
    Path((orgtnt_id, p_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<PermissionRoleUsage>>, AppError> {
    repo::permission::permission_roles(&s.pool, orgtnt_id, p_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

// ── Rol = permission grubu ──────────────────────────────────────────────────

#[utoipa::path(get, path = "/orgtnt/{id}/roles/{rid}/permissions", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("rid" = Uuid, Path, description = "Rol id"),
    ),
    responses((status = 200, description = "Rolün yetkileri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn role_permissions(
    State(s): State<AppState>,
    Path((orgtnt_id, r_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Permission>>, AppError> {
    repo::permission::role_permissions(&s.pool, orgtnt_id, r_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct PermissionSetBody {
    /// Rolün/kullanıcının TAM kümesi — listede olmayanlar silinir.
    p_ids: Vec<Uuid>,
}

/// KÜME semantiği (PUT): yönetim ekranı "kutucukları işaretle → kaydet" akışıdır.
/// Tek tek POST/DELETE çağrısı iki yöneticinin aynı rolü düzenlemesinde yarım
/// uygulanmış küme bırakırdı; burada diff tek transaction'da uygulanır.
#[utoipa::path(put, path = "/orgtnt/{id}/roles/{rid}/permissions", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("rid" = Uuid, Path, description = "Rol id"),
    ),
    request_body = PermissionSetBody,
    responses(
        (status = 200, description = "Rolün güncel yetkileri", body = serde_json::Value),
        (status = 404, description = "Rol veya yetki bu tenant'ta yok"),
    ),
    security(("x_admin_key" = [])))]
async fn set_role_permissions(
    State(s): State<AppState>,
    Path((orgtnt_id, r_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PermissionSetBody>,
) -> Result<Json<Vec<Permission>>, AppError> {
    repo::permission::set_role_permissions(&s.pool, orgtnt_id, r_id, &body.p_ids)
        .await
        .map(Json)
        .map_err(Into::into)
}

// ── Kullanıcı görünümü ──────────────────────────────────────────────────────

/// Yönetim ekranının kullanıcı sekmesi: etkin küme + neden (`via_roles`) + ıskartalar.
///
/// `ToSchema` türetilmez: içerdiği tipler `wf-org`'da yaşıyor ve o crate utoipa'ya
/// bağımlı değil (org katmanı HTTP'den habersiz kalsın). Belgede gövde
/// `serde_json::Value` olarak geçer — repodaki mevcut desen.
#[derive(Serialize)]
struct UserPermissionsView {
    effective: Vec<EffectivePermission>,
    exceptions: Vec<PermissionException>,
}

#[utoipa::path(get, path = "/users/{id}/permissions", tag = "org",
    params(("id" = Uuid, Path, description = "Kullanıcı id")),
    responses((status = 200, description = "Etkin küme + via_roles + ıskartalar",
        body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn user_permissions(
    State(s): State<AppState>,
    Path(u_id): Path<Uuid>,
) -> Result<Json<UserPermissionsView>, AppError> {
    let orgtnt_id = repo::permission::tenant_of_user(&s.pool, u_id).await?;
    Ok(Json(UserPermissionsView {
        effective: repo::permission::effective_for_user(&s.pool, orgtnt_id, u_id).await?,
        exceptions: repo::permission::user_exceptions(&s.pool, orgtnt_id, u_id).await?,
    }))
}

#[utoipa::path(get, path = "/users/{id}/permission-exceptions", tag = "org",
    params(("id" = Uuid, Path, description = "Kullanıcı id")),
    responses((status = 200, description = "Kişisel ıskartalar", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn user_exceptions(
    State(s): State<AppState>,
    Path(u_id): Path<Uuid>,
) -> Result<Json<Vec<PermissionException>>, AppError> {
    let orgtnt_id = repo::permission::tenant_of_user(&s.pool, u_id).await?;
    repo::permission::user_exceptions(&s.pool, orgtnt_id, u_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// T‑A2: kişisel ıskarta kümesi. Iskarta yetkiyi kullanıcıdan TAMAMEN kaldırır,
/// hangi rolden geldiğine bakmaz.
#[utoipa::path(put, path = "/users/{id}/permission-exceptions", tag = "org",
    params(("id" = Uuid, Path, description = "Kullanıcı id")),
    request_body = PermissionSetBody,
    responses((status = 200, description = "Güncel ıskartalar", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn set_user_exceptions(
    State(s): State<AppState>,
    Path(u_id): Path<Uuid>,
    Json(body): Json<PermissionSetBody>,
) -> Result<Json<Vec<PermissionException>>, AppError> {
    let orgtnt_id = repo::permission::tenant_of_user(&s.pool, u_id).await?;
    repo::permission::set_user_exceptions(&s.pool, orgtnt_id, u_id, &body.p_ids)
        .await
        .map(Json)
        .map_err(Into::into)
}

// ── Tenant API anahtarları (/ext erişimi) ───────────────────────────────────

#[utoipa::path(get, path = "/orgtnt/{id}/api-keys", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    responses((status = 200, description = "Anahtar listesi (sır taşımaz)", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_api_keys(
    State(s): State<AppState>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<TenantApiKey>>, AppError> {
    repo::permission::list_api_keys(&s.pool, orgtnt_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct CreateApiKeyBody {
    /// İnsan okunur ad, ör. "SET entegrasyonu".
    name: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Düz metin anahtar YALNIZ burada, BİR KEZ döner; DB'de sadece SHA-256 özeti durur.
#[utoipa::path(post, path = "/orgtnt/{id}/api-keys", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    request_body = CreateApiKeyBody,
    responses((status = 200, description = "Anahtar kaydı + düz metin (`key`, bir kez gösterilir)",
        body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_api_key(
    State(s): State<AppState>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let generated = api_key::generate();
    let row = repo::permission::create_api_key(
        &s.pool,
        orgtnt_id,
        &body.name,
        &generated.prefix,
        &generated.key_hash,
        body.expires_at,
    )
    .await?;
    Ok(Json(json!({
        "api_key": row,
        "key": generated.plaintext,
        "note": "Bu anahtar bir daha gösterilmez; güvenli bir yere kaydedin."
    })))
}

#[utoipa::path(delete, path = "/orgtnt/{id}/api-keys/{key_id}", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("key_id" = Uuid, Path, description = "Anahtar id"),
    ),
    responses((status = 204, description = "Anahtar kapatıldı")),
    security(("x_admin_key" = [])))]
async fn revoke_api_key(
    State(s): State<AppState>,
    Path((orgtnt_id, key_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    repo::permission::revoke_api_key(&s.pool, orgtnt_id, key_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
