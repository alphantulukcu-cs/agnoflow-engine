//! Görünürlük projeksiyonunun YENİDEN üretimi — tek kod yolu.
//!
//! Projeksiyon normalde commit'te yazılır (`WfeExecutor::fill_view_grants`), ama
//! iki durumda commit'ten BAĞIMSIZ olarak yeniden üretilmesi gerekir:
//!
//!   1. **Backfill** — kolonlar eklenmeden önce yaratılmış satırlar
//!      (`visibility_backfill` komutu).
//!   2. **Org ağacı değişimi** — grant'lar ORGTRVLANG selector'larını (`self`,
//!      `parent`, `*:[type:x]`) SOMUT `orgu_id` kümesine çözüp donduruyor. Bir
//!      birim taşınır/eklenir/pasifleşirse o küme eskir. Rol ATAMASI bu sınıfa
//!      GİRMEZ: satırlar kullanıcı değil (birim, rol) çifti tuttuğu için role
//!      sonradan atanan kişi işi anında görür — yeniden projeksiyon gerekmez.
//!
//! İkisi de AYNI fonksiyonu çağırır: "projeksiyon nasıl üretilir" sorusunun tek
//! bir cevabı olsun. Commit yolundaki `fill_view_grants` ile aynı kuralları
//! uygular; ayrışmayı `visibility_report` kontrat denetçisi yakalar.

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{BranchStatus, WfdStore, WfeStore};
use wfe_core::EngineError;

/// Tek WFE'nin yeniden projeksiyonu.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Yazıldı (kaç kol satırı güncellendi).
    Written { branches: usize },
    /// WFD çözülemedi (silinmiş/pasif sürüm) — projeksiyon üretilemez.
    WfdMissing,
    /// Çapa türetilemedi: `origin_orgu_id` boş VE insan WFAH kaydı yok. Yanlış
    /// çapayla yazmak görünürlüğü sessizce başka bir birime kaydırırdı.
    NoAnchor,
}

/// Bir WFE'nin `view_c_a` / `current_c_a` / kol `c_a` kolonlarını yeniden üretir.
///
/// `dry_run = true` iken hiçbir şey yazılmaz (backfill komutunun varsayılanı).
/// Çapa sırası: kolon → ilk İNSAN WFAH kaydının birimi. İkincisi eski satırlar
/// içindir: akışı başlatan kişi odur (sistem marker'ları bir birimi temsil etmez).
pub async fn reproject_wfe(
    pool: &PgPool,
    wfd_store: &dyn WfdStore,
    wfe_store: &dyn WfeStore,
    engine: &Engine<'_>,
    wfe_id: Uuid,
    dry_run: bool,
) -> Result<Outcome, EngineError> {
    let wfes = wfe_store.load(wfe_id).await?;
    let Ok(wfd) = wfd_store.fetch(wfes.wfd_id, wfes.wfd_version).await else {
        return Ok(Outcome::WfdMissing);
    };

    let anchor = wfes.origin_orgu_id.or_else(|| {
        wfes.wfah
            .entries()
            .iter()
            .find(|e| !e.actor.user_id.is_nil())
            .map(|e| e.actor.orgu_id)
    });
    let Some(origin) = anchor else {
        return Ok(Outcome::NoAnchor);
    };
    // Node/kol c_a'sı çözümünde aktörün YALNIZ birimi okunur (çapa); kimlik
    // alanları kuralın kişi kanalına girmez çünkü bu bir "kim eşleşir" sorusu
    // değil, "adaylar kimler" sorusudur.
    let anchor_actor = Actor {
        orgu_id: origin,
        user_id: Uuid::nil(),
        role: String::new(),
    };
    let ctx = wfes.dynctx.as_value();

    let view_c_a = engine
        .view_grants(
            &wfd,
            ctx,
            &wfes.wfah,
            wfes.current_node.as_deref(),
            wfes.wfe_id,
            origin,
            wfes.orgtnt_id,
        )
        .await?;

    // `current_c_a` yalnız AKTİF tek-kol satırda anlamlıdır; bitmiş işte kolon
    // boştur ve boş KALMALIDIR (bitmiş işi `view_c_a` gösterir).
    let current_c_a = match (&wfes.current_node, wfes.status) {
        (Some(node), WfeStatus::Active) => Some(
            engine
                .resolve_node_c_a(&wfd, node, ctx, &wfes.wfah, &anchor_actor, wfes.orgtnt_id)
                .await?,
        ),
        _ => None,
    };

    let mut branches = Vec::new();
    for b in wfes
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
    {
        let c_a = engine
            .resolve_node_c_a(
                &wfd,
                &b.branch_node,
                ctx,
                &wfes.wfah,
                &anchor_actor,
                wfes.orgtnt_id,
            )
            .await?;
        branches.push((b.branch_node.clone(), c_a));
    }

    if dry_run {
        return Ok(Outcome::Written {
            branches: branches.len(),
        });
    }

    let view_json = serde_json::to_value(&view_c_a).map_err(io_err)?;
    let current_json = match &current_c_a {
        Some(c) => Some(serde_json::to_value(c).map_err(io_err)?),
        None => None,
    };
    sqlx::query(
        "UPDATE wf.wfe
            SET view_c_a = $1, origin_orgu_id = $2, grants_built_at = now(),
                current_c_a = COALESCE($4, current_c_a)
          WHERE wfe_id = $3",
    )
    .bind(&view_json)
    .bind(origin)
    .bind(wfe_id)
    .bind(&current_json)
    .execute(pool)
    .await
    .map_err(io_err)?;

    for (node, c_a) in &branches {
        let c_a_json = serde_json::to_value(c_a).map_err(io_err)?;
        sqlx::query(
            "UPDATE wf.wfe_branch SET c_a = $1, updated_at = now()
              WHERE wfe_id = $2 AND branch_node = $3",
        )
        .bind(&c_a_json)
        .bind(wfe_id)
        .bind(node)
        .execute(pool)
        .await
        .map_err(io_err)?;
    }

    Ok(Outcome::Written {
        branches: branches.len(),
    })
}

