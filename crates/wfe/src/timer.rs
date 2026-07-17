//! Event-driven SLA timer servisi (M5/M6 — WOR-46/47, 2026-07-17).
//!
//! Eski 60 sn'lik kör poll döngüsünün yerine: aktif WFE'lerin en yakın SLA
//! vadesi (deadline / claim_timeout / sıradaki escalation) hesaplanır ve tam
//! o ana kadar uyunur. `WfeExecutor` her kalıcı mutasyonda (create/commit/claim)
//! `Notify` kanalını dürter; servis uyanıp vadeyi yeniden hesaplar. Böylece
//! 5 sn'lik bir SLA ~5 sn'de ateşlenir, 60 sn'ye kadar sarkmaz.
//!
//! Güvenlik ağı: uyku süresi `FALLBACK_SWEEP` (60 sn) ile tavanlanır — sinyal
//! kaçarsa/DB dışı bir yoldan durum değişirse en geç 60 sn'de süpürülür
//! (eski davranışla aynı taban garanti). Ateşleme mantığı DEĞİŞMEDİ:
//! her uyanışta mevcut `tick_timers` aynı sırayla çalışır.

use crate::executor::WfeExecutor;
use crate::repo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use wfe_core::EngineError;

/// Sinyal kaçaklarına karşı azami uyku — eski periyodik sweep aralığı.
pub const FALLBACK_SWEEP: Duration = Duration::from_secs(60);

/// Asgari uyku — vadesi geçmiş ama ateşlenemeyen (ör. geçici DB/WFD hatası)
/// bir kayıt sıcak döngüye (busy-loop) dönmesin diye taban.
const MIN_SLEEP: Duration = Duration::from_millis(500);

/// Aktif WFE id kaynağı — prod'da `PgPool` (`repo::wfe::list_active_ids`),
/// testte in-memory store. Timer servisini DB'den ayrıştırır.
#[async_trait]
pub trait ActiveWfeSource: Send + Sync {
    async fn active_ids(&self) -> Result<Vec<Uuid>, EngineError>;
}

#[async_trait]
impl ActiveWfeSource for PgPool {
    async fn active_ids(&self) -> Result<Vec<Uuid>, EngineError> {
        repo::wfe::list_active_ids(self)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }
}

/// Servis gövdesi — `tokio::spawn` ile çalıştırılır, hiç dönmez.
///
/// Döngü: süpür → en yakın vadeyi hesapla → `sleep(vade)` VEYA executor
/// sinyali (hangisi önce). `Notify::notify_one` permit biriktirdiği için
/// süpürme sırasında gelen sinyal kaybolmaz (bir sonraki `notified()` anında
/// döner).
pub async fn run_timer_service(executor: Arc<WfeExecutor>, source: Arc<dyn ActiveWfeSource>) {
    let signal = executor.timer_signal();
    loop {
        sweep_once(&executor, &*source).await;
        let sleep_for = next_sleep(&executor, &*source).await;
        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = signal.notified() => {}
        }
    }
}

/// Tek süpürme turu — eski 60 sn'lik döngünün gövdesiyle birebir aynı:
/// tüm aktif WFE'lerde `tick_timers` (deadline → claim_timeout → escalation).
pub async fn sweep_once(executor: &WfeExecutor, source: &dyn ActiveWfeSource) {
    let ids = match source.active_ids().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("timer sweep: aktif WFE listesi alınamadı: {e}");
            return;
        }
    };
    for wfe_id in ids {
        match executor.tick_timers(wfe_id).await {
            Ok(true) => tracing::info!("timer fired for wfe {wfe_id}"),
            Ok(false) => {}
            Err(e) => tracing::warn!("timer sweep {wfe_id}: {e}"),
        }
    }
}

/// Bir sonraki uyanışa kadar uyku süresi: aktif WFE'lerin `next_timer_due`
/// min'i, `[MIN_SLEEP, FALLBACK_SWEEP]` aralığına kıstırılır. Vade yoksa veya
/// hesap hatası olursa güvenlik ağı aralığı (FALLBACK_SWEEP) kullanılır.
async fn next_sleep(executor: &WfeExecutor, source: &dyn ActiveWfeSource) -> Duration {
    let ids = match source.active_ids().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("timer next-due: aktif WFE listesi alınamadı: {e}");
            return FALLBACK_SWEEP;
        }
    };
    let now = Utc::now();
    let mut min_due: Option<DateTime<Utc>> = None;
    for wfe_id in ids {
        match executor.next_timer_due(wfe_id, now).await {
            Ok(Some(due)) => {
                min_due = Some(match min_due {
                    Some(m) => m.min(due),
                    None => due,
                });
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("timer next-due {wfe_id}: {e}"),
        }
    }
    match min_due {
        // Geçmiş vade `to_std` hatası verir → ZERO → MIN_SLEEP tabanına kıstırılır.
        Some(due) => (due - Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO)
            .clamp(MIN_SLEEP, FALLBACK_SWEEP),
        None => FALLBACK_SWEEP,
    }
}
