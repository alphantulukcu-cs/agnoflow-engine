//! Org ağacı değişince görünürlük projeksiyonunu bayatlamış işaretleyen kuyruk.
//!
//! Bkz. `migrations/wf/20260813000002_visibility_reprojection_queue.sql` (neden
//! kuyruk, neden tenant başına tek satır) ve `wf_wfe::reproject` (işin kendisi).

use sqlx::PgPool;
use uuid::Uuid;

/// Kuyruğa yazar (tenant başına tek satır; tekrar istek `requested_at`i tazeler).
///
/// HATA YUTULUR — bilinçli: org mutasyonu kuyruk yazımı yüzünden BAŞARISIZ
/// OLMAMALI. Birim taşıma işlemi tamamlanmışsa geri alınamaz; kuyruk satırı
/// yazılamadıysa doğru davranış yüksek sesle log basıp devam etmektir (operatör
/// `visibility_backfill --apply` ile elle de tetikleyebilir). Ters seçim, org
/// yönetimini görünürlük altyapısının uptime'ına bağlardı.
pub async fn enqueue(pool: &PgPool, orgtnt_id: Uuid, reason: &str) {
    let res = sqlx::query(
        "INSERT INTO wf.visibility_reprojection (orgtnt_id, reason)
         VALUES ($1, $2)
         ON CONFLICT (orgtnt_id) DO UPDATE
            SET requested_at = now(), reason = $2, started_at = NULL",
    )
    .bind(orgtnt_id)
    .bind(reason)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(
            %orgtnt_id, reason,
            "görünürlük yeniden-projeksiyon kuyruğuna yazılamadı: {e} \
             (org değişikliği YAPILDI; projeksiyon elle tetiklenmeli)"
        );
    }
}

/// `orgu_id`den tenant'ı çözüp kuyruğa yazar — org uçlarının çoğu elinde
/// yalnız birim id'si tutuyor.
pub async fn enqueue_for_orgu(pool: &PgPool, orgu_id: Uuid, reason: &str) {
    match wf_org::repo::orgu::get_orgtnt_id(pool, orgu_id).await {
        Ok(orgtnt_id) => enqueue(pool, orgtnt_id, reason).await,
        Err(e) => tracing::warn!(%orgu_id, "tenant çözülemedi, kuyruğa yazılamadı: {e}"),
    }
}
