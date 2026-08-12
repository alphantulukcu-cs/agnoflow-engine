//! Portal WFE detay + aksiyon endpoint'leri (WOR-44 — v2.2).
//! Assignment/permission kontrolleri engine pipeline'ındadır (§7.1-7.2);
//! burada yalnızca görünüm zenginleştirme yapılır.

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
use wf_wfe::executor::BranchView;
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(get_wfe_detail))
        .routes(routes!(submit_action))
        .routes(routes!(portal_fire_escalation))
        .routes(routes!(portal_skip_escalation))
        .merge(super::attachments::routes())
        .merge(super::notes::routes())
        .with_state(state)
}

fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct ActionInputSchema {
    required: Vec<String>,
    optional: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct AvailableAction {
    /// Aksiyonun kimlik+gösterim çifti (`{id, label}`) — `id` istekte AYNEN geri gider.
    #[schema(value_type = Object)]
    action: wf_wfe::executor::Ref,
    input: ActionInputSchema,
    /// GLB (global aksiyon) hedef seçimi — yalnız `wft: {targets}` aksiyonlarında
    /// bulunur. İstemci seçilen `options[].id`yi gövdede `target` olarak yollar.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    target: Option<wf_wfe::executor::TargetChoice>,
    /// WOR-31 T4: paralel modda bu aksiyonun ait olduğu kol — action
    /// gönderiminde `branch` olarak geri geçilmelidir (aksiyon ≥2 kolla eşleşirse
    /// disambiguasyon için zorunlu). Paralel değilse `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    branch: Option<wf_wfe::executor::Ref>,
    /// Bu aksiyonun kaynak node'una bağlı ek-belge grupları ve item bazlı yükleme
    /// durumu. Boşsa attachment gate yok. Portal, `attachments_satisfied=false`
    /// iken submit butonunu disable eder ve eksik dosyalar için upload gösterir.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Object>)]
    attachments: Vec<super::attachments::AttachmentGroupStatus>,
    /// Kaynak node'un tüm `required` dosyaları yüklü mü? attachments boşsa `true`.
    attachments_satisfied: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct WfeDetailResponse {
    wfe_id: Uuid,
    wfd_name: String,
    /// Kimlik + gösterim (`{id, label}`) — ayrı bir `node_label` alanı YOK,
    /// etiket kimliğin yanında taşınır.
    #[schema(value_type = Object)]
    current_node: Option<wf_wfe::executor::Ref>,
    dynctx: Value,
    claimed_by: Option<Uuid>,
    available_actions: Vec<AvailableAction>,
    /// WOR-31 T4: paralel mod kol durumları — `/wfe/:id` (WfeView) ile aynı şekil:
    /// `[{node, status, claimed_by, claimed_at, entered_at, c_a}]`; `c_a` sorgu-anında
    /// çözülmüş kol claim adayları (bkz. `BranchView`); paralel değilken boş dizi.
    #[schema(value_type = Vec<Object>)]
    branches: Vec<BranchView>,
    /// WOR-31 T4: fork'ta persist edilen AND-join hedefi; `Some` = paralel mod
    /// (bu durumda `current_node` `None`'dur). `{kind, node|terminal}` — hedef `Ref`.
    #[schema(value_type = Object)]
    join_target: Option<wf_wfe::executor::JoinTargetView>,
    /// Madde 6: tek-kol modda viewer'ın claim provenance'ı (direct/delegated); paralel
    /// modda `None` — kol-bazlı `branches[].claim_as`'e bakılır.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    claim_as: Option<wf_wfe::executor::ClaimProvenance>,
    /// WOR-65: WFE revizyon token'ı — `/wfe/:id` (WfeView) ile AYNI değer.
    /// Portal bunu saklayıp `POST /portal/wfe/:id/action` gövdesinde
    /// `expected_rev` olarak geri gönderir; arada durum değiştiyse 409 +
    /// `conflict.stale_revision` alır ve jenerik toast yerine "bu görev artık
    /// sizde değil, sayfa yenileniyor" diyebilir.
    rev: u32,
}

#[derive(sqlx::FromRow)]
struct WfeInfoRow {
    wfd_id: Uuid,
    wfd_version: i32,
    wfd_name: String,
}

#[utoipa::path(get, path = "/{wfe_id}", tag = "portal",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "WFE detayı + uygulanabilir aksiyonlar", body = WfeDetailResponse)),
    security(("bearer_jwt" = [])))]
