//! Portal WFE not defteri endpoint'leri — JWT `/portal/wfe/*` ağacı. Direkt
//! X-Actor ağacının kopyası `routes/notes.rs`'dedir; ikisi de `crate::notes`
//! ortak mantığını kullanır (bkz. `routes/portal/attachments.rs` ile aynı
//! desen). Faz 2: not dosyaları (`PUT/GET/DELETE .../files*`) eklendi — deseni
//! `routes/portal/attachments.rs::upload/download/remove`den alır, tek farkla:
//! depo çözümü `attachment_store::store_for_wfe_strict` (fallback YOK, K4).
//!
//! Yetki (K6): her uçta `executor.query(wfe_id, actor)` başarılı olmalı — WFE'yi
//! göremeyen aktör notu da göremez/değiştiremez (403/404).
//!
//! 2026-08-11 kuralı: not/dosya EKLEME claim ister (`notes::assert_actor_holds_claim`
//! — create/update/file-upload) ve yayın yalnız AKSİYONLA olur; serbest yayın
//! kaldırıldı (`publish_note` artık yalnız apply sonrası yeniden denemedir).
//! Silme/gizleme ve okuma uçları claim İSTEMEZ — kendi taslağını temizlemek claim
//! düştükten sonra da mümkün kalmalı.

use super::jwt::PortalActor;
use crate::{error::AppError, notes, state::AppState};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;
use wfe_core::types::actor::Actor;

/// wfe router'ına merge edilir (aynı `/:wfe_id` uzayında).
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_note, list_notes))
        .routes(routes!(update_note, delete_note))
        .routes(routes!(publish_note))
        .routes(routes!(mark_notes_read))
        .routes(routes!(upload_note_file, download_note_file, remove_note_file))
}

fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

#[derive(Deserialize, ToSchema)]
struct CreateNoteBody {
    body: String,
    /// K9 hedefleme: göndermeyen istemci `{"kind":"all"}` (herkes) alır.
    #[serde(default)]
    audience: notes::Audience,
}

#[derive(serde::Serialize, ToSchema)]
struct CreateNoteResult {
    note_id: Uuid,
}

#[utoipa::path(post,
    operation_id = "portal_note_create", path = "/{wfe_id}/notes", tag = "notes",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    request_body = CreateNoteBody,
    responses(
        (status = 200, description = "Draft not oluşturuldu — yalnız yazarı görür", body = CreateNoteResult),
        (status = 409, description = "Aktör bu işi claim etmemiş (`note.requires_claim`)"),
    ),
    security(("bearer_jwt" = [])))]
async fn create_note(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<CreateNoteBody>,
) -> Result<Json<CreateNoteResult>, AppError> {
    let a = to_actor(&actor);
    let view = s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::assert_actor_holds_claim(&view, &a)?;
    let note_id =
        notes::create_draft(&s.pool, wfe_id, actor.orgtnt_id, &a, body.body, body.audience)
            .await?;
    Ok(Json(CreateNoteResult { note_id }))
}

#[utoipa::path(get,
    operation_id = "portal_note_list", path = "/{wfe_id}/notes", tag = "notes",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "Görünür notlar: kendi + `notes_visible_to_caller` ile katılan alt akış notları (Faz 4)", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn list_notes(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<Vec<notes::NoteView>>, AppError> {
    let a = to_actor(&actor);
    s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    // K8 (Faz 4-runtime): bkz. direkt ağaçtaki `list_notes` yorumu — yetki
    // yalnız çağıran WFE üzerinden, çocuk için ayrı kapı yok.
    let wfd = super::attachments::load_wfd_for_wfe(&s, wfe_id, actor.orgtnt_id).await?;
    let list = notes::list_visible_with_children(&s.pool, &wfd, wfe_id, &a).await?;
    Ok(Json(list))
}

#[derive(Deserialize, ToSchema)]
struct MarkReadBody {
    note_ids: Vec<Uuid>,
}

#[utoipa::path(post,
    operation_id = "portal_note_mark_read", path = "/{wfe_id}/notes/read", tag = "notes",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    request_body = MarkReadBody,
    responses((status = 204, description = "İşaretlendi — kapsam dışı/görünmeyen note_id'ler sessizce atlanır")),
    security(("bearer_jwt" = [])))]
async fn mark_notes_read(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<MarkReadBody>,
) -> Result<StatusCode, AppError> {
    let a = to_actor(&actor);
    s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::mark_read(&s.pool, wfe_id, &body.note_ids, &a).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
struct UpdateNoteBody {
    body: String,
}

#[utoipa::path(patch,
    operation_id = "portal_note_update", path = "/{wfe_id}/notes/{note_id}", tag = "notes",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    request_body = UpdateNoteBody,
    responses(
        (status = 204, description = "Güncellendi (yalnız draft, yalnız yazarı)"),
        (status = 409, description = "Aktör bu işi claim etmemiş (`note.requires_claim`)"),
    ),
    security(("bearer_jwt" = [])))]
async fn update_note(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateNoteBody>,
) -> Result<StatusCode, AppError> {
    let a = to_actor(&actor);
    let view = s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::assert_actor_holds_claim(&view, &a)?;
    notes::update_draft(&s.pool, wfe_id, note_id, &a, body.body).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete,
    operation_id = "portal_note_remove", path = "/{wfe_id}/notes/{note_id}", tag = "notes",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    responses((status = 204, description = "Draft silindi / published gizlendi")),
    security(("bearer_jwt" = [])))]
