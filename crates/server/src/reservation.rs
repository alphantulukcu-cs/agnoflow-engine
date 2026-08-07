//! wfe_id REZERVASYONU — başlatma öncesi ek-belge yüklemesi (2026-08-07).
//!
//! Dosya anahtarı `attachments/{wfe_id}/{grup}/{item}`; wfe_id ise eskiden akış
//! başlarken doğardı. Bu yüzden "belgeler yüklenmeden akış başlamasın" kuralı BAŞLATMA
//! aksiyonunda sunucuda zorlanamıyordu. Çözüm sırayı tersine çevirir:
//!
//! ```text
//! POST /wfe/reserve                     → wfe_id (DB'de wfe satırı YOK, rezervasyon var)
//! PUT  /wfe/{wfe_id}/attachments/g/i    → dosyalar NİHAİ anahtarına yazılır
//! POST /wfe { …, wfe_id }               → engine depoya bakar; eksikse 422, WFE HİÇ oluşmaz
//! ```
//!
//! Rezervasyon satırı iki soruyu cevaplar: (1) yükleme rotası dosyayı hangi WFD'nin
//! kataloguna göre doğrulayacak, (2) süpürücü hangi dosyaların sahipsiz kaldığını nereden
//! bilecek. Başlatma başarılı olunca satır silinir (wfe artık gerçek).

use crate::error::AppError;
use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Reservation {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub wfd_id: Uuid,
    pub wfd_version: i32,
    pub environment_id: Option<Uuid>,
    pub actor_orgu_id: Uuid,
    pub actor_user_id: Uuid,
}

/// Süresi dolmuş sayılan rezervasyon yaşı. Kullanıcı belgeleri yükleyip başlatana kadar
/// geçen makul süre; aşıldığında satır ve dosyaları süpürülür.
pub const TTL_HOURS: i64 = 24;

pub async fn create(
    pool: &PgPool,
    r: &Reservation,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO wf.wfe_reservation \
           (wfe_id, orgtnt_id, wfd_id, wfd_version, environment_id, actor_orgu_id, actor_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(r.wfe_id)
    .bind(r.orgtnt_id)
    .bind(r.wfd_id)
    .bind(r.wfd_version)
    .bind(r.environment_id)
    .bind(r.actor_orgu_id)
    .bind(r.actor_user_id)
    .execute(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(())
}

pub async fn get(pool: &PgPool, wfe_id: Uuid) -> Result<Option<Reservation>, AppError> {
    sqlx::query_as::<_, Reservation>(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, environment_id, actor_orgu_id, actor_user_id \
           FROM wf.wfe_reservation WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}

pub async fn delete(pool: &PgPool, wfe_id: Uuid) -> Result<(), AppError> {
    sqlx::query("DELETE FROM wf.wfe_reservation WHERE wfe_id = $1")
        .bind(wfe_id)
        .execute(pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(())
}

/// Rezervasyon aynı tenant'ın ve aynı aktörün mü? Başkasının rezervasyonuna dosya
/// yazmayı (ya da onun rezervasyonuyla akış başlatmayı) engeller.
pub fn owned_by(r: &Reservation, orgtnt_id: Uuid, actor: &wfe_core::types::actor::Actor) -> bool {
    r.orgtnt_id == orgtnt_id && r.actor_orgu_id == actor.orgu_id && r.actor_user_id == actor.user_id
}

/// Süresi geçmiş rezervasyonları döndürür (süpürücü önce dosyaları siler, sonra satırı).
pub async fn expired(pool: &PgPool) -> Result<Vec<Reservation>, sqlx::Error> {
    sqlx::query_as::<_, Reservation>(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, environment_id, actor_orgu_id, actor_user_id \
           FROM wf.wfe_reservation \
          WHERE created_at < now() - ($1 || ' hours')::interval",
    )
    .bind(TTL_HOURS.to_string())
    .fetch_all(pool)
    .await
}

/// Süresi geçmiş rezervasyonları ve DOSYALARINI temizler. Sunucu açılışında bir kez,
/// sonra saatte bir koşar (`spawn_sweeper`).
///
/// Sıra önemlidir: önce dosyalar, sonra satır. Ters sırada satır silinip dosya silme
/// başarısız olsaydı dosyaların kime ait olduğu bir daha bilinemez, depoda sonsuza dek
/// kalırlardı.
pub async fn sweep(state: &crate::state::AppState) -> usize {
    let rows = match expired(&state.pool).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("rezervasyon süpürmesi okunamadı: {e}");
            return 0;
        }
    };
    let mut swept = 0usize;
    for r in rows {
        // Depo WFD başına çözülür ($env) — dosyalar hangi bucket'a yazıldıysa oradan silinir.
        let store =
            match crate::attachment_store::store_for_wfd(state, r.wfd_id, r.orgtnt_id, r.environment_id)
                .await
            {
                Ok(store) => store,
                Err(e) => {
                    tracing::warn!(wfe_id = %r.wfe_id, "rezervasyon deposu çözülemedi: {}", e.message);
                    continue;
                }
            };
        if let Err(e) = store.remove_all(r.wfe_id).await {
            tracing::warn!(wfe_id = %r.wfe_id, "rezervasyon dosyaları silinemedi: {e}");
            continue;
        }
        if let Err(e) = delete(&state.pool, r.wfe_id).await {
            tracing::warn!(wfe_id = %r.wfe_id, "rezervasyon satırı silinemedi: {}", e.message);
            continue;
        }
        swept += 1;
    }
    swept
}

/// Saatlik süpürücüyü başlatır. TTL 24 saat olduğu için sıklık kritik değil —
/// gecikmiş bir tur yalnız temizliği erteler, veri kaybetmez.
pub fn spawn_sweeper(state: crate::state::AppState) {
    tokio::spawn(async move {
        loop {
            let n = sweep(&state).await;
            if n > 0 {
                tracing::info!("{n} süresi geçmiş wfe rezervasyonu temizlendi");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
}
