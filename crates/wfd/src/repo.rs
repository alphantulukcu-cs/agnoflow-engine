use crate::{error::WfdError, models::WfdMeta};
use sqlx::PgPool;
use uuid::Uuid;

const COLS: &str = "wfd_id, orgtnt_id, project_id, name, version, s3_key, is_active, created_at, \
                    status, description, tags, owner, updated_at, source_template_id, review_note, \
                    submitted_by, doc_id, doc_version";
const M_COLS: &str = "m.wfd_id, m.orgtnt_id, m.project_id, m.name, m.version, m.s3_key, m.is_active, m.created_at, \
                      m.status, m.description, m.tags, m.owner, m.updated_at, m.source_template_id, m.review_note, \
                      m.submitted_by, m.doc_id, m.doc_version";

/// Yeni satır ekler (published veya draft). status/description/tags/owner verilir.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &PgPool,
    wfd_id: Uuid,
    orgtnt_id: Uuid,
    project_id: Uuid,
    name: &str,
    version: i32,
    s3_key: &str,
    status: &str,
    description: Option<&str>,
    tags: &[String],
    owner: &str,
    source_template_id: Option<Uuid>,
    // WFC: dokümanın kendi `id`/`version` alanları. Bunlar olmadan bir çağrı
    // (`calls.<key>.wfd_id`) çözülemez — bkz. `resolve_doc`.
    doc_id: Option<&str>,
    doc_version: Option<&str>,
) -> Result<Uuid, WfdError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfd_meta \
         (wfd_id, orgtnt_id, project_id, name, version, s3_key, status, description, tags, owner, source_template_id, doc_id, doc_version) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING wfd_id"
    )
    .bind(wfd_id).bind(orgtnt_id).bind(project_id).bind(name).bind(version).bind(s3_key)
    .bind(status).bind(description).bind(tags).bind(owner).bind(source_template_id)
    .bind(doc_id).bind(doc_version)
    .fetch_one(pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.constraint()) {
        // tek-draft kısmi-unique index ihlali (versiyon unique'i buraya düşmez)
        Some("wfd_single_draft") =>
            WfdError::Conflict(format!("{name}: açık draft zaten var")),
        _ => WfdError::Database(e),
    })?;
    Ok(id)
}

/// Yalnızca published (is_active) satırı döner — mevcut çalıştırma yolu.
/// status='published' filtresi draft'ların engine'de koşmasını engeller.
pub async fn get_meta(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(&format!(
        "SELECT {COLS} FROM wf.wfd_meta \
                  WHERE wfd_id=$1 AND version=$2 AND is_active=true AND status='published'"
    ))
    .bind(wfd_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

/// Draft dahil herhangi bir satırı döner (is_active filtresi yok).
pub async fn get_meta_any(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(&format!(
        "SELECT {COLS} FROM wf.wfd_meta WHERE wfd_id=$1 AND version=$2"
    ))
    .bind(wfd_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

/// Liste — draft ve published birlikte döner (UI ayırır).
/// `project_id` verilirse o projeyle sınırlanır.
pub async fn list(
    pool: &PgPool,
    orgtnt_id: Uuid,
    project_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<Vec<WfdMeta>, WfdError> {
    sqlx::query_as::<_, WfdMeta>(&format!(
        "SELECT {COLS} FROM wf.wfd_meta \
                  WHERE orgtnt_id=$1 AND is_active=true \
                    AND ($4::uuid IS NULL OR project_id=$4) \
                  ORDER BY name, version DESC LIMIT $2 OFFSET $3"
    ))
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .bind(project_id)
    .fetch_all(pool)
    .await
    .map_err(WfdError::Database)
}

/// Versiyon sayacı proje kapsamındadır (name benzersizliği gibi).
pub async fn next_version(pool: &PgPool, project_id: Uuid, name: &str) -> Result<i32, WfdError> {
    let max: Option<i32> =
        sqlx::query_scalar("SELECT MAX(version) FROM wf.wfd_meta WHERE project_id=$1 AND name=$2")
            .bind(project_id)
            .bind(name)
            .fetch_one(pool)
            .await?;
    Ok(max.unwrap_or(0) + 1)
}

/// Draft metadata günceller (JSON storage'da; burada sadece meta + updated_at).
pub async fn update_draft(
    pool: &PgPool,
    wfd_id: Uuid,
    version: i32,
    description: Option<&str>,
    tags: Option<&[String]>,
) -> Result<(), WfdError> {
    // COALESCE: verilmeyen alan (NULL) mevcut değeri korur — editör kaydı
    // yalnızca JSON gönderdiğinden create'te girilen description/tags silinmez.
    let n = sqlx::query(
        "UPDATE wf.wfd_meta \
         SET description = COALESCE($3, description), \
             tags = COALESCE($4, tags), \
             updated_at = now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'",
    )
    .bind(wfd_id)
    .bind(version)
    .bind(description)
    .bind(tags)
    .execute(pool)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}")));
    }
    Ok(())
}

/// Aynı tenant + workflow name grubundaki tüm versiyonların görünen metadata'sını günceller.
/// WFD JSON immutable kalır; bu değerler katalog/detay ekranı için meta kaydından gelir.
pub async fn update_group_metadata(
    pool: &PgPool,
    anchor_wfd_id: Uuid,
    anchor_version: i32,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Vec<WfdMeta>, WfdError> {
    let rows = sqlx::query_as::<_, WfdMeta>(&format!(
        "WITH anchor AS (
             SELECT project_id, name AS old_name
             FROM wf.wfd_meta
             WHERE wfd_id = $1 AND version = $2 AND is_active = true
         ),
         updated AS (
             UPDATE wf.wfd_meta m
             SET name = COALESCE($3, m.name),
                 description = COALESCE($4, m.description),
                 updated_at = now()
             FROM anchor a
             WHERE m.project_id = a.project_id
               AND m.name = a.old_name
               AND m.is_active = true
             RETURNING {M_COLS}
         ),
         renamed_conns AS (
             -- Lokal DB bağlantılarının sahipliği (project_id, wfd_name)'dir; grup adı
             -- değişince onlar da taşınmalı, yoksa bağlantılar WFD'den kopar.
             -- Veri değiştiren CTE referans edilmese de bir kez ve tam olarak koşar.
             UPDATE wf.db_connection c
             SET wfd_name = COALESCE($3, c.wfd_name), updated_at = now()
             FROM anchor a
             WHERE c.scope = 'local'
               AND c.project_id = a.project_id
               AND c.wfd_name = a.old_name
             RETURNING 1
         )
         SELECT {COLS} FROM updated ORDER BY version DESC"
    ))
    .bind(anchor_wfd_id)
    .bind(anchor_version)
    .bind(name)
    .bind(description)
    .fetch_all(pool)
    .await
    .map_err(
        |e| match e.as_database_error().and_then(|d| d.constraint()) {
            Some("wfd_meta_project_name_version_key") | Some("wfd_single_draft") => {
                WfdError::Conflict("Bu isimde başka bir workflow zaten var".into())
            }
            _ => WfdError::Database(e),
        },
    )?;

    if rows.is_empty() {
        return Err(WfdError::NotFound(format!(
            "{anchor_wfd_id} v{anchor_version}"
        )));
    }
    Ok(rows)
}

/// Draft'ı onaya gönderir: draft → pending_approval (+ gönderen, eski ret notu silinir).
pub async fn set_pending(
    pool: &PgPool,
    wfd_id: Uuid,
    version: i32,
    submitted_by: &str,
) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET status='pending_approval', submitted_by=$3, review_note=NULL, updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version).bind(submitted_by)
    .execute(pool).await?.rows_affected();
    if n == 0 {
        return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}")));
    }
    Ok(())
}