async fn get_wfe_detail(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<WfeDetailResponse>, AppError> {
    let row = sqlx::query_as::<_, WfeInfoRow>(
        "SELECT e.wfd_id, e.wfd_version, m.name AS wfd_name
         FROM wf.wfe e
         JOIN wf.wfd_meta m ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.wfe_id = $1 AND e.orgtnt_id = $2 AND e.status = 'active'",
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    let portal_actor = to_actor(&actor);

    let view = s
        .executor
        .query(wfe_id, &portal_actor)
        .await
        .map_err(AppError::from)?;
    let possible = s
        .executor
        .possible_actions(wfe_id, &portal_actor)
        .await
        .map_err(AppError::from)?;

    let wfd = s
        .wfd
        .fetch(row.wfd_id, row.wfd_version)
        .await
        .map_err(AppError::from)?;

    // WOR-31 T4: paralel modda aynı aksiyon adı birden fazla kolda tekrar edebilir
    // (ör. üç kolun da `approve`'u) — her tekrar kendi `branch`'iyle ayrı satırdır,
    // istemci hangi kolu onayladığını `branch`i geri göndererek belirtir.
    // Depo WFD başına çözülür ($env) — döngü başına değil, bir kez.
    let store = crate::attachment_store::store_for_wfe(&s, wfe_id).await?;
    let mut available_actions: Vec<AvailableAction> = Vec::with_capacity(possible.len());
    for pa in &possible {
        let Some(def) = wfd.actions.get(&pa.action.id) else {
            continue;
        };
        // Aksiyonun kaynak node'u: paralelde kol node'u, aksi halde current_node.
        let src_node = pa
            .branch
            .as_ref()
            .map(|b| b.id.clone())
            .or_else(|| view.current_node.as_ref().map(|n| n.id.clone()));
        let attachments = match &src_node {
            // Durum AKSIYON BAŞINA sorulur: aynı node'da "Onayla" belge isterken
            // "Reddet" istemeyebilir — `attachments_satisfied` o aksiyonun cevabıdır.
            Some(n) => super::attachments::status_for_node(
                &store,
                &wfd,
                wfe_id,
                n,
                Some(pa.action.id.as_str()),
            )
            .await
                .map_err(|e| {
                    AppError(
                        format!("attachment durum sorgusu başarısız: {e}"),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                })?,
            None => vec![],
        };
        // Kapı kararı (`satisfied`) DEPODAN gelir; metadata yalnız GÖSTERİM için eklenir
        // (ad/tip/boyut/tarih). Direkt `/wfe/*` ağacındaki `GET /wfe/{id}/attachments` ile
        // aynı fonksiyon — iki ağaç aynı cevabı vermeli. Metadata okunamazsa süsleme
        // düşer, aksiyon listesi ayakta kalır: bir gösterim ayrıntısı yüzünden portal
        // detay sayfasını 500'e düşürmüyoruz.
        let mut attachments = attachments;
        match crate::wfe_attachment::list_by_wfe(&s.pool, wfe_id).await {
            Ok(metas) => super::attachments::enrich_with_meta(&mut attachments, &metas),
            Err(e) => tracing::warn!(%wfe_id, "attachment metadata okunamadı: {}", e.message),
        }
        let attachments_satisfied = super::attachments::satisfied(&attachments);
        available_actions.push(AvailableAction {
            action: pa.action.clone(),
            input: ActionInputSchema {
                required: def.input.required.clone(),
                optional: def.input.optional.clone(),
            },
            target: pa.target.clone(),
            branch: pa.branch.clone(),
            attachments,
            attachments_satisfied,
        });
    }

    Ok(Json(WfeDetailResponse {
        wfe_id,
        wfd_name: row.wfd_name,
        current_node: view.current_node,
        dynctx: view.dynctx,
        claimed_by: view.claimed_by,
        available_actions,
        branches: view.branches,
        join_target: view.join_target,
        claim_as: view.claim_as,
        rev: view.rev,
    }))
}

#[derive(Deserialize, ToSchema)]
struct ActionRequest {
    action: String,
    #[serde(default)]
    input: Value,
    /// WOR-31 T4: paralel modda kol seçimi (bkz. `AvailableAction.branch`).
    #[serde(default)]
    branch: Option<String>,
    /// GLB hedef seçimi (bkz. `AvailableAction.target`) — `wft: {targets}`
    /// aksiyonlarında ZORUNLU, diğerlerinde YASAK.
    #[serde(default)]
    target: Option<String>,
    /// WOR-65: `WfeDetailResponse.rev`'den okunan revizyon token'ı. OPSİYONEL —
    /// göndermeyen istemci bugünkü davranışı görür. Uyuşmazlıkta hiçbir yan etki
    /// üretilmeden 409 + `code: "conflict.stale_revision"`.
    #[serde(default)]
    expected_rev: Option<u32>,
    /// K5 (2026-08-10, WFE not tasarımı): apply BAŞARILI olduktan SONRA bu
    /// draft notu yayınlar (bkz. `crate::notes::publish_after_apply`).
    /// Göndermeyen istemci için davranış HİÇ değişmez.
    #[serde(default)]
    note_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
struct ActionResponse {
    wfe_status: String,
    #[schema(value_type = Object)]
    current_node: Option<wf_wfe::executor::Ref>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_response: Option<Value>,
    /// K5: yalnız `note_id` gönderilip yayınlama başarısız olduğunda dolar —
    /// apply BAŞARILI olduysa sonuç yine döner, not draft kalır.
    #[serde(skip_serializing_if = "Option::is_none")]
    note_error: Option<String>,
}

#[utoipa::path(post, path = "/{wfe_id}/action", tag = "portal",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    request_body = ActionRequest,
    responses((status = 200, description = "Aksiyon sonucu", body = ActionResponse)),
    security(("bearer_jwt" = [])))]
async fn submit_action(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ActionRequest>,
) -> Result<Json<ActionResponse>, AppError> {
    // Attachment gate (portal katmanı): hedef node'un `required` dosyaları
    // yüklenmeden engine'e HİÇ gitmeyiz. UI zaten disable eder; bu server-side
    // zorlamadır (UI-only gating'e güvenilmez). Engine core dosyadan habersiz kalır.
    let wfd = super::attachments::load_wfd_for_wfe(&s, wfe_id, actor.orgtnt_id).await?;
    let view = s
        .executor
        .query(wfe_id, &to_actor(&actor))
        .await
        .map_err(AppError::from)?;
    // Paralelde current_node None'dur; kol seçimi body.branch ile gelir.
    let target_node = body
        .branch
        .clone()
        .or_else(|| view.current_node.as_ref().map(|n| n.id.clone()));
    if let Some(node) = &target_node {
        let store = crate::attachment_store::store_for_wfe(&s, wfe_id).await?;
        let groups =
            super::attachments::status_for_node(
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
        let missing = super::attachments::missing_required(&groups);
        if !missing.is_empty() {
            return Err(AppError {
                message: format!("Eksik zorunlu belgeler: {}", missing.join(", ")),
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: Some("attachment.missing"),
                items: None,
            });
        }
    }

    // Assignment, yetki, input validasyonu ve claimed_by reset'i engine +
    // atomik commit içinde — burada ek SQL yok (WOR-43).
    let result = s
        .executor
        .apply(
            wfe_id,
            &to_actor(&actor),
            &body.action,
            &body.input,
            body.branch.as_deref(),
            body.target.as_deref(),
            body.expected_rev,
        )
        .await
        .map_err(AppError::from)?;

    // Not, apply BAŞARILI olduktan SONRA yayınlanır (K5) — geçiş zaten
    // gerçekleşti; yayınlama hatası apply sonucunu YUTMAZ, `note_error` olarak
    // taşınır ve not draft kalır (kullanıcı tekrar dener).
    let note_error = match body.note_id {
        Some(note_id) => {
            crate::notes::publish_after_apply(&s.pool, wfe_id, note_id, &to_actor(&actor))
                .await
                .err()
                .map(|e| e.message)
        }
        None => None,
    };

    Ok(Json(ActionResponse {
        wfe_status: if result.terminal {
            "terminal".into()
        } else {
            "active".into()
        },
        current_node: result.current_node,
        end_response: result.end_response,
        note_error,
    }))
}

// ── T‑A5 WF Admin: escalation müdahalesi (portal ağacı, JWT) ─────────────────
//
// `/wfe/*` ağacındaki ikizlerin ince kabuğu: yetki, terminal kontrolü ve adım seçimi
// çekirdektedir; burada yalnız aktör JWT'den çözülür. "Bekleyen adım yok" → 409
// dönüşümü de ortaktır (`routes::wfe::none_pending_to_conflict`).

#[derive(Deserialize, ToSchema)]
struct PortalEscalationBody {
    #[serde(default)]
    node: Option<String>,
}

#[utoipa::path(post, path = "/{id}/escalation/fire", tag = "portal",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = PortalEscalationBody,
    responses(
        (status = 200, description = "Uygulanan adım", body = serde_json::Value),
        (status = 403, description = "Aktör wf_admin kurallarına uymuyor"),
        (status = 409, description = "Bekleyen adım yok (escalation.none_pending)"),
    ),
    security(("bearer_jwt" = [])))]
async fn portal_fire_escalation(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<PortalEscalationBody>,
) -> Result<Json<wf_wfe::executor::EscalationAdminOutcome>, AppError> {
    let outcome = s
        .executor
        .fire_escalation_now(wfe_id, &to_actor(&actor), body.node.as_deref())
        .await?;
    crate::routes::wfe::none_pending_to_conflict(outcome).map(Json)
}

#[utoipa::path(post, path = "/{id}/escalation/skip", tag = "portal",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = PortalEscalationBody,
    responses(
        (status = 200, description = "Atlanan adım", body = serde_json::Value),
        (status = 403, description = "Aktör wf_admin kurallarına uymuyor"),
        (status = 409, description = "Bekleyen adım yok (escalation.none_pending)"),
    ),
    security(("bearer_jwt" = [])))]
async fn portal_skip_escalation(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<PortalEscalationBody>,
) -> Result<Json<wf_wfe::executor::EscalationAdminOutcome>, AppError> {
    let outcome = s
        .executor
        .skip_escalation(wfe_id, &to_actor(&actor), body.node.as_deref())
        .await?;
    crate::routes::wfe::none_pending_to_conflict(outcome).map(Json)
}
