use utoipa_axum::router::OpenApiRouter;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::v22::ports::{BranchState, BranchStatus, WfdStore};

/// Ortam ADINI tenant'ın kayıtlı ortamlarına çözer. Bilinmeyen ad 422 — serbest metin
/// kabul edilmez, bir tipo sessizce yeni bir ortam yaratmaz. `None` = tenant varsayılanı
/// (executor tarafında çözülür).
async fn resolve_environment_id(
    s: &AppState,
    actor: &Actor,
    name: Option<&str>,
) -> Result<Option<uuid::Uuid>, AppError> {
    let Some(name) = name else { return Ok(None) };
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    wf_wfe::repo::env::resolve_environment(&s.pool, orgtnt_id, Some(name))
        .await
        .map(|e| Some(e.id))
        .map_err(|e| {
            AppError(
                e.to_string(),
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            )
        })
}

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(start_wfe, list_wfe))
        .routes(routes!(apply_action))
        .routes(routes!(query_wfe))
        .routes(routes!(claim_wfe))
        .routes(routes!(reassign_wfe))
        .routes(routes!(possible_actions))
        .merge(super::attachments::routes())
        .with_state(state)
}

pub(crate) fn extract_actor(headers: &HeaderMap) -> Result<Actor, AppError> {
    let orgu_id = parse_uuid_header(headers, "x-actor-orgu")?;
    let user_id = parse_uuid_header(headers, "x-actor-user")?;
    let role = headers
        .get("x-actor-role")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            AppError(
                "X-Actor-Role header required".into(),
                StatusCode::BAD_REQUEST,
            )
        })?
        .to_string();
    Ok(Actor {
        orgu_id,
        user_id,
        role,
    })
}

fn parse_uuid_header(headers: &HeaderMap, name: &str) -> Result<Uuid, AppError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            AppError(
                format!("{name} header required (UUID)"),
                StatusCode::BAD_REQUEST,
            )
        })
}

#[derive(Deserialize, ToSchema)]
struct StartBody {
    wfd_id: Uuid,
    version: i32,
    /// M16: start aksiyonları gerçek ad taşır — verilirse yalnız bu action adını
    /// taşıyan start kuralları aday olur; verilmezse tüm start kuralları denenir.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    input: Value,
    /// SLA-3 (2026-07-16): opsiyonel ISO 8601 duration, start anından itibaren.
    /// WFD `timeout` tanımlıysa tavan olarak uygulanır (aşarsa InvalidInput).
    #[serde(default)]
    deadline: Option<String>,
    /// Koşum ortamı ADI (`test` | `prod` | ...). Verilmezse tenant'ın varsayılanı.
    /// Örnek ömrü boyunca SABİTLENİR. Çağıran yalnız adı seçer, değerleri değil.
    #[serde(default)]
    environment: Option<String>,
}

#[utoipa::path(post, path = "/", tag = "wfe",
    request_body = StartBody,
    responses((status = 200, description = "Başlatılan WFE (WfeStartResult)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn start_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StartBody>,
) -> Result<Json<wf_wfe::executor::WfeStartResult>, AppError> {
    let actor = extract_actor(&headers)?;
    let environment_id = resolve_environment_id(&s, &actor, body.environment.as_deref()).await?;
    s.executor
        .start_in(
            body.wfd_id,
            body.version,
            &actor,
            body.action.as_deref(),
            &body.input,
            body.deadline.as_deref(),
            environment_id,
        )
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize, ToSchema)]
struct ApplyBody {
    action: String,
    #[serde(default)]
    input: Value,
    /// WOR-31 T4: paralel modda kol seçimi — action ≥2 aktif kolun
    /// transition'ıyla eşleşiyorsa zorunlu (aksi halde `AmbiguousAction`/409).
    #[serde(default)]
    node: Option<String>,
    /// WOR-65: istemcinin okuduğu WFE revizyon token'ı (`GET /wfe/:id` →`rev`,
    /// `GET /wfe` → satır başına `rev`). OPSİYONEL — göndermeyen istemci bugünkü
    /// davranışı görür. Verilirse ve durum bu arada ilerlemişse hiçbir şey
    /// uygulanmaz: 409 + `code: "conflict.stale_revision"`.
    ///
    /// Neden gövde alanı, `If-Match` başlığı değil: token opak bir entity-tag
    /// değil, düz bir tamsayıdır; `If-Match`'in weak/strong karşılaştırma, `*`
    /// ve liste semantiğinin yarısını uygulamak yanıltıcı olurdu. Ayrıca bu
    /// endpoint'in gövdesinde zaten opsiyonel alanlar var (`node`) — token da
    /// aynı yerde, aynı tipte, tek bir sözleşmede durur.
    #[serde(default)]
    expected_rev: Option<u32>,
}

