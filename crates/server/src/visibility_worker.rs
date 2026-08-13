//! Görünürlük yeniden-projeksiyon işçisi — kuyruğu (`wf.visibility_reprojection`)
//! partiler hâlinde tüketir.
//!
//! Mevcut saatlik süpürücünün içinde koşar (`reservation::spawn_sweeper`): ayrı
//! bir servis/zamanlayıcı eklemek, tek bir bakım döngüsü olan bu sunucuda ikinci
//! bir hayat döngüsü ve ikinci bir izleme yüzeyi demekti.
//!
//! Parti sınırı KASITLI: org yeniden yapılanması binlerce WFE'yi bayatlatabilir
//! ve her satır org portuna sorgu demektir. Tur başına `BATCH` satır işlenir,
//! ilerleme `wf.wfe.grants_built_at` damgasında kalıcıdır; bitmeyen iş bir sonraki
//! tura devreder (kuyruk satırı ancak tamamen bitince silinir). Yani hız yerine
//! ilerleme garantisi seçildi: yarıda kesilen bir tur işi baştan başlatmaz.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use wfe_core::v22::pipeline::Engine;

use crate::state::AppState;

/// Tur başına yeniden projelendirilecek en fazla WFE sayısı.
const BATCH: i64 = 500;

pub async fn run_once(s: &AppState) {
    let pending: Vec<(Uuid, DateTime<Utc>, String)> = match sqlx::query_as(
        "SELECT orgtnt_id, requested_at, reason FROM wf.visibility_reprojection
          ORDER BY requested_at ASC",
    )
    .fetch_all(&s.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            // Tablo yoksa (migration uygulanmadı) sessiz kalmak yanlış olurdu:
            // projeksiyon bayatlar ve kimse fark etmez.
            tracing::warn!("yeniden-projeksiyon kuyruğu okunamadı: {e}");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    let engine = Engine {
        org: &*s.executor.org,
        exec: &*s.executor.runner,
        env: Default::default(),
    };

    for (orgtnt_id, requested_at, reason) in pending {
        // `requested_at`ten ESKİ damgalı satırlar yeniden üretilir. İstek anını
        // eşik almak, tur sırasında commit edilen (dolayısıyla zaten TAZE
        // projeksiyonlu) satırları boşuna gezmeyi de önler.
        let done = match wf_wfe::reproject::reproject_tenant(
            &s.pool,
            &*s.wfd,
            &*s.executor.wfe,
            &engine,
            orgtnt_id,
            BATCH,
            requested_at,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(%orgtnt_id, reason, "yeniden projeksiyon başarısız: {e}");
                continue;
            }
        };

        if done < BATCH as usize {
            // Parti dolmadı → bu tenant'ta bayat satır kalmadı, kuyruktan düşer.
            let _ = sqlx::query("DELETE FROM wf.visibility_reprojection WHERE orgtnt_id = $1")
                .bind(orgtnt_id)
                .execute(&s.pool)
                .await;
            tracing::info!(%orgtnt_id, reason, "görünürlük projeksiyonu tazelendi ({done} WFE)");
        } else {
            // Parti doldu → iş sürüyor, satır KALIR ve bir sonraki tur devam eder.
            let _ = sqlx::query(
                "UPDATE wf.visibility_reprojection SET started_at = COALESCE(started_at, now())
                  WHERE orgtnt_id = $1",
            )
            .bind(orgtnt_id)
            .execute(&s.pool)
            .await;
            tracing::info!(
                %orgtnt_id, reason,
                "görünürlük projeksiyonu sürüyor ({done} WFE bu turda, devam edecek)"
            );
        }
    }
}
