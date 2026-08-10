//! WFE not defteri endpoint'leri — DİREKT `/wfe/*` route ağacı (X-Actor header
//! auth). JWT `/portal/wfe/*` ağacının kendi kopyası `routes/portal/notes.rs`'dedir;
//! ikisi de `crate::notes` ortak mantığını kullanır (bkz. `routes/attachments.rs`
//! ile aynı desen). Faz 2: not dosyaları (`PUT/GET/DELETE .../files*`) eklendi —
//! deseni `routes/attachments.rs::upload/download/remove`den alır, tek farkla:
//! depo çözümü `attachment_store::store_for_wfe_strict` (fallback YOK, K4).
//!
//! Yetki (K6): her uçta `executor.query(wfe_id, actor)` başarılı olmalı — WFE'yi
//! göremeyen aktör notu da göremez/değiştiremez (403/404). Draft'a özgü ek
//! kısıtlar (yalnız yazarı düzenler/siler/yayınlar/dosya ekler-siler) `crate::notes`
//! içindedir.

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

/// `/wfe` router'ına merge edilir.
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_note, list_notes))
        .routes(routes!(update_note, delete_note))
        .routes(routes!(publish_note))
        .routes(routes!(mark_notes_read))
        .routes(routes!(upload_note_file, download_note_file, remove_note_file))
}

async fn orgtnt_of(s: &AppState, actor: &Actor) -> Result<Uuid, AppError> {
    s.executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)
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

#[utoipa::path(post, path = "/{id}/notes", tag = "notes",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = CreateNoteBody,
    responses((status = 200, description = "Draft not oluşturuldu — yalnız yazarı görür", body = CreateNoteResult)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn create_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<CreateNoteBody>,
) -> Result<Json<CreateNoteResult>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    let orgtnt_id = orgtnt_of(&s, &actor).await?;
    let note_id =
        notes::create_draft(&s.pool, wfe_id, orgtnt_id, &actor, body.body, body.audience).await?;
    Ok(Json(CreateNoteResult { note_id }))
}

#[utoipa::path(get, path = "/{id}/notes", tag = "notes",
    params(("id" = Uuid, Path, description = "WFE id")),
    responses((status = 200, description = "Görünür notlar: kendi + `notes_visible_to_caller` ile katılan alt akış notları (Faz 4)", body = serde_json::Value)),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn list_notes(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<Vec<notes::NoteView>>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    // K8 (Faz 4-runtime): WFD çağıranın YAPTIĞI çağrıları çözmek için gerekli
    // (`calls.<key>.notes_visible_to_caller`) — yetki zaten `executor.query`
    // ile doğrulandı, çocuk WFE için ayrı bir kapı KOŞULMAZ (bkz. notes.rs doc).
    let wfd = super::attachments::load_wfd(&s, wfe_id).await?;
    let list = notes::list_visible_with_children(&s.pool, &wfd, wfe_id, &actor).await?;
    Ok(Json(list))
}

#[derive(Deserialize, ToSchema)]
struct MarkReadBody {
    note_ids: Vec<Uuid>,
}