/// Onay bekleyeni yayınlar: pending_approval → published.
pub async fn set_published_from_pending(
    pool: &PgPool,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET status='published', review_note=NULL, updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='pending_approval'",
    )
    .bind(wfd_id)
    .bind(version)
    .execute(pool)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(WfdError::NotFound(format!("pending {wfd_id} v{version}")));
    }
    Ok(())
}

/// Onay bekleyeni reddeder: pending_approval → draft (+ gerekçe).
pub async fn set_rejected(
    pool: &PgPool,
    wfd_id: Uuid,
    version: i32,
    note: Option<&str>,
) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET status='draft', review_note=$3, updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='pending_approval'",
    )
    .bind(wfd_id)
    .bind(version)
    .bind(note)
    .execute(pool)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(WfdError::NotFound(format!("pending {wfd_id} v{version}")));
    }
    Ok(())
}

/// Draft'ı published yapar (publish sonrası). status flip + updated_at.
pub async fn set_published(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET status='published', updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'",
    )
    .bind(wfd_id)
    .bind(version)
    .execute(pool)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}")));
    }
    Ok(())
}

/// Draft satırını siler (published silinemez).
pub async fn delete_draft(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<(), WfdError> {
    let mut tx = pool.begin().await?;
    // Silinen satırın grup kimliği: son versiyon da gidiyorsa gruba ait lokal DB
    // bağlantıları sahipsiz kalır (hiçbir WFD listelemez) → onlar da temizlenir.
    let owner = sqlx::query_as::<_, (Uuid, String)>(
        "DELETE FROM wf.wfd_meta WHERE wfd_id=$1 AND version=$2 AND status='draft' \
         RETURNING project_id, name",
    )
    .bind(wfd_id)
    .bind(version)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, name)) = owner else {
        return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}")));
    };
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM wf.wfd_meta WHERE project_id=$1 AND name=$2")
            .bind(project_id)
            .bind(&name)
            .fetch_one(&mut *tx)
            .await?;
    if remaining == 0 {
        sqlx::query(
            "DELETE FROM wf.db_connection \
             WHERE scope='local' AND project_id=$1 AND wfd_name=$2",
        )
        .bind(project_id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// WFC: dokümanın `id` (+ opsiyonel semver) alanından yayınlanmış satırı çözer.
///
/// `doc_version` verilmezse **en son yayınlanmış** satır seçilir (`version DESC`) —
/// `calls.<key>.version` boş bırakıldığında beklenen davranış budur. Yaratılan WFE
/// yine tek bir (wfd_id, version) çiftine sabitlenir; yani yeni sürüm yayınlamak
/// KOŞAN WFE'leri etkilemez.
pub async fn resolve_doc(
    pool: &PgPool,
    orgtnt_id: Uuid,
    doc_id: &str,
    doc_version: Option<&str>,
) -> Result<Option<(Uuid, i32)>, WfdError> {
    let row = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT wfd_id, version FROM wf.wfd_meta
          WHERE orgtnt_id = $1
            AND doc_id = $2
            AND ($3::text IS NULL OR doc_version = $3)
            AND status = 'published'
            AND is_active = true
          ORDER BY version DESC
          LIMIT 1",
    )
    .bind(orgtnt_id)
    .bind(doc_id)
    .bind(doc_version)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