async fn delete_note(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let a = to_actor(&actor);
    s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::hide(&s.pool, wfe_id, note_id, &a).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Yayın-DIŞI tek yol: apply BAŞARILI oldu ama not yayınlanamadı (`note_error`)
/// → yeniden dene. Serbest (aksiyona bağlı olmayan) yayın KALDIRILDI — bkz.
/// `notes::republish_after_apply`.
#[utoipa::path(post,
    operation_id = "portal_note_publish", path = "/{wfe_id}/notes/{note_id}/publish", tag = "notes",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    responses(
        (status = 204, description = "Son aksiyona çapalanarak yayınlandı (apply sonrası yeniden deneme)"),
        (status = 409, description = "Son wfah kaydı bu aktörün değil (`note.requires_action`)"),
    ),
    security(("bearer_jwt" = [])))]
async fn publish_note(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let a = to_actor(&actor);
    s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::republish_after_apply(&s.pool, wfe_id, note_id, &a).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
struct NoteFileUploadResult {
    file_id: Uuid,
    filename: String,
    mime: String,
    size_bytes: i64,
}

fn content_type_of(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| "application/octet-stream".into())
}

fn filename_of(headers: &HeaderMap) -> String {
    headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .map(crate::notes::decode_filename)
        .unwrap_or_else(|| "dosya".to_string())
}

#[utoipa::path(put,
    operation_id = "portal_note_file_upload", path = "/{wfe_id}/notes/{note_id}/files", tag = "notes",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    request_body(content = Vec<u8>, description = "Dosya içeriği (binary); ad `X-Filename` header'ından, tip `Content-Type`'tan okunur"),
    responses(
        (status = 200, description = "Dosya eklendi", body = NoteFileUploadResult),
        (status = 409, description = "Not draft değil (`note.immutable`) ya da aktör claim etmemiş (`note.requires_claim`)"),
        (status = 413, description = "Dosya çok büyük (`note.too_large`)"),
        (status = 415, description = "İzin verilmeyen içerik tipi (`note.unsupported_type`)"),
        (status = 422, description = "Kota aşıldı ya da $env'de depo tanımsız (`attachment_storage.missing_env`)"),
    ),
    security(("bearer_jwt" = [])))]
async fn upload_note_file(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<NoteFileUploadResult>, AppError> {
    let a = to_actor(&actor);
    let view = s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::assert_actor_holds_claim(&view, &a)?;
    if body.is_empty() {
        return Err(AppError(
            "boş dosya yüklenemez".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let filename = filename_of(&headers);
    let mime = content_type_of(&headers);

    // K4: depo ÖNCE çözülür (fallback'siz) — $env eksikse DB satırı hiç yazılmaz.
    let store = crate::attachment_store::store_for_wfe_strict(&s, wfe_id).await?;
    let file_id = notes::add_file(
        &s.pool,
        wfe_id,
        note_id,
        &a,
        &filename,
        &mime,
        body.len() as i64,
    )
    .await?;
    if let Err(e) = store.note_write(wfe_id, file_id, body.to_vec()).await {
        let _ = notes::remove_file(&s.pool, wfe_id, note_id, file_id, &a).await;
        return Err(AppError(
            format!("yükleme başarısız: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
    }
    Ok(Json(NoteFileUploadResult {
        file_id,
        filename: notes::sanitize_filename(&filename),
        mime,
        size_bytes: body.len() as i64,
    }))
}

#[utoipa::path(get,
    operation_id = "portal_note_file_download", path = "/{wfe_id}/notes/{note_id}/files/{file_id}", tag = "notes",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
        ("file_id" = Uuid, Path, description = "Dosya id"),
    ),
    responses((status = 200, description = "Dosya içeriği (binary)")),
    security(("bearer_jwt" = [])))]
async fn download_note_file(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, note_id, file_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let a = to_actor(&actor);
    s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    let file = notes::find_file(&s.pool, wfe_id, note_id, file_id, &a).await?;
    let store = crate::attachment_store::store_for_wfe_strict(&s, wfe_id).await?;
    let bytes = store
        .note_read(wfe_id, file_id)
        .await
        .map_err(|_| AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND))?;

    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        file.mime
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    h.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().unwrap(),
    );
    let escaped = file.filename.replace('\\', "\\\\").replace('"', "\\\"");
    if let Ok(v) = format!("attachment; filename=\"{escaped}\"").parse() {
        h.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    Ok((StatusCode::OK, h, Bytes::from(bytes)))
}

#[utoipa::path(delete,
    operation_id = "portal_note_file_remove", path = "/{wfe_id}/notes/{note_id}/files/{file_id}", tag = "notes",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
        ("file_id" = Uuid, Path, description = "Dosya id"),
    ),
    responses(
        (status = 204, description = "Silindi"),
        (status = 409, description = "Not draft değil (`note.immutable`)"),
    ),
    security(("bearer_jwt" = [])))]
async fn remove_note_file(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, note_id, file_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let a = to_actor(&actor);
    s.executor.query(wfe_id, &a).await.map_err(AppError::from)?;
    notes::remove_file(&s.pool, wfe_id, note_id, file_id, &a).await?;
    let store = crate::attachment_store::store_for_wfe_strict(&s, wfe_id).await?;
    store.note_delete(wfe_id, file_id).await.map_err(|e| {
        AppError(
            format!("silme başarısız: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}
