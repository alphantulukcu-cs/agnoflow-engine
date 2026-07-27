// Proje uçları — tamamı auth ister.
// list: tenant admin hepsini, üye yalnız atandıklarını görür.
// create: yalnız tenant admin. update: tenant admin ya da o projenin admin'i.

use utoipa_axum::router::OpenApiRouter;
use super::auth::{require_can_design, require_can_manage_project, AppAuth};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::routes;
use uuid::Uuid;
use wf_wfd::models::Project;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_projects, create_project))
        .routes(routes!(get_project, update_project))
        .routes(routes!(list_members))
        .with_state(state)
}

#[derive(serde::Serialize, ToSchema)]
struct MemberRow {
    user_id: Uuid,
    display_name: String,
    email: String,
    role: String,
}

/// Projenin üyeleri — görünürlük seçimi için; proje admini de görebilir.
#[utoipa::path(get, path = "/{id}/members", tag = "project",
    params(("id" = Uuid, Path, description = "Proje id")),
    responses((status = 200, description = "Proje üyeleri", body = Vec<MemberRow>)),
    security(("bearer_jwt" = [])))]
async fn list_members(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MemberRow>>, AppError> {
    let project = wf_wfd::project::get(&s.pool, id)
        .await
        .map_err(map_wfd_err)?;
    if project.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Proje bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_manage_project(&s.pool, &auth, id).await?;
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT u.user_id, u.display_name, u.email, m.role
         FROM wf.project_member m JOIN wf.app_user u USING (user_id)
         WHERE m.project_id = $1 AND u.is_active = true ORDER BY u.display_name",
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(
        rows.into_iter()
            .map(|(user_id, display_name, email, role)| MemberRow {
                user_id,
                display_name,
                email,
                role,
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/", tag = "project",
    responses((status = 200, description = "Görünür projeler", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn list_projects(
    State(s): State<AppState>,
    auth: AppAuth,
) -> Result<Json<Vec<Project>>, AppError> {
    let all = wf_wfd::project::list(&s.pool, auth.orgtnt_id)
        .await
        .map_err(map_wfd_err)?;
    if auth.role == "admin" {
        return Ok(Json(all));
    }
    let member_of: Vec<Uuid> =
        sqlx::query_scalar("SELECT project_id FROM wf.project_member WHERE user_id = $1")
            .bind(auth.user_id)
            .fetch_all(&s.pool)
            .await
            .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(
        all.into_iter()
            .filter(|p| member_of.contains(&p.project_id))
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
#[schema(as = ProjectCreateBody)]
struct CreateBody {
    /// Geriye uyum için kabul edilir ama token'daki tenant esas alınır.
    #[serde(default)]
    orgtnt_id: Option<Uuid>,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(post, path = "/", tag = "project",
    request_body = CreateBody,
    responses((status = 201, description = "Oluşturulan proje", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn create_project(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(b): Json<CreateBody>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    auth.require_admin()?;
    if let Some(body_tenant) = b.orgtnt_id {
        if body_tenant != auth.orgtnt_id {
            return Err(AppError("Tenant uyuşmuyor".into(), StatusCode::FORBIDDEN));
        }
    }
    let name = b.name.trim();
    if name.is_empty() {
        return Err(AppError(
            "Proje adı boş olamaz".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    wf_wfd::project::create(&s.pool, auth.orgtnt_id, name, b.description.as_deref())
        .await
        .map(|p| (StatusCode::CREATED, Json(p)))
        .map_err(map_wfd_err)
}

#[utoipa::path(get, path = "/{id}", tag = "project",
    params(("id" = Uuid, Path, description = "Proje id")),
    responses((status = 200, description = "Proje", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn get_project(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Project>, AppError> {
    let project = wf_wfd::project::get(&s.pool, id)
        .await
        .map_err(map_wfd_err)?;
    if project.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Proje bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_design(&s.pool, &auth, id).await?;
    Ok(Json(project))
}

#[derive(Deserialize, ToSchema)]
struct UpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(patch, path = "/{id}", tag = "project",
    params(("id" = Uuid, Path, description = "Proje id")), request_body = UpdateBody,
    responses((status = 200, description = "Güncel proje", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn update_project(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateBody>,
) -> Result<Json<Project>, AppError> {
    let project = wf_wfd::project::get(&s.pool, id)
        .await
        .map_err(map_wfd_err)?;
    if project.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Proje bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_manage_project(&s.pool, &auth, id).await?;
    let name = b.name.as_deref().map(str::trim).filter(|v| !v.is_empty());
    if b.name.is_some() && name.is_none() {
        return Err(AppError(
            "Proje adı boş olamaz".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    wf_wfd::project::update(&s.pool, id, name, b.description.as_deref())
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

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
