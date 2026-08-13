use crate::error::AppError;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use wf_org::{
    repo,
    traversal::{executor, parser},
};

pub fn router(pool: PgPool) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_orgtnt))
        .routes(routes!(get_orgtnt))
        .routes(routes!(update_orgtnt))
        .routes(routes!(list_orgt_by_tenant, create_orgt))
        .routes(routes!(update_orgt))
        .routes(routes!(set_default_orgt))
        .routes(routes!(list_users_by_tenant, create_user))
        .routes(routes!(update_user))
        .routes(routes!(list_roles_by_tenant, create_role))
        .routes(routes!(update_role))
        .routes(routes!(list_orgu_types, create_orgu_type))
        .routes(routes!(update_orgu_type, delete_orgu_type))
        .routes(routes!(list_actors))
        .routes(routes!(create_assignment, revoke_assignment))
        .routes(routes!(create_orgu_role, delete_orgu_role))
        .routes(routes!(list_delegations, create_delegation_admin))
        .routes(routes!(revoke_delegation_admin))
        .routes(routes!(list_orgu_by_tree, create_orgu))
        .routes(routes!(list_user_orgu))
        .routes(routes!(list_user_roles))
        .routes(routes!(get_orgu, update_orgu, delete_orgu))
        .routes(routes!(traverse_orgu))
        .with_state(pool)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[utoipa::path(get, path = "/orgtnt", tag = "org", params(PageQuery),
    responses((status = 200, description = "Tenant listesi", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_orgtnt(
    State(pool): State<PgPool>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgtnt>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgtnt::list(&pool, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/orgtnt/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    responses((status = 200, description = "Tenant", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn get_orgtnt(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgtnt>, AppError> {
    repo::orgtnt::get(&pool, id)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Tenant kimlik + kurumsal metadata yaması.
///
/// Semantik: **alan gönderilmezse değişmez, boş string gönderilirse temizlenir**
/// (NULL). `name`/`code`/`timezone`/`locale`/`currency` zorunludur — boş string
/// gönderilirse 400. Marka varlıkları (logo/favicon) bu gövdeyle DEĞİL,
/// `/orgtnt/{id}/logo/{slot}` rotalarıyla yönetilir.
#[derive(Deserialize, ToSchema)]
struct OrgtntBody {
    name: Option<String>,
    code: Option<String>,
    display_name: Option<String>,
    /// `#RRGGBB`.
    brand_color: Option<String>,
    legal_name: Option<String>,
    tax_no: Option<String>,
    tax_office: Option<String>,
    contact_email: Option<String>,
    contact_phone: Option<String>,
    website: Option<String>,
    address: Option<String>,
    city: Option<String>,
    /// ISO 3166-1 alpha-2.
    country: Option<String>,
    /// IANA saat dilimi.
    timezone: Option<String>,
    locale: Option<String>,
    /// ISO 4217.
    currency: Option<String>,
    external_id: Option<String>,
    #[schema(value_type = Option<Object>)]
    settings: Option<serde_json::Value>,
    is_active: Option<bool>,
}

impl From<OrgtntBody> for repo::orgtnt::OrgtntPatch {
    fn from(b: OrgtntBody) -> Self {
        Self {
            name: b.name,
            code: b.code,
            display_name: b.display_name,
            brand_color: b.brand_color,
            legal_name: b.legal_name,
            tax_no: b.tax_no,
            tax_office: b.tax_office,
            contact_email: b.contact_email,
            contact_phone: b.contact_phone,
            website: b.website,
            address: b.address,
            city: b.city,
            country: b.country,
            timezone: b.timezone,
            locale: b.locale,
            currency: b.currency,
            external_id: b.external_id,
            settings: b.settings,
            is_active: b.is_active,
        }
    }
}

#[utoipa::path(patch, path = "/orgtnt/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = OrgtntBody,
    responses(
        (status = 200, description = "Güncellenen tenant", body = serde_json::Value),
        (status = 400, description = "Zorunlu alan boş ya da settings object değil"),
    ),
    security(("x_admin_key" = [])))]
async fn update_orgtnt(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    Json(body): Json<OrgtntBody>,
) -> Result<Json<wf_org::models::Orgtnt>, AppError> {
    repo::orgtnt::patch(&pool, id, body.into())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/orgtnt/{id}/orgt", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), PageQuery),
    responses((status = 200, description = "Tenant'ın org ağaçları", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_orgt_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgt>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgt::list_by_tenant(&pool, orgtnt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct OrgtBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(post, path = "/orgtnt/{id}/orgt", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = OrgtBody,
    responses((status = 200, description = "Oluşturulan org ağacı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_orgt(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<OrgtBody>,
) -> Result<Json<wf_org::models::Orgt>, AppError> {
    repo::orgt::create(&pool, orgtnt_id, &body.name, body.description.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(patch, path = "/orgt/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Org ağacı id")), request_body = OrgtBody,
    responses((status = 200, description = "Güncellenen org ağacı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn update_orgt(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
    Json(body): Json<OrgtBody>,
) -> Result<Json<wf_org::models::Orgt>, AppError> {
    repo::orgt::update(&pool, orgt_id, &body.name, body.description.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(post, path = "/orgt/{id}/set-default", tag = "org",
    params(("id" = Uuid, Path, description = "Org ağacı id")),
    responses((status = 200, description = "Varsayılan yapılan org ağacı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn set_default_orgt(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgt>, AppError> {
    let orgtnt_id = repo::orgt::get_orgtnt_id(&pool, orgt_id)
        .await
        .map_err(AppError::from)?;
    repo::orgt::set_default(&pool, orgtnt_id, orgt_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Kullanıcı listesi + ARAMA. `q` verilirse sayfalama yerine arama koşar
/// (`repo::user_role::search_users`): editörün `c_u` tamamlaması tüm tenant listesini
/// indirmek zorunda kalmasın — on binlerce kullanıcıda o yol taşar.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct UserSearchQuery {
    /// Arama metni: kullanıcı adı, tam ad, e-posta ya da UUID. Boş/eksikse düz liste.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[utoipa::path(get, path = "/orgtnt/{id}/users", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), UserSearchQuery),
    responses((status = 200, description = "Tenant kullanıcıları", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_users_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<UserSearchQuery>,
) -> Result<Json<Vec<wf_org::models::User>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    // Yalnız BOŞLUK olan `q` arama değildir — "hepsini getir"e düşer.
    let query = page.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    match query {
        Some(q) => repo::user_role::search_users(&pool, orgtnt_id, q, limit).await,
        None => repo::user_role::list_users(&pool, orgtnt_id, limit, offset).await,
    }
    .map(Json)
    .map_err(Into::into)
}

#[utoipa::path(get, path = "/orgtnt/{id}/roles", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), PageQuery),
    responses((status = 200, description = "Tenant rolleri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_roles_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Role>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_roles(&pool, orgtnt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Simülasyon aktör listesi: tenant'taki tüm (kullanıcı, birim, rol) atamaları.
/// Aktör switcher bundan beslenir — her satır bir X-Actor (orgu+user+role) demektir.
#[derive(Serialize, FromRow, ToSchema)]
struct ActorRow {
    user_id: Uuid,
    full_name: String,
    username: String,
    email: Option<String>,
    orgu_id: Uuid,
    orgu_name: String,
    role: String,
}

const ACTOR_SELECT: &str = "SELECT u.u_id AS user_id, u.full_name, u.username, u.email,
            o.orgu_id, o.name AS orgu_name, r.name AS role
     FROM org.ur ur
     JOIN org.u u  ON ur.u_id = u.u_id
     JOIN org.orgu o ON ur.orgu_id = o.orgu_id
     JOIN org.r r  ON ur.r_id = r.r_id
     WHERE ur.orgtnt_id = $1
       AND ur.ur_type <> 'excluded'
       AND u.is_active = true AND r.is_active = true";

#[utoipa::path(get, path = "/orgtnt/{id}/actors", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    responses((status = 200, description = "Simülasyon aktör listesi", body = Vec<ActorRow>)),
    security(("x_admin_key" = [])))]
async fn list_actors(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<ActorRow>>, AppError> {
    sqlx::query_as::<_, ActorRow>(&format!(
        "{ACTOR_SELECT} ORDER BY o.name, u.full_name, r.name"
    ))
    .bind(orgtnt_id)
    .fetch_all(&pool)
    .await
    .map(Json)
    .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))
}

/// Sim playground: yeni kullanıcı ekle.
#[derive(Deserialize, ToSchema)]
#[schema(as = OrgCreateUserBody)]
struct CreateUserBody {
    username: String,
    full_name: String,
    email: Option<String>,
}

#[utoipa::path(post,
    operation_id = "org_create_user", path = "/orgtnt/{id}/users", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = CreateUserBody,
    responses((status = 200, description = "Oluşturulan kullanıcı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_user(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreateUserBody>,
) -> Result<Json<wf_org::models::User>, AppError> {
    repo::user_role::create_user(
        &pool,
        orgtnt_id,
        &body.username,
        &body.full_name,
        body.email.as_deref(),
    )
    .await
    .map(Json)
    .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
#[schema(as = OrgUpdateUserBody)]
struct UpdateUserBody {
    full_name: String,
}

#[utoipa::path(patch,
    operation_id = "org_update_user", path = "/users/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Kullanıcı id")), request_body = UpdateUserBody,
    responses((status = 200, description = "Güncellenen kullanıcı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn update_user(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Json(body): Json<UpdateUserBody>,
) -> Result<Json<wf_org::models::User>, AppError> {
    let full_name = body.full_name.trim();
    if full_name.is_empty() {
        return Err(AppError(
            "full_name boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::update_user(&pool, user_id, full_name)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Yeni rol ekle veya aynı isimli pasif rolü yeniden aktifleştir.
#[derive(Deserialize, ToSchema)]
struct CreateRoleBody {
    name: String,
    display_name: Option<String>,
}

#[utoipa::path(post, path = "/orgtnt/{id}/roles", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = CreateRoleBody,
    responses((status = 200, description = "Oluşturulan rol", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreateRoleBody>,
) -> Result<Json<wf_org::models::Role>, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError(
            "rol adı boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::create_role(&pool, orgtnt_id, name, body.display_name.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Rol adını/görünen adını günceller. Kullanıcı atamaları r_id üzerinden kaldığı
/// için değişiklik rolün geçtiği tüm aktör listelerine yansır.
#[derive(Deserialize, ToSchema)]
struct UpdateRoleBody {
    name: String,
    display_name: String,
}

#[utoipa::path(patch, path = "/orgtnt/{id}/roles/{rid}", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), ("rid" = Uuid, Path, description = "Rol id")),
    request_body = UpdateRoleBody,
    responses((status = 200, description = "Güncellenen rol", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn update_role(
    State(pool): State<PgPool>,
    Path((orgtnt_id, r_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateRoleBody>,
) -> Result<Json<wf_org::models::Role>, AppError> {
    let name = body.name.trim();
    let display_name = body.display_name.trim();
    if name.is_empty() || display_name.is_empty() {
        return Err(AppError(
            "rol adı ve görünen ad boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::update_role(&pool, orgtnt_id, r_id, name, display_name)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct CreateOrguTypeBody {
    key: String,
    display_name: String,
}

#[utoipa::path(get, path = "/orgtnt/{id}/orgu-types", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    responses((status = 200, description = "Org birimi tipi kataloğu", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_orgu_types(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::models::OrguTypeDef>>, AppError> {
    repo::orgu_type::list(&pool, orgtnt_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(post, path = "/orgtnt/{id}/orgu-types", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = CreateOrguTypeBody,
    responses((status = 200, description = "Oluşturulan/reaktifleştirilen tip", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_orgu_type(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreateOrguTypeBody>,
) -> Result<Json<wf_org::models::OrguTypeDef>, AppError> {
    repo::orgu_type::create(&pool, orgtnt_id, &body.key, &body.display_name)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(patch, path = "/orgtnt/{id}/orgu-types/{type_id}", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), ("type_id" = Uuid, Path, description = "Tip id")),
    request_body = CreateOrguTypeBody,
    responses((status = 200, description = "Güncellenen tip", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn update_orgu_type(
    State(pool): State<PgPool>,
    Path((orgtnt_id, type_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CreateOrguTypeBody>,
) -> Result<Json<wf_org::models::OrguTypeDef>, AppError> {
    repo::orgu_type::update(&pool, orgtnt_id, type_id, &body.key, &body.display_name)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(delete, path = "/orgtnt/{id}/orgu-types/{type_id}", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), ("type_id" = Uuid, Path, description = "Tip id")),
    responses((status = 204, description = "Tip pasifleştirildi")),
    security(("x_admin_key" = [])))]
async fn delete_orgu_type(
    State(pool): State<PgPool>,
    Path((orgtnt_id, type_id)): Path<(Uuid, Uuid)>,
) -> Result<axum::http::StatusCode, AppError> {
    let removed = repo::orgu_type::deactivate(&pool, orgtnt_id, type_id)
        .await
        .map_err(AppError::from)?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "orgu tipi bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}

/// Sim playground: (kullanıcı, birim, rol) atamasını garantiler → dönüşte hazır aktör satırı.
/// Kritere uygun aktör yoksa UI bununla bir aktör üretir.
#[derive(Deserialize, ToSchema)]
struct AssignBody {
    u_id: Uuid,
    orgu_id: Uuid,
    role_name: String,
}

#[utoipa::path(post, path = "/orgtnt/{id}/assignments", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = AssignBody,
    responses((status = 200, description = "Hazır aktör satırı", body = ActorRow)),
    security(("x_admin_key" = [])))]
async fn create_assignment(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<AssignBody>,
) -> Result<Json<ActorRow>, AppError> {
    repo::user_role::grant_assignment(&pool, orgtnt_id, body.u_id, body.orgu_id, &body.role_name)
        .await
        .map_err(AppError::from)?;

    sqlx::query_as::<_, ActorRow>(&format!(
        "{ACTOR_SELECT} AND u.u_id = $2 AND o.orgu_id = $3 AND r.name = $4 LIMIT 1"
    ))
    .bind(orgtnt_id)
    .bind(body.u_id)
    .bind(body.orgu_id)
    .bind(&body.role_name)
    .fetch_one(&pool)
    .await
    .map(Json)
    .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))
}

/// Atama kaldırma: (u_id, orgu_id, role_name) query paramlarıyla granted rolü siler.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct RevokeAssignQuery {
    u_id: Uuid,
    orgu_id: Uuid,
    role_name: String,
}

#[utoipa::path(delete, path = "/orgtnt/{id}/assignments", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), RevokeAssignQuery),
    responses((status = 204, description = "Atama kaldırıldı")),
    security(("x_admin_key" = [])))]
async fn revoke_assignment(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(q): Query<RevokeAssignQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let removed =
        repo::user_role::revoke_assignment(&pool, orgtnt_id, q.u_id, q.orgu_id, &q.role_name)
            .await
            .map_err(AppError::from)?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "atama bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}

/// Orgu'ya rol grant'ı: birimdeki tüm kullanıcılar bu rolü devralır (check_user_role).
#[derive(Deserialize, ToSchema)]
struct OrguRoleBody {
    orgu_id: Uuid,
    role_name: String,
}

#[utoipa::path(post, path = "/orgtnt/{id}/orgu-roles", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = OrguRoleBody,
    responses((status = 204, description = "Birim-rol grant'ı eklendi")),
    security(("x_admin_key" = [])))]
async fn create_orgu_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<OrguRoleBody>,
) -> Result<axum::http::StatusCode, AppError> {
    let role_name = body.role_name.trim();
    if role_name.is_empty() {
        return Err(AppError(
            "rol adı boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::grant_orgu_role(&pool, orgtnt_id, body.orgu_id, role_name)
        .await
        .map_err(AppError::from)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Orgu rol grant'ını kaldırma: (orgu_id, role_name) query paramlarıyla.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct RevokeOrguRoleQuery {
    orgu_id: Uuid,
    role_name: String,
}

#[utoipa::path(delete, path = "/orgtnt/{id}/orgu-roles", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), RevokeOrguRoleQuery),
    responses((status = 204, description = "Birim-rol grant'ı kaldırıldı")),
    security(("x_admin_key" = [])))]
async fn delete_orgu_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(q): Query<RevokeOrguRoleQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let removed = repo::user_role::revoke_orgu_role(&pool, orgtnt_id, q.orgu_id, &q.role_name)
        .await
        .map_err(AppError::from)?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "birim-rol bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}

#[utoipa::path(get, path = "/orgt/{id}/orgu", tag = "org",
    params(("id" = Uuid, Path, description = "Org ağacı id"), PageQuery),
    responses((status = 200, description = "Ağaçtaki org birimleri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_orgu_by_tree(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgu::list_by_tree(&pool, orgt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct CreateOrguBody {
    name: String,
    type_key: String,
    #[serde(default)]
    parent_orgu_id: Option<Uuid>,
}

#[utoipa::path(post, path = "/orgt/{id}/orgu", tag = "org",
    params(("id" = Uuid, Path, description = "Org ağacı id")), request_body = CreateOrguBody,
    responses((status = 200, description = "Oluşturulan org birimi", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_orgu(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
    Json(body): Json<CreateOrguBody>,
) -> Result<Json<wf_org::models::Orgu>, AppError> {
    let orgtnt_id = repo::orgt::get_orgtnt_id(&pool, orgt_id)
        .await
        .map_err(AppError::from)?;
    repo::orgu_type::require_active(&pool, orgtnt_id, &body.type_key)
        .await
        .map_err(AppError::from)?;
    let created = repo::orgu::create(
        &pool,
        orgtnt_id,
        orgt_id,
        body.parent_orgu_id,
        &body.name,
        &body.type_key,
    )
    .await
    .map_err(AppError::from)?;
    // Yeni birim, `*:[type:x]` / `self,children` gibi çözülmüş grant kümelerini
    // eskitir — görünürlük projeksiyonu yeniden üretilmeli (asenkron kuyruk).
    crate::visibility_queue::enqueue(&pool, orgtnt_id, "orgu.create").await;
    Ok(Json(created))
}

#[utoipa::path(get, path = "/users/{id}/orgu", tag = "org",
    params(("id" = Uuid, Path, description = "Kullanıcı id"), PageQuery),
    responses((status = 200, description = "Kullanıcının org birimleri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_user_orgu(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::UserOrgu>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_user_orgus(&pool, user_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/users/{id}/roles", tag = "org",
    params(("id" = Uuid, Path, description = "Kullanıcı id"), PageQuery),
    responses((status = 200, description = "Kullanıcının rolleri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_user_roles(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::UserRole>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_user_roles(&pool, user_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(get, path = "/orgu/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Org birimi id")),
    responses((status = 200, description = "Org birimi", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn get_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgu>, AppError> {
    repo::orgu::get(&pool, orgu_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize, ToSchema)]
struct UpdateOrguBody {
    name: String,
    type_key: String,
}

#[utoipa::path(patch, path = "/orgu/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Org birimi id")), request_body = UpdateOrguBody,
    responses((status = 200, description = "Güncellenen org birimi", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn update_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
    Json(body): Json<UpdateOrguBody>,
) -> Result<Json<wf_org::models::Orgu>, AppError> {
    let orgtnt_id = repo::orgu::get_orgtnt_id(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;
    repo::orgu_type::require_active(&pool, orgtnt_id, &body.type_key)
        .await
        .map_err(AppError::from)?;
    let updated = repo::orgu::update(&pool, orgu_id, &body.name, &body.type_key)
        .await
        .map_err(AppError::from)?;
    // Tip değişimi `*:[type:x]` filtrelerinin sonucunu değiştirir.
    crate::visibility_queue::enqueue(&pool, orgtnt_id, "orgu.update").await;
    Ok(Json(updated))
}

#[derive(Serialize, ToSchema)]
struct DeleteOrguResponse {
    deactivated_count: i64,
}

#[utoipa::path(delete, path = "/orgu/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Org birimi id")),
    responses((status = 200, description = "Pasifleştirilen birim sayısı", body = DeleteOrguResponse)),
    security(("x_admin_key" = [])))]
async fn delete_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
) -> Result<Json<DeleteOrguResponse>, AppError> {
    // Tenant kimliği SİLMEDEN ÖNCE çözülür — cascade sonrası satır pasif olur.
    let orgtnt_id = repo::orgu::get_orgtnt_id(&pool, orgu_id).await.ok();
    let deactivated_count = repo::orgu::delete_cascade(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;
    if let Some(orgtnt_id) = orgtnt_id {
        crate::visibility_queue::enqueue(&pool, orgtnt_id, "orgu.deactivate").await;
    }
    Ok(Json(DeleteOrguResponse { deactivated_count }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TraverseQuery {
    expr: String,
}

#[utoipa::path(get, path = "/orgu/{id}/traverse", tag = "org",
    params(("id" = Uuid, Path, description = "Org birimi id"), TraverseQuery),
    responses((status = 200, description = "Traversal sonucu org birimleri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn traverse_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
    Query(q): Query<TraverseQuery>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    let orgt_id = repo::orgu::get_orgt_id(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;
    let orgtnt_id = repo::orgu::get_orgtnt_id(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;

    let expr = normalize_traverse_expr(&q.expr);
    let pipeline = parser::parse(&expr)
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::BAD_REQUEST))?;

    let result = executor::execute(&pool, orgu_id, orgt_id, orgtnt_id, &pipeline)
        .await
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(result))
}

fn normalize_traverse_expr(expr: &str) -> String {
    let expr = expr.trim();
    // Global tip selektörü (*:[...]) anchor'dan bağımsızdır — "self." ile sarma.
    if expr == "self" || expr.starts_with("self.") || expr.starts_with("*:") {
        expr.to_string()
    } else {
        format!("self.{expr}")
    }
}

// ── Madde 6: vekalet/delegasyon (admin yönetim; /org X-Admin-Key kapısı altında) ──

#[utoipa::path(get, path = "/orgtnt/{id}/delegations", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    responses((status = 200, description = "Tenant vekaletleri", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn list_delegations(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::models::Delegation>>, AppError> {
    repo::delegation::list_by_tenant(&pool, orgtnt_id)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize, ToSchema)]
struct SeatBody {
    orgu_id: Uuid,
    role: String,
}

#[derive(Deserialize, ToSchema)]
struct AdminDelegationBody {
    delegator_user_id: Uuid,
    #[serde(default)]
    seat: Option<SeatBody>,
    #[serde(default)]
    all: bool,
    #[schema(value_type = Object)]
    grantee: serde_json::Value,
    #[serde(default)]
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: chrono::DateTime<chrono::Utc>,
}

#[utoipa::path(post, path = "/orgtnt/{id}/delegations", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = AdminDelegationBody,
    responses((status = 200, description = "Oluşturulan vekaletler", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_delegation_admin(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<AdminDelegationBody>,
) -> Result<Json<Vec<wf_org::models::Delegation>>, AppError> {
    let seat = body.seat.map(|s| (s.orgu_id, s.role));
    let created = crate::routes::delegation::create_delegations(
        &pool,
        orgtnt_id,
        body.delegator_user_id,
        seat,
        body.all,
        &body.grantee,
        body.valid_from,
        body.valid_to,
        body.delegator_user_id, // admin yolunda created_by = delegator
    )
    .await?;
    Ok(Json(created))
}

#[utoipa::path(delete, path = "/orgtnt/{id}/delegations/{did}", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id"), ("did" = Uuid, Path, description = "Vekalet id")),
    responses((status = 204, description = "Vekalet kaldırıldı")),
    security(("x_admin_key" = [])))]
async fn revoke_delegation_admin(
    State(pool): State<PgPool>,
    Path((orgtnt_id, did)): Path<(Uuid, Uuid)>,
) -> Result<axum::http::StatusCode, AppError> {
    let ok = repo::delegation::revoke(&pool, did, orgtnt_id)
        .await
        .map_err(AppError::from)?;
    if ok {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "vekalet bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}
