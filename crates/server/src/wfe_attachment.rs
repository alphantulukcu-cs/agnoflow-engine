//! Ek-belge METADATA'sı — `wf.wfe_attachment` (2026-08-11, K7).
//!
//! Bugüne kadar bir dosyanın DB'de hiçbir kaydı yoktu; tek gerçeklik storage'daki nesneydi.
//! Bu modül tek SQL'lik bir kapı kontrolü ve bedava audit sağlar: kim, ne zaman, hangi ad
//! ve boyutla yükledi sorusunun cevabı artık DB'de.
//!
//! Tablo FK ile `wf.wfe`'ye `ON DELETE CASCADE` bağlıdır (bkz. migration'daki NEDEN yorumu):
//! satırlar WFE commit olduktan SONRA, ayrı bir adımda yazılır — `wf_wfe` crate'inin
//! `WfeStore::commit` transaction'ına bu crate katılamaz. FK yalnız "satır varsa WFE
//! vardır" garantisini korur; "metadata WFE ile aynı anda yazılır" garantisini vermez.
//! Bu yüzden `insert_many` hatası WFE'yi geri almaz — çağıran yer (`routes::wfe`) bunu
//! `tracing::warn!` ile loglayıp başarı cevabını döndürmeye devam eder.

use crate::error::AppError;
use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

/// Yazılacak tek dosya satırı. `version` burada YOK: `insert_many` her satır için o
/// (wfe_id, grp, item) için mevcut en yüksek version'ı kendisi hesaplar.
pub struct AttachmentRow {
    pub wfe_id: Uuid,
    pub grp: String,
    pub item: String,
    pub storage_key: String,
    pub filename: Option<String>,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub uploaded_by: Uuid,
}

/// Okuma tarafı — kapı ve gösterim uçlarının gördüğü biçim.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct AttachmentMeta {
    pub grp: String,
    pub item: String,
    pub version: i32,
    pub filename: Option<String>,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub uploaded_by: Uuid,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
}

fn db_err(e: sqlx::Error) -> AppError {
    AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}

/// TEK transaction'da yazar. Her satır için `version` = o (wfe_id,grp,item) için mevcut
/// EN YÜKSEK version + 1 — aynı slota tekrar yükleme ÜZERİNE YAZMAZ, yeni sürüm açar
/// (denetimde "karar anında hangi belge oradaydı" sorusu cevaplanabilsin). Boş dizide
/// hiç sorgu açmadan (transaction bile başlatmadan) `Ok(())` döner — çağıran yer (tek
/// istekte başlatma) bazen sıfır dosyalı bir aksiyonu da bu yoldan geçirir.
pub async fn insert_many(pool: &PgPool, rows: &[AttachmentRow]) -> Result<(), AppError> {
    if rows.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await.map_err(db_err)?;
    for r in rows {
        let version: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) + 1 \
               FROM wf.wfe_attachment \
              WHERE wfe_id = $1 AND grp = $2 AND item = $3",
        )
        .bind(r.wfe_id)
        .bind(&r.grp)
        .bind(&r.item)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        sqlx::query(
            "INSERT INTO wf.wfe_attachment \
               (wfe_id, grp, item, version, storage_key, filename, content_type, size_bytes, sha256, uploaded_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(r.wfe_id)
        .bind(&r.grp)
        .bind(&r.item)
        .bind(version)
        .bind(&r.storage_key)
        .bind(&r.filename)
        .bind(&r.content_type)
        .bind(r.size_bytes)
        .bind(&r.sha256)
        .bind(r.uploaded_by)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(())
}

/// Her (grp,item) için EN YÜKSEK version'ı döner. Okuma uçları (kapı kontrolü, gösterim)
/// bunu kullanır — eski sürümler audit amaçlı DB'de kalır ama varsayılan görünüme girmez.
pub async fn list_by_wfe(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<AttachmentMeta>, AppError> {
    sqlx::query_as::<_, AttachmentMeta>(
        "SELECT DISTINCT ON (grp, item) \
                grp, item, version, filename, content_type, size_bytes, sha256, uploaded_by, uploaded_at \
           FROM wf.wfe_attachment \
          WHERE wfe_id = $1 \
          ORDER BY grp, item, version DESC",
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

// Elle silme fonksiyonu KASITEN yok: `wfe_id` FK'sı `ON DELETE CASCADE` taşıyor, WFE
// silinince satırlar kendiliğinden gider. İkinci bir silme yolu açmak, "satır varsa WFE
// vardır" değişmezini iki yerden korumak demekti.
