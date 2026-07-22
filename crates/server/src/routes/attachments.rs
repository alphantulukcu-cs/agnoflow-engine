//! Ek-belge (attachment) endpoint'leri — DİREKT `/wfe/*` route ağacı (X-Actor
//! header auth). work-pool-portal bu ağacı kullanır (bkz. MEMORY: portal /wfe/*
//! direkt rotaları kullanır). JWT `/portal/wfe/*` ağacının kendi kopyası
//! `routes/portal/attachments.rs`'dedir; ikisi de `crate::attachments` paylaşımlı
//! store + durum yardımcılarını kullanır.
//!
//! Mimari: engine core dosya I/O yapmaz. Varlık kontrolü + yükleme burada
//! (opendal `AttachmentStore`) yürür; yetki `executor.query` (görünürlük) ile
//! doğrulanır. Aksiyon gate'i `routes/wfe.rs::apply_action` içinde uygulanır.

use crate::attachments::{status_for_node, AttachmentGroupStatus};
use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::{AttachmentItem, Wfd};
use wfe_core::v22::ports::WfdStore;

/// `/wfe` router'ına merge edilir.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/:id/attachments", get(status))
        .route(
            "/:id/attachments/:group/:item",
            get(download).put(upload).delete(remove),
        )
}

#[derive(Serialize)]
struct NodeAttachmentStatus {
    satisfied: bool,
    groups: Vec<AttachmentGroupStatus>,
}

#[derive(Serialize)]
struct AttachmentsResponse {
    /// node slug → o node'un attachment durumu. Tek-node modda tek anahtar; paralel
    /// modda her aktif kol node'u için bir anahtar. İstemci, aksiyonu uygularken
    /// kullanacağı node'un durumunu buradan okur.
    attachments: BTreeMap<String, NodeAttachmentStatus>,
}

/// WFE'nin WFD'sini getirir. Yetki `executor.query` ile ayrıca doğrulanır — bu
/// yalnız wfd_id/version çözümüdür (orgtnt filtresi yok; direkt route deseni).
pub(crate) async fn load_wfd(s: &AppState, wfe_id: Uuid) -> Result<Wfd, AppError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        wfd_id: Uuid,
        wfd_version: i32,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT wfd_id, wfd_version FROM wf.wfe WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    s.wfd
        .fetch(row.wfd_id, row.wfd_version)
        .await
        .map_err(AppError::from)
}

/// Aktörün bu WFE'yi görebildiğini doğrular (executor.query authz), aktif node
/// listesini döndürür (tek-node → current_node; paralel → kol node'ları).
async fn authorized_nodes(
    s: &AppState,
    actor: &Actor,
    wfe_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let view = s
        .executor
        .query(wfe_id, actor)
        .await
        .map_err(AppError::from)?;
    let nodes = match view.current_node {
        Some(n) => vec![n],
        None => view.branches.iter().map(|b| b.state.branch_node.clone()).collect(),
    };
    Ok(nodes)
}

fn find_item<'a>(wfd: &'a Wfd, group: &str, item: &str) -> Result<&'a AttachmentItem, AppError> {
    wfd.attachments
        .get(group)
        .and_then(|g| g.items.iter().find(|i| i.id == item))
        .ok_or_else(|| {
            AppError(
                format!("attachment slotu bulunamadı: {group}/{item}"),
                StatusCode::NOT_FOUND,
            )
        })
}

/// `check_upload`'ı çağırıp reddi HTTP statüsüne çevirir.
fn validate_upload(item: &AttachmentItem, content_type: Option<&str>, len: usize) -> Result<(), AppError> {
    match crate::attachments::check_upload(item, content_type, len) {
        Ok(()) => Ok(()),
        Err(crate::attachments::UploadReject::UnsupportedType(ct)) => Err(AppError(
            format!("izin verilmeyen içerik tipi: {ct}"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        )),
        Err(crate::attachments::UploadReject::TooLarge(max_mb)) => Err(AppError(
            format!("dosya {max_mb} MB sınırını aşıyor"),
            StatusCode::PAYLOAD_TOO_LARGE,
        )),
    }
}

async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<AttachmentsResponse>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    let nodes = authorized_nodes(&s, &actor, wfe_id).await?;
    let wfd = load_wfd(&s, wfe_id).await?;

    let mut map = BTreeMap::new();
    for node in nodes {
        let groups = status_for_node(&s.attachments, &wfd, wfe_id, &node)
            .await
            .map_err(|e| {
                AppError(
                    format!("attachment durum sorgusu başarısız: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;
        if groups.is_empty() {
            continue;
        }
        map.insert(
            node,
            NodeAttachmentStatus {
                satisfied: crate::attachments::satisfied(&groups),
                groups,
            },
        );
    }
    Ok(Json(AttachmentsResponse { attachments: map }))
}

async fn upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    // Yetki: aktör WFE'yi görebilmeli.
    authorized_nodes(&s, &actor, wfe_id).await?;
    let wfd = load_wfd(&s, wfe_id).await?;
    let def = find_item(&wfd, &group, &item)?;

    if body.is_empty() {
        return Err(AppError(
            "boş dosya yüklenemez".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim());
    validate_upload(def, ct, body.len())?;

    s.attachments
        .write(wfe_id, &group, &item, body.to_vec())
        .await
        .map_err(|e| {
            AppError(
                format!("yükleme başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

    Ok(Json(serde_json::json!({
        "uploaded": true, "group": group, "item": item
    })))
}

async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    authorized_nodes(&s, &actor, wfe_id).await?;
    let wfd = load_wfd(&s, wfe_id).await?;
    find_item(&wfd, &group, &item)?;

    let bytes = s
        .attachments
        .read(wfe_id, &group, &item)
        .await
        .map_err(|_| AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND))?;

    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    Ok((StatusCode::OK, h, Bytes::from(bytes)))
}

async fn remove(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    authorized_nodes(&s, &actor, wfe_id).await?;
    let wfd = load_wfd(&s, wfe_id).await?;
    find_item(&wfd, &group, &item)?;

    s.attachments
        .delete(wfe_id, &group, &item)
        .await
        .map_err(|e| {
            AppError(
                format!("silme başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}
