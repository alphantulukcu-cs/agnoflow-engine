use utoipa_axum::router::OpenApiRouter;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
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
        .routes(routes!(reserve_wfe))
        .routes(routes!(apply_action))
        .routes(routes!(query_wfe))
        .routes(routes!(claim_wfe))
        .routes(routes!(reassign_wfe))
        .routes(routes!(fire_escalation))
        .routes(routes!(skip_escalation))
        .routes(routes!(possible_actions))
        .merge(super::attachments::routes())
        .merge(super::notes::routes())
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
    /// `POST /wfe/reserve` ile alınmış wfe_id (2026-08-07). Başlatma aksiyonu ek-belge
    /// istiyorsa ZORUNLUDUR: dosyalar bu id'nin altına önceden yüklenir, burada kapı
    /// kontrol edilir ve WFE ancak belgeler tamsa O id ile oluşturulur. Verilmezse
    /// bugünkü davranış (id start'ta üretilir) — belge isteyen bir başlatmada 422.
    #[serde(default)]
    wfe_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
struct ReserveBody {
    wfd_id: Uuid,
    version: i32,
    #[serde(default)]
    environment: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct ReserveResult {
    /// Başlatmada geri gönderilecek id. Dosyalar `PUT /wfe/{wfe_id}/attachments/...`
    /// ile bu id'nin altına yüklenir.
    wfe_id: Uuid,
}

/// Başlatma öncesi belge yüklemesi için wfe_id rezerve eder. DB'de WFE satırı OLUŞMAZ —
/// yalnız `wf.wfe_reservation` defterine bir kayıt düşer (bkz. crate::reservation).
/// Başlatılmayan rezervasyonlar süpürücü tarafından dosyalarıyla birlikte silinir.
#[utoipa::path(post, path = "/reserve", tag = "wfe",
    request_body = ReserveBody,
    responses((status = 200, description = "Rezerve edilen wfe_id", body = ReserveResult)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn reserve_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReserveBody>,
) -> Result<Json<ReserveResult>, AppError> {
    let actor = extract_actor(&headers)?;
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    // WFD gerçekten var mı (ve bu sürüm çekilebiliyor mu) — rezervasyon uydurma bir
    // dokümana bağlanmasın; yükleme rotası katalog doğrulamasını buradan yapacak.
    s.wfd
        .fetch(body.wfd_id, body.version)
        .await
        .map_err(AppError::from)?;
    let environment_id = resolve_environment_id(&s, &actor, body.environment.as_deref()).await?;

    let reservation = crate::reservation::Reservation {
        wfe_id: Uuid::new_v4(),
        orgtnt_id,
        wfd_id: body.wfd_id,
        wfd_version: body.version,
        environment_id,
        actor_orgu_id: actor.orgu_id,
        actor_user_id: actor.user_id,
    };
    crate::reservation::create(&s.pool, &reservation).await?;
    Ok(Json(ReserveResult {
        wfe_id: reservation.wfe_id,
    }))
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
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;

    // Rezervasyon: dosyalar bu id'nin altına ÖNCEDEN yüklendi. Ortam da rezervasyonla
    // sabitlendi — yükleme hangi depoya yapıldıysa kapı da orada aranmalı, gövdeden
    // gelen farklı bir ortam adı dosyaları başka bir bucket'ta aratırdı.
    let reservation = match body.wfe_id {
        Some(id) => {
            let r = crate::reservation::get(&s.pool, id).await?.ok_or_else(|| {
                AppError(
                    "rezervasyon bulunamadı (süresi dolmuş olabilir)".into(),
                    StatusCode::NOT_FOUND,
                )
            })?;
            if !crate::reservation::owned_by(&r, orgtnt_id, &actor) {
                return Err(AppError(
                    "bu rezervasyon size ait değil".into(),
                    StatusCode::FORBIDDEN,
                ));
            }
            if r.wfd_id != body.wfd_id || r.wfd_version != body.version {
                return Err(AppError(
                    "rezervasyon başka bir WFD/versiyon için alınmış".into(),
                    StatusCode::UNPROCESSABLE_ENTITY,
                ));
            }
            Some(r)
        }
        None => None,
    };
    let environment_id = match &reservation {
        Some(r) => r.environment_id,
        None => resolve_environment_id(&s, &actor, body.environment.as_deref()).await?,
    };

    // Başlatma aksiyonunun ek-belge kapısı. Engine'e GİTMEDEN kontrol edilir: eksikse
    // WFE hiç oluşmaz (rezervasyon durur, kullanıcı eksiği yükleyip tekrar dener).
    let wfd = s
        .wfd
        .fetch(body.wfd_id, body.version)
        .await
        .map_err(AppError::from)?;
    if let Some((node, action)) = start_gate_target(&wfd, body.action.as_deref()) {
        let gated = wfd
            .nodes
            .get(&node)
            .map(|n| n.attachments.iter().any(|a| a.gates_action(Some(&action))))
            .unwrap_or(false);
        if gated {
            // Kapı var ama id rezerve edilmemiş: dosyaların yükleneceği bir anahtar
            // yoktu, yani belgeler olamaz. Sessizce başlatmak kuralı delerdi.
            let Some(r) = &reservation else {
                return Err(AppError {
                    message: "bu akış başlatma için belge istiyor: önce POST /wfe/reserve ile wfe_id alıp belgeleri yükleyin".into(),
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: Some("attachment.reservation_required"),
                });
            };
            let store =
                crate::attachment_store::store_for_wfd(&s, body.wfd_id, orgtnt_id, environment_id)
                    .await?;
            let groups =
                crate::attachments::status_for_node(&store, &wfd, r.wfe_id, &node, Some(&action))
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

    let result = s
        .executor
        .start_reserved(
            body.wfd_id,
            body.version,
            &actor,
            body.action.as_deref(),
            &body.input,
            body.deadline.as_deref(),
            environment_id,
            reservation.as_ref().map(|r| r.wfe_id),
        )
        .await
        .map_err(AppError::from)?;

    // WFE artık gerçek — defter kaydı silinir, dosyalar zaten nihai anahtarında.
    if let Some(r) = &reservation {
        crate::reservation::delete(&s.pool, r.wfe_id).await?;
    }
    Ok(Json(result))
}

/// Kapının uygulanacağı (node, aksiyon) çifti. Aksiyon verilmişse onu taşıyan start
/// kuralı; verilmemişse TEK start kuralı varsa o. Birden çok kural arasından seçim
/// yapılmaz: hangisinin koşacağını engine belirler, yanlış kuralın kapısını uygulamak
/// olmayan bir belgeyi istemek ya da olan bir kapıyı atlamak olurdu.
fn start_gate_target(
    wfd: &wfe_core::types::wfd_v22::Wfd,
    action: Option<&str>,
) -> Option<(String, String)> {
    let rule = match action {
        Some(a) => wfd.start.iter().find(|r| r.action == a)?,
        None => match wfd.start.as_slice() {
            [only] => only,
            _ => return None,
        },
    };
    Some((rule.from.clone(), rule.action.clone()))
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
    /// K5 (2026-08-10, WFE not tasarımı): apply BAŞARILI olduktan SONRA bu
    /// draft notu yayınlar (`wfah_seq`/`node` commit'in ürettiği geçişten
    /// türetilir — bkz. `publish_note_after_apply`). Göndermeyen istemci için
    /// davranış HİÇ değişmez.
    #[serde(default)]
    note_id: Option<Uuid>,
}

/// `WfeApplyResult` (`wf_wfe::executor`) ALAN EKLEMEDEN sarılır — o tipe alan
/// eklemek başka bir işin sözleşmesini değiştirirdi. `note_error` yalnız
/// `note_id` gönderilip yayınlama başarısız olduğunda dolar (K5).
#[derive(Serialize)]
struct ApplyResultWithNote {
    #[serde(flatten)]
    result: wf_wfe::executor::WfeApplyResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    note_error: Option<String>,
}

#[utoipa::path(post, path = "/{id}/actions", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = ApplyBody,
    responses((status = 200, description = "Uygulanan aksiyon sonucu (WfeApplyResult + opsiyonel note_error)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn apply_action(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<ApplyResultWithNote>, AppError> {
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
            let store = crate::attachment_store::store_for_wfe(&s, wfe_id).await?;
            let groups =
                crate::attachments::status_for_node(
                    &store,
                    &wfd,
                    wfe_id,
                    node,
                    Some(body.action.as_str()),
                )
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

    let result = s
        .executor
        .apply(
            wfe_id,
            &actor,
            &body.action,
            &body.input,
            body.node.as_deref(),
            body.expected_rev,
        )
        .await
        .map_err(AppError::from)?;

    // Not, apply BAŞARILI olduktan SONRA yayınlanır (K5) — geçiş zaten
    // gerçekleşti; not yayınlama hatası apply sonucunu YUTMAZ, `note_error`
    // olarak taşınır ve not draft kalır (kullanıcı tekrar dener).
    let note_error = match body.note_id {
        Some(note_id) => crate::notes::publish_after_apply(&s.pool, wfe_id, note_id, &actor)
            .await
            .err()
            .map(|e| e.message),
        None => None,
    };

    Ok(Json(ApplyResultWithNote { result, note_error }))
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

/// T‑A5 WF Admin: escalation müdahalesi gövdesi. `node` yalnız paralel modda anlamlı
/// (kol başına sayaç) — `reassign` ile aynı konvansiyon.
#[derive(Deserialize, ToSchema)]
struct EscalationAdminBody {
    #[serde(default)]
    node: Option<String>,
}

/// Sıradaki escalation adımını ELLE tetikler (vade beklemeden).
///
/// Adım numarası istemciden alınmaz: sıradaki ateşlenmemiş adım uygulanır, böylece
/// escalation adımlarının sıralı olma sözleşmesi korunur.
#[utoipa::path(post, path = "/{id}/escalation/fire", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = EscalationAdminBody,
    responses(
        (status = 200, description = "Uygulanan adım", body = serde_json::Value),
        (status = 400, description = "Paralel modda kol node'u verilmedi"),
        (status = 403, description = "Aktör wf_admin kurallarına uymuyor"),
        (status = 409, description = "Bekleyen adım yok (escalation.none_pending) veya WFE bitmiş"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn fire_escalation(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<EscalationAdminBody>,
) -> Result<Json<wf_wfe::executor::EscalationAdminOutcome>, AppError> {
    let admin = extract_actor(&headers)?;
    let outcome = s
        .executor
        .fire_escalation_now(wfe_id, &admin, body.node.as_deref())
        .await?;
    none_pending_to_conflict(outcome).map(Json)
}

/// Sıradaki escalation adımını ATLAR — geçiş uygulanmaz, audit satırı yazılır.
#[utoipa::path(post, path = "/{id}/escalation/skip", tag = "wfe",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = EscalationAdminBody,
    responses(
        (status = 200, description = "Atlanan adım", body = serde_json::Value),
        (status = 400, description = "Paralel modda kol node'u verilmedi"),
        (status = 403, description = "Aktör wf_admin kurallarına uymuyor"),
        (status = 409, description = "Bekleyen adım yok (escalation.none_pending) veya WFE bitmiş"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn skip_escalation(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<EscalationAdminBody>,
) -> Result<Json<wf_wfe::executor::EscalationAdminOutcome>, AppError> {
    let admin = extract_actor(&headers)?;
    let outcome = s
        .executor
        .skip_escalation(wfe_id, &admin, body.node.as_deref())
        .await?;
    none_pending_to_conflict(outcome).map(Json)
}

/// "Dokunacak adım yok" çekirdekte bir CEVAPtır; HTTP'de 409 + makine kodudur.
/// Dönüşüm burada, tek yerde yapılır (portal kabuğu da bunu kullanır).
pub(crate) fn none_pending_to_conflict(
    outcome: wf_wfe::executor::EscalationAdminOutcome,
) -> Result<wf_wfe::executor::EscalationAdminOutcome, AppError> {
    if matches!(
        outcome,
        wf_wfe::executor::EscalationAdminOutcome::NonePending
    ) {
        return Err(AppError {
            message: "bu node'da bekleyen escalation adımı yok".into(),
            status: StatusCode::CONFLICT,
            code: Some("escalation.none_pending"),
        });
    }
    Ok(outcome)
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
    /// WFE not tasarımı Faz 1 (K9): görünür (published + gizlenmemiş + bu
    /// aktöre `audience` açık) not sayısı — TEK toplu sorgu
    /// (`notes::count_by_wfe`), N+1 yok. `WfeView`'a DOKUNULMAZ; sayaç yalnız
    /// liste görünümündedir.
    note_count: i64,
    /// Faz 3 (K9 okundu takibi): bu aktör için OKUNMAMIŞ not sayısı — havuz
    /// rozeti "kaç not var" değil "kaç YENİ not var" göstersin diye
    /// `note_count`'un YANINA eklendi, onu YERİNE geçmedi.
    unread_note_count: i64,
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

    // WFE not tasarımı Faz 1/3: not sayaçları da TEK toplu sorguda (N+1 yok).
    let note_counts = crate::notes::count_by_wfe(&s.pool, &wfe_ids, &actor).await?;
    let unread_counts = crate::notes::unread_count_by_wfe(&s.pool, &wfe_ids, &actor).await?;

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
        let note_count = note_counts.get(&row.wfe_id).copied().unwrap_or(0);
        let unread_note_count = unread_counts.get(&row.wfe_id).copied().unwrap_or(0);
        out.push(WfeListItem {
            priority,
            claim_deadline,
            rev,
            branches,
            note_count,
            unread_note_count,
            row,
        });
    }
    Ok(Json(out))
}
