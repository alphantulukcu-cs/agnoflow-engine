//! Tenant permission verisinin DIŞ UYGULAMA yüzeyi (`/ext`, X-Api-Key, SALT OKUMA).
//!
//! Bu ağaç `/org` altında OLAMAZ: `main.rs` tüm `/org` (ve `/db`) ağacını tek bir
//! X-Admin-Key middleware'inin arkasına koyuyor, oraya eklenen bir yola X-Api-Key
//! ile erişilemez. Ayrı üst ağaç olması aynı zamanda yetki sınırını YAPI gereği
//! kılar: burada yazma rotası yoktur.
//!
//! Aktör `api_key::ApiKeyActor` — TEK tenant'a bağlı. Kapsam dışı kullanıcı `404`
//! döner (varlığı da sızmaz).

use crate::{api_key::ApiKeyActor, error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;
use wf_org::{
    permission::{check_codes, CheckResult, EffectivePermission},
    repo,
};

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(check))
        .routes(routes!(user_permissions))
        .with_state(state)
}

#[derive(Deserialize, ToSchema)]
struct CheckBody {
    /// `u_id` ya da `username` — biri zorunlu.
    u_id: Option<Uuid>,
    username: Option<String>,
    /// Sorulan kodlar. Karşılaştırma büyük/küçük harf duyarsızdır.
    codes: Vec<String>,
}

/// Toplu kontrol: dış uygulama tek ekranda onlarca yetki sorar, tek tek uç N+1
/// üretirdi.
///
/// `unknown` TEŞHİStir, yetki cevabı değil — havuzda hiç olmayan kod hem `denied`da
/// hem `unknown`da görünür. Bilinmeyen kodu hata saymıyoruz (tenant henüz
/// tanımlamamış olabilir) ama sessiz de bırakmıyoruz: aksi halde dış uygulamadaki
/// yazım hatası "yetki yok" gibi okunurdu.
#[utoipa::path(post, path = "/permissions/check", tag = "ext",
    request_body = CheckBody,
    responses(
        (status = 200, description = "granted / denied / unknown", body = serde_json::Value),
        (status = 400, description = "u_id veya username verilmedi"),
        (status = 401, description = "X-Api-Key geçersiz (api_key.invalid)"),
        (status = 404, description = "Kullanıcı bu tenant'ta yok"),
    ),
    security(("x_api_key" = [])))]
async fn check(
    State(s): State<AppState>,
    actor: ApiKeyActor,
    Json(body): Json<CheckBody>,
) -> Result<Json<CheckResult>, AppError> {
    let u_id = resolve_user(&s, &actor, body.u_id, body.username.as_deref()).await?;
    let effective = repo::permission::effective_for_user(&s.pool, actor.orgtnt_id, u_id).await?;
    // Katalog yalnız SORULAN kodlar için çekilir — `unknown` teşhisi varlık sorusudur,
    // binlerce satırlık havuzu indirmeyi gerektirmez.
    let catalog =
        repo::permission::catalog_by_codes(&s.pool, actor.orgtnt_id, &body.codes).await?;
    Ok(Json(check_codes(&effective, &catalog, &body.codes)))
}

/// Etkin küme. `/org/users/{id}/permissions` ile aynı kümeyi döner; fark kapı ve
/// içeriktir — burada ıskarta listesi YOK, dış uygulamaya iç yönetim detayı sızmaz.
#[utoipa::path(get, path = "/permissions/user/{u_id}", tag = "ext",
    params(("u_id" = Uuid, Path, description = "Kullanıcı id")),
    responses(
        (status = 200, description = "Etkin yetkiler + via_roles", body = serde_json::Value),
        (status = 401, description = "X-Api-Key geçersiz (api_key.invalid)"),
        (status = 404, description = "Kullanıcı bu tenant'ta yok"),
    ),
    security(("x_api_key" = [])))]
async fn user_permissions(
    State(s): State<AppState>,
    actor: ApiKeyActor,
    Path(u_id): Path<Uuid>,
) -> Result<Json<Vec<EffectivePermission>>, AppError> {
    let u_id = resolve_user(&s, &actor, Some(u_id), None).await?;
    repo::permission::effective_for_user(&s.pool, actor.orgtnt_id, u_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Kullanıcıyı çözer VE anahtarın tenant'ına bağlar. Bağlanmazsa bir tenant'ın
/// anahtarı başka tenant'ın kullanıcısını sorgulayabilirdi.
async fn resolve_user(
    s: &AppState,
    actor: &ApiKeyActor,
    u_id: Option<Uuid>,
    username: Option<&str>,
) -> Result<Uuid, AppError> {
    match (u_id, username) {
        (Some(id), _) => {
            let owner = repo::permission::tenant_of_user(&s.pool, id).await?;
            if owner != actor.orgtnt_id {
                // Kapsam dışı = 404: başka tenant'ta VAR olduğu bilgisi de sızmaz.
                return Err(AppError(
                    "not found: user".into(),
                    axum::http::StatusCode::NOT_FOUND,
                ));
            }
            Ok(id)
        }
        (None, Some(name)) => {
            repo::permission::user_id_by_username(&s.pool, actor.orgtnt_id, name.trim())
                .await
                .map_err(Into::into)
        }
        (None, None) => Err(AppError(
            "u_id veya username verilmeli".into(),
            axum::http::StatusCode::BAD_REQUEST,
        )),
    }
}