#[utoipa::path(post, path = "/{id}/notes/read", tag = "notes",
    params(("id" = Uuid, Path, description = "WFE id")),
    request_body = MarkReadBody,
    responses((status = 204, description = "İşaretlendi — kapsam dışı/görünmeyen note_id'ler sessizce atlanır")),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn mark_notes_read(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<MarkReadBody>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    notes::mark_read(&s.pool, wfe_id, &body.note_ids, &actor).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
struct UpdateNoteBody {
    body: String,
}

#[utoipa::path(patch, path = "/{id}/notes/{note_id}", tag = "notes",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    request_body = UpdateNoteBody,
    responses((status = 204, description = "Güncellendi (yalnız draft, yalnız yazarı)")),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn update_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateNoteBody>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    notes::update_draft(&s.pool, wfe_id, note_id, &actor, body.body).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/{id}/notes/{note_id}", tag = "notes",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    responses((status = 204, description = "Draft silindi / published gizlendi")),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn delete_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    notes::hide(&s.pool, wfe_id, note_id, &actor).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/{id}/notes/{note_id}/publish", tag = "notes",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    responses((status = 204, description = "Serbest yayınlama (aksiyona bağlı değil)")),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn publish_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    notes::publish(&s.pool, wfe_id, note_id, &actor, None, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
struct NoteFileUploadResult {
    file_id: Uuid,
    filename: String,
    mime: String,
    size_bytes: i64,
}

/// `Content-Type` başlığından parametre kısmını (`; charset=...`) at.
fn content_type_of(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| "application/octet-stream".into())
}

/// `X-Filename` başlığından ham dosya adı (`notes::sanitize_filename` çağıran
/// katmandaki `add_file` içinde temizler, burada yalnız düşürülmez bırakılır).
fn filename_of(headers: &HeaderMap) -> String {
    headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .map(crate::notes::decode_filename)
        .unwrap_or_else(|| "dosya".to_string())
}

#[utoipa::path(put, path = "/{id}/notes/{note_id}/files", tag = "notes",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
    ),
    request_body(content = Vec<u8>, description = "Dosya içeriği (binary); ad `X-Filename` header'ından, tip `Content-Type`'tan okunur"),
    responses(
        (status = 200, description = "Dosya eklendi", body = NoteFileUploadResult),
        (status = 409, description = "Not draft değil (`note.immutable`)"),
        (status = 413, description = "Dosya çok büyük (`note.too_large`)"),
        (status = 415, description = "İzin verilmeyen içerik tipi (`note.unsupported_type`)"),
        (status = 422, description = "Kota aşıldı ya da $env'de depo tanımsız (`attachment_storage.missing_env`)"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn upload_note_file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, note_id)): Path<(Uuid, Uuid)>,
    body: Bytes,
) -> Result<Json<NoteFileUploadResult>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
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
        &actor,
        &filename,
        &mime,
        body.len() as i64,
    )
    .await?;
    if let Err(e) = store.note_write(wfe_id, file_id, body.to_vec()).await {
        // Blob yazımı başarısız — DB satırı yetim kalmasın, geri al (best-effort).
        let _ = notes::remove_file(&s.pool, wfe_id, note_id, file_id, &actor).await;
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

#[utoipa::path(get, path = "/{id}/notes/{note_id}/files/{file_id}", tag = "notes",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
        ("file_id" = Uuid, Path, description = "Dosya id"),
    ),
    responses((status = 200, description = "Dosya içeriği (binary)")),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn download_note_file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, note_id, file_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    let file = notes::find_file(&s.pool, wfe_id, note_id, file_id, &actor).await?;
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
    // Dosya adı quote-escape edilir (`"`/`\` kaçışsız header'ı bozardı).
    let escaped = file.filename.replace('\\', "\\\\").replace('"', "\\\"");
    if let Ok(v) = format!("attachment; filename=\"{escaped}\"").parse() {
        h.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    Ok((StatusCode::OK, h, Bytes::from(bytes)))
}

#[utoipa::path(delete, path = "/{id}/notes/{note_id}/files/{file_id}", tag = "notes",
    params(
        ("id" = Uuid, Path, description = "WFE id"),
        ("note_id" = Uuid, Path, description = "Not id"),
        ("file_id" = Uuid, Path, description = "Dosya id"),
    ),
    responses(
        (status = 204, description = "Silindi"),
        (status = 409, description = "Not draft değil (`note.immutable`)"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn remove_note_file(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((wfe_id, note_id, file_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map_err(AppError::from)?;
    notes::remove_file(&s.pool, wfe_id, note_id, file_id, &actor).await?;
    let store = crate::attachment_store::store_for_wfe_strict(&s, wfe_id).await?;
    store.note_delete(wfe_id, file_id).await.map_err(|e| {
        AppError(
            format!("silme başarısız: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}
