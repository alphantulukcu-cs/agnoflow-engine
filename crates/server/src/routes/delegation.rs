//! Madde 6: vekalet/delegasyon CRUD.
//!
//! Governance (onaylı): hem self-service (kişi kendi koltuğunu delege eder) hem de
//! amir/admin başkası adına (X-Admin-Key). Aktör X-Actor-User / X-Actor-Orgu
//! header'larından çözülür; orgtnt actor'ün orgu'sundan türetilir. Koltuk sahipliği
//! (delegator seat_role'ü gerçekten taşıyor mu) grant öncesi doğrulanır.

use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wf_org::repo::delegation;
use wfe_core::types::wfd_v22::CandidateActor;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", post(create_delegation).get(list_mine))
        .route("/:id", axum::routing::delete(revoke_delegation))
        .with_state(state)
}

fn parse_uuid_header(headers: &HeaderMap, name: &str) -> Result<Uuid, AppError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| AppError(format!("{name} header required (UUID)"), StatusCode::BAD_REQUEST))
}

/// X-Admin-Key mevcut VE cfg.admin_api_key ile eşleşiyor mu (amir/admin yolu).
fn is_admin(headers: &HeaderMap, s: &AppState) -> bool {
    match (&s.cfg.admin_api_key, headers.get("x-admin-key").and_then(|v| v.to_str().ok())) {
        (Some(expected), Some(got)) => expected == got,
        _ => false,
    }
}

#[derive(Deserialize)]
struct Seat {
    orgu_id: Uuid,
    role: String,
}

#[derive(Deserialize)]
struct CreateBody {
    /// Amir/admin başkası adına verirken; atlanırsa self-service (delegator = actor).
    #[serde(default)]
    delegator_user_id: Option<Uuid>,
    /// Tek koltuk. `all=true` ile bir arada verilmez (biri zorunlu).
    #[serde(default)]
    seat: Option<Seat>,
    /// "Tüm koltuklarım" kısayolu — delegator'ın mevcut (orgu,rol) üyeliklerine genişler.
    #[serde(default)]
    all: bool,
    /// Alıcı kuralı (CandidateActor): kişi {c_orgu, c_u:[...]} veya havuz {c_orgu, c_r:[...]}.
    grantee: Value,
    #[serde(default)]
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: chrono::DateTime<chrono::Utc>,
}

async fn create_delegation(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<Json<Vec<wf_org::models::Delegation>>, AppError> {
    let actor_user = parse_uuid_header(&headers, "x-actor-user")?;
    let actor_orgu = parse_uuid_header(&headers, "x-actor-orgu")?;
    let orgtnt_id = s.executor.org.orgtnt_for_orgu(actor_orgu).await.map_err(AppError::from)?;
    let admin = is_admin(&headers, &s);

    // delegator = self (default) veya admin ise gövdedeki kullanıcı.
    let delegator = body.delegator_user_id.unwrap_or(actor_user);
    if delegator != actor_user && !admin {
        return Err(AppError(
            "başkası adına vekalet için X-Admin-Key gerekir".into(),
            StatusCode::FORBIDDEN,
        ));
    }

    let seat = body.seat.map(|s| (s.orgu_id, s.role));
    let created = create_delegations(
        &s.pool,
        orgtnt_id,
        delegator,
        seat,
        body.all,
        &body.grantee,
        body.valid_from,
        body.valid_to,
        actor_user, // created_by = işlemi yapan
    )
    .await?;
    Ok(Json(created))
}

/// Vekalet oluşturmanın ortak çekirdeği — hem self-service (`/delegation`) hem de
/// admin yönetim ekranı (`/org/orgtnt/:id/delegations`) bunu kullanır. Koltuk(lar)ı
/// çözer, sahipliği doğrular, grantee'yi parse eder ve satır(lar)ı yazar.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_delegations(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    delegator: Uuid,
    seat: Option<(Uuid, String)>,
    all: bool,
    grantee: &Value,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: chrono::DateTime<chrono::Utc>,
    created_by: Uuid,
) -> Result<Vec<wf_org::models::Delegation>, AppError> {
    // grantee geçerli bir CandidateActor mı? (auth anında parse hatası olmasın)
    serde_json::from_value::<CandidateActor>(grantee.clone())
        .map_err(|e| AppError(format!("grantee geçersiz CandidateActor: {e}"), StatusCode::BAD_REQUEST))?;

    let valid_from = valid_from.unwrap_or_else(chrono::Utc::now);
    if valid_to <= valid_from {
        return Err(AppError("valid_to, valid_from'dan sonra olmalı".into(), StatusCode::BAD_REQUEST));
    }

    // Delege edilecek koltuk(lar).
    let seats: Vec<(Uuid, String)> = if all {
        let roles = wf_org::repo::user_role::list_user_roles(pool, delegator, 1000, 0)
            .await
            .map_err(AppError::from)?;
        let mut v: Vec<(Uuid, String)> = roles
            .into_iter()
            .filter_map(|ur| ur.orgu_id.map(|o| (o, ur.role_name)))
            .collect();
        v.sort();
        v.dedup();
        if v.is_empty() {
            return Err(AppError("delegator'ın delege edilebilir koltuğu yok".into(), StatusCode::BAD_REQUEST));
        }
        v
    } else {
        let (orgu_id, role) =
            seat.ok_or_else(|| AppError("seat veya all zorunlu".into(), StatusCode::BAD_REQUEST))?;
        vec![(orgu_id, role)]
    };

    let mut created = Vec::with_capacity(seats.len());
    for (seat_orgu_id, seat_role) in seats {
        // Koltuk sahipliği: delegator bu koltuğu gerçekten taşımalı.
        if !delegation::delegator_holds_seat(pool, delegator, seat_orgu_id, &seat_role)
            .await
            .map_err(AppError::from)?
        {
            return Err(AppError(
                format!("delegator '{seat_role}' rolünü {seat_orgu_id} orgu'sunda taşımıyor"),
                StatusCode::BAD_REQUEST,
            ));
        }
        let d = delegation::create(
            pool,
            orgtnt_id,
            delegator,
            seat_orgu_id,
            &seat_role,
            grantee,
            valid_from,
            valid_to,
            created_by,
        )
        .await
        .map_err(AppError::from)?;
        created.push(d);
    }
    Ok(created)
}

async fn list_mine(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<wf_org::models::Delegation>>, AppError> {
    let actor_user = parse_uuid_header(&headers, "x-actor-user")?;
    let rows = delegation::list_by_delegator(&s.pool, actor_user)
        .await
        .map_err(AppError::from)?;
    Ok(Json(rows))
}

async fn revoke_delegation(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let actor_user = parse_uuid_header(&headers, "x-actor-user")?;
    let actor_orgu = parse_uuid_header(&headers, "x-actor-orgu")?;
    let orgtnt_id = s.executor.org.orgtnt_for_orgu(actor_orgu).await.map_err(AppError::from)?;
    let admin = is_admin(&headers, &s);

    let d = delegation::get_by_id(&s.pool, id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError("vekalet bulunamadı".into(), StatusCode::NOT_FOUND))?;
    // Yalnız veren (delegator) veya admin iptal edebilir; tenant sınırı içinde.
    if d.orgtnt_id != orgtnt_id || (d.delegator_user_id != actor_user && !admin) {
        return Err(AppError("bu vekaleti iptal etme yetkiniz yok".into(), StatusCode::FORBIDDEN));
    }
    delegation::revoke(&s.pool, id, orgtnt_id).await.map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}