fn io_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::WfePort(e.to_string())
}

/// Bir tenant'ın en eski projeksiyonlu WFE'lerinden `limit` tanesini yeniden
/// üretir; kaç satır işlendiğini döner.
///
/// `grants_built_at ASC NULLS FIRST` sırası kasıtlı: kuyruk her turda en bayat
/// satırdan başlar, böylece iş yarıda kalsa bile (deploy, hata) ilerleme kalıcıdır
/// ve aynı satırlar tekrar tekrar seçilmez. Bitmiş işler de dahildir — onların
/// `view_c_a`'sı hâlâ tek görünürlük kaynağıdır.
pub async fn reproject_tenant(
    pool: &PgPool,
    wfd_store: &dyn WfdStore,
    wfe_store: &dyn WfeStore,
    engine: &Engine<'_>,
    orgtnt_id: Uuid,
    limit: i64,
    since: chrono::DateTime<Utc>,
) -> Result<usize, EngineError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT wfe_id FROM wf.wfe
          WHERE orgtnt_id = $1 AND (grants_built_at IS NULL OR grants_built_at < $2)
          ORDER BY grants_built_at ASC NULLS FIRST
          LIMIT $3",
    )
    .bind(orgtnt_id)
    .bind(since)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(io_err)?;

    let mut n = 0usize;
    for wfe_id in ids {
        match reproject_wfe(pool, wfd_store, wfe_store, engine, wfe_id, false).await? {
            Outcome::Written { .. } => n += 1,
            // Projeksiyonu üretilemeyen satır kuyruğu TIKAMAMALI: damga
            // ilerletilir ki bir sonraki tur onu tekrar seçmesin. Satır zaten
            // görünmez (boş grant) ve `visibility_report` onu ayrıca raporlar.
            skipped => {
                tracing::warn!(%wfe_id, ?skipped, "yeniden projeksiyon atlandı");
                sqlx::query("UPDATE wf.wfe SET grants_built_at = now() WHERE wfe_id = $1")
                    .bind(wfe_id)
                    .execute(pool)
                    .await
                    .map_err(io_err)?;
            }
        }
    }
    Ok(n)
}
