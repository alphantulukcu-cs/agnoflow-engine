//! wfe_id REZERVASYONU — bugün yalnız CRASH AĞI (2026-08-07 → 2026-08-11).
//!
//! Dosya anahtarı `attachments/{wfe_id}/{grup}/{item}`; wfe_id ise eskiden akış başlarken
//! doğardı. "Belgeler yüklenmeden akış başlamasın" kuralı bu yüzden başlatmada sunucuda
//! zorlanamıyordu ve 2026-08-07'de sıra tersine çevrilmişti: istemci `POST /wfe/reserve`
//! ile id alır, dosyaları o id'nin altına yükler, sonra o id ile başlatırdı.
//!
//! **2026-08-11: o üç ucun HTTP karşılığı KALDIRILDI.** Rezervasyon dışarıya açılmıyor;
//! tek istekli multipart `POST /wfe` id'yi kendi içinde üretiyor ve hata yollarında
//! yazdığını kendi siliyor.
//!
//! Modül yine de yaşıyor, çünkü o istek sırasında yazılan satırın TEK bir işlevi var:
//! **süreç isteğin ORTASINDA ölürse** (deploy/OOM/kill) `remove_all` çağrılamaz ve
//! yazılmış baytları kimse bilemez. Satır, süpürücüye "bu id'nin altındaki dosyalar
//! sahipsiz" diyen tek kayıttır. İstemciye HİÇ görünmez; başarıda silinir.

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

/// Rezervasyonu DOSYALARIYLA birlikte bırakır: önce depo, sonra defter satırı.
///
/// Sıra önemlidir: ters sırada satır silinip dosya silme başarısız olsaydı dosyaların
/// kime ait olduğu bir daha bilinemez, depoda sonsuza dek kalırlardı.
///
/// İki çağıran var: (1) süpürücü (TTL dolmuş ya da süreç istek ortasında ölmüş),
/// (2) tek istekli başlatmanın hata yolu (`routes::wfe::start_multipart_committed`).
/// İkisi de aynı soruyu sorar: bu id'nin altındaki dosyaların bağlanacağı bir WFE artık
/// olmayacak. (`DELETE /wfe/reserve/{id}` ucu 2026-08-11'de kaldırıldı.)
pub async fn release(
    state: &crate::state::AppState,
    r: &Reservation,
) -> Result<(), AppError> {
    // Depo WFD başına çözülür ($env) — dosyalar hangi bucket'a yazıldıysa oradan silinir.
    let store =
        crate::attachment_store::store_for_wfd(state, r.wfd_id, r.orgtnt_id, r.environment_id)
            .await?;
    store.remove_all(r.wfe_id).await.map_err(|e| {
        AppError(
            format!("rezervasyon dosyaları silinemedi: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    delete(&state.pool, r.wfe_id).await
}

/// Süresi geçmiş rezervasyonları ve DOSYALARINI temizler. Sunucu açılışında bir kez,
/// sonra saatte bir koşar (`spawn_sweeper`).
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
        if let Err(e) = release(state, &r).await {
            tracing::warn!(wfe_id = %r.wfe_id, "rezervasyon bırakılamadı: {}", e.message);
            continue;
        }
        swept += 1;
    }

    // WFE not defteri (K5): yetim draft'lar — kullanıcı yayınlamadan/silmeden
    // vazgeçtiği taslaklar. TTL 24 saat; Faz 2'den beri dosyaları da silinir
    // (bkz. `notes::sweep_expired_drafts`). Rezervasyon süpürmesinin sonucunu
    // (swept) ETKİLEMEZ.
    match crate::notes::sweep_expired_drafts(state).await {
        Ok(n) if n > 0 => tracing::info!("{n} süresi geçmiş taslak not temizlendi"),
        Ok(_) => {}
        Err(e) => tracing::warn!("taslak not süpürmesi başarısız: {}", e.message),
    }

    // Staging (Faz 3, K8): başlatmaya hiç girmemiş yüklemeler. Rezervasyonla AYNI
    // gerekçe — nesne var, sahibi yok. Kendi TTL'i var (`staging::TTL_HOURS`) ve
    // `swept` sayacını ETKİLEMEZ.
    match crate::staging::sweep_expired(state).await {
        Ok(n) if n > 0 => tracing::info!("{n} süresi geçmiş staging yüklemesi temizlendi"),
        Ok(_) => {}
        Err(e) => tracing::warn!("staging süpürmesi başarısız: {}", e.message),
    }

    // Tek istekli başlatma dedupe defteri (K6): fiziksel TTL 1 saat, `window_secs`
    // (tazelik penceresi) ile AYRI bir eksen — bkz. `start_dedupe.rs`. Hata warn'lanır,
    // `swept` sayacını ETKİLEMEZ.
    match crate::start_dedupe::sweep_expired(&state.pool).await {
        Ok(n) if n > 0 => tracing::info!("{n} süresi geçmiş başlatma dedupe kaydı temizlendi"),
        Ok(_) => {}
        Err(e) => tracing::warn!("başlatma dedupe süpürmesi başarısız: {}", e.message),
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