#[utoipa::path(post, path = "/{id}/actions", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ApplyBody,
    responses((status = 200, description = "Uygulanan aksiyon sonucu (WfeApplyResult)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn apply_action(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<wf_wfe::executor::WfeApplyResult>, AppError> {
    let actor = extract_actor(&headers)?;

    // Attachment gate: hedef node'un `required` dosyaları yüklenmeden engine'e HİÇ
    // gitmeyiz (server-side zorlama; UI-only gating'e güvenilmez). Engine core
    // dosyadan habersiz kalır — kontrol portal katmanı opendal store'undadır.
    let wfd = super::attachments::load_wfd(&s, wfe_id).await?;
    let has_attachments = wfd.nodes.values().any(|n| !n.attachments.is_empty());
    if has_attachments {
        let view = s
            .executor
            .query(wfe_id, &actor)
            .await
            .map_err(AppError::from)?;
        // Paralelde current_node None'dur; kol seçimi body.node ile gelir.
        let target_node = body.node.clone().or(view.current_node.clone());
        if let Some(node) = &target_node {
            let groups =
                crate::attachments::status_for_node(&s.attachments, &wfd, wfe_id, node)
                    .await
                    .map_err(|e| {
                        AppError(
                            format!("attachment durum sorgusu başarısız: {e}"),
                            StatusCode::INTERNAL_SERVER_ERROR,
                        )
                    })?;
            let missing = crate::attachments::missing_required(&groups);
            if !missing.is_empty() {
                return Err(AppError {
                    message: format!("Eksik zorunlu belgeler: {}", missing.join(", ")),
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: Some("attachment.missing"),
                });
            }
        }
    }

    s.executor
        .apply(
            wfe_id,
            &actor,
            &body.action,
            &body.input,
            body.node.as_deref(),
            body.expected_rev,
        )
        .await
        .map(Json)
        .map_err(AppError::from)
}

/// WOR-31 T4: gövde opsiyonel — hiç body/`{}` gönderilirse `node = None` (eski
/// davranış), `{"node": "..."}` verilirse paralel kol ipucu. Body zorunlu bir
/// `Json<T>` extractor'ı OLMADIĞI için (geriye uyumluluk: eski istemciler hiç
/// gövde göndermez) ham `Bytes` üzerinden opsiyonel ayrıştırılır.
#[derive(Deserialize, Default, ToSchema)]
#[schema(as = WfeClaimBody)]
struct ClaimBody {
    #[serde(default)]
    node: Option<String>,
    /// WOR-65: opsiyonel revizyon token'ı (bkz. `ApplyBody::expected_rev`).
    /// Claim'in kendi CAS'ı yarışı zaten çözer; bu kapı "listede gördüğüm satır
    /// hâlâ geçerli mi" sorusunu yanıtlar. Göndermeyen istemci için claim akışı
    /// HİÇ DEĞİŞMEZ (200 + `{success:false, reason:"already_claimed"}`).
    #[serde(default)]
    expected_rev: Option<u32>,
}

fn parse_claim_body(bytes: &axum::body::Bytes) -> Result<ClaimBody, AppError> {
    if bytes.is_empty() {
        return Ok(ClaimBody::default());
    }
    serde_json::from_slice(bytes)
        .map_err(|e| AppError(format!("invalid claim body: {e}"), StatusCode::BAD_REQUEST))
}

#[utoipa::path(post, path = "/{id}/claim", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ClaimBody,
    responses((status = 200, description = "Claim sonucu (ClaimOutcome)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn claim_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Json<wf_wfe::executor::ClaimOutcome>, AppError> {
    let actor = extract_actor(&headers)?;
    let claim_body = parse_claim_body(&body)?;
    s.executor
        .claim(
            wfe_id,
            &actor,
            claim_body.node.as_deref(),
            claim_body.expected_rev,
        )
        .await
        .map(Json)
        .map_err(AppError::from)
}

/// Madde 7: yetkili claim devri. `to` = devralacak tam aktör üçlüsü; `null`/atlanırsa
/// havuza bırakma (force-unclaim). `node` = WOR-31 paralel modda kol seçimi.
#[derive(Deserialize, ToSchema)]
struct ReassignBody {
    #[serde(default)]
    to: Option<TargetActor>,
    #[serde(default)]
    node: Option<String>,
}

#[derive(Deserialize, ToSchema)]
struct TargetActor {
    orgu_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[utoipa::path(post, path = "/{id}/reassign", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ReassignBody,
    responses((status = 200, description = "Devir sonucu (ReassignOutcome)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn reassign_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ReassignBody>,
) -> Result<Json<wf_wfe::executor::ReassignOutcome>, AppError> {
    let reassigner = extract_actor(&headers)?;
    let target = body.to.map(|t| Actor {
        orgu_id: t.orgu_id,
        user_id: t.user_id,
        role: t.role,
    });
    s.executor
        .reassign(wfe_id, &reassigner, target.as_ref(), body.node.as_deref())
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[utoipa::path(get, path = "/{id}", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "WFE görünümü (WfeView)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn query_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<wf_wfe::executor::WfeView>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[utoipa::path(get, path = "/{id}/possible-actions", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "Aktörün uygulayabileceği aksiyonlar", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn possible_actions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<Vec<wf_wfe::executor::PossibleAction>>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .possible_actions(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WfeListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `WfeRow` + SLA görünüm alanları (2026-07-16): `priority` (1-10, otomatik) ve
/// `claim_deadline` (claimed_at + node.claim_timeout, hesaplanmış). `deadline` /
/// `claimed_at` / `join_target` zaten `WfeRow` kolonları — flatten ile response'a
/// dahil olur.
#[derive(serde::Serialize)]
struct WfeListItem {
    #[serde(flatten)]
    row: wf_wfe::models::WfeRow,
    priority: i32,
    claim_deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// WOR-65: WFE revizyon token'ı. Listede AÇIKÇA döner çünkü buraya `wfah`
    /// dizisi dahil değildir — portal'ın revizyonu türetebileceği başka bir alan
    /// YOK. `priority`/`claim_deadline` gibi hesaplanmış bir alandır (`wf.wfe`
    /// kolonu DEĞİL), bu yüzden `WfeRow`'a değil buraya konur.
    rev: i32,
    /// WOR-31 T4: paralel modda bu WFE'nin AKTİF kolları (`[{node, status,
    /// claimed_by, claimed_at, entered_at}]`, `GET /wfe/:id` ile aynı şekil).
    /// Paralel değilken (join_target NULL) BOŞ dizi — liste tüketicisi kol-başına
    /// satır fan-out'u için bunu okur (current_node paralel modda NULL'dır).
    branches: Vec<BranchState>,
}

/// WOR-31 T4: liste kol satırı (`BranchListRow`) → API görünümü (`BranchState`,
/// `node` alan adıyla serialize olur). `claimed_by` jsonb `{"user_id": "<uuid>"}`
/// biçiminden Uuid'e çözülür (wfe_adapter `parse_claimed_by` ile aynı sözleşme);
/// `status` metni enum'a eşlenir (aktif dışı zaten sorguda süzülür).
fn branch_list_row_to_state(r: wf_wfe::models::BranchListRow) -> BranchState {
    BranchState {
        // WOR-73: liste sorgusu kol kimliğini çekmiyor (havuz görünümü kimliği
        // kullanmaz) — boş bırakılır, `entry_or_current()` branch_node'a düşer.
        entry_node: String::new(),
        branch_node: r.branch_node,
        status: match r.status.as_str() {
            "arrived" => BranchStatus::Arrived,
            "cancelled" => BranchStatus::Cancelled,
            _ => BranchStatus::Active,
        },
        claimed_by: r
            .claimed_by
            .as_ref()
            .and_then(|cb| cb.get("user_id"))
            .and_then(|u| u.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        claimed_at: r.claimed_at,
        entered_at: r.entered_at,
    }
}

#[utoipa::path(get, path = "/", tag = "wfe", params(WfeListQuery),
    responses((status = 200, description = "Tenant'ın WFE listesi (WfeListItem[])", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn list_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<WfeListQuery>,
) -> Result<Json<Vec<WfeListItem>>, AppError> {
    let actor = extract_actor(&headers)?;
    // WOR-5 fix: orgu_id tenant DEĞİLDİR — orgtnt_id org katmanından çözülür
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = wf_wfe::repo::wfe::list_by_tenant(&s.pool, orgtnt_id, limit, offset)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    // WOR-65: revizyonlar TEK toplu sorguda (satır başına sorgu YOK).
    let wfe_ids: Vec<Uuid> = rows.iter().map(|r| r.wfe_id).collect();
    let revs = wf_wfe::repo::wfah::max_seq_by_wfe(&s.pool, &wfe_ids)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    // WOR-31 T4: paralel WFE'lerin aktif kolları — TEK toplu sorgu, `wfe_id`'ye
    // göre grupla. Yalnız `join_target` dolu (paralel) satırları sorgula: tek-kol
    // WFE'ler için gereksiz yük olmasın.
    let parallel_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.join_target.is_some())
        .map(|r| r.wfe_id)
        .collect();
    let mut branches_by_wfe: std::collections::HashMap<Uuid, Vec<BranchState>> =
        std::collections::HashMap::new();
    for br in wf_wfe::repo::branch::load_active_for_wfes(&s.pool, &parallel_ids)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    {
        branches_by_wfe
            .entry(br.wfe_id)
            .or_default()
            .push(branch_list_row_to_state(br));
    }

    let now = chrono::Utc::now();
    // (wfd_id, version) immutable — aynı sürüm birden çok satırda paylaşılıyorsa
    // fetch tekrarlanmaz.
    let mut wfd_cache: std::collections::HashMap<(Uuid, i32), wfe_core::types::wfd_v22::Wfd> =
        std::collections::HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let priority = wf_wfe::priority::compute_priority(row.created_at, row.deadline, now);
        let claim_deadline = if row.claimed_at.is_some() && row.current_node.is_some() {
            let key = (row.wfd_id, row.wfd_version);
            let wfd = match wfd_cache.get(&key) {
                Some(w) => Some(w),
                None => match s.wfd.fetch(row.wfd_id, row.wfd_version).await {
                    Ok(w) => {
                        wfd_cache.insert(key, w);
                        wfd_cache.get(&key)
                    }
                    // Bozuk/silinmiş tek bir WFD tüm listeyi düşürmesin.
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
        let rev = revs.get(&row.wfe_id).copied().unwrap_or(0);
        let branches = branches_by_wfe.remove(&row.wfe_id).unwrap_or_default();
        out.push(WfeListItem {
            priority,
            claim_deadline,
            rev,
            branches,
            row,
        });
    }
    Ok(Json(out))
}
