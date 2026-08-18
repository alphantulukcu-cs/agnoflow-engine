//! Görünürlük KONTRAT denetleyicisi (salt okuma, hiçbir şey yazmaz).
//!
//! 2026-08-13'ten önce bu araç eski/yeni KURALI karşılaştırıyordu (o geçiş bitti,
//! ölçüm raporu: 205 → 148 erişim). Bugünkü işi kuralın İKİ OKUMASINI
//! karşılaştırmak:
//!   * **projeksiyon** — `wf.wfe.view_c_a`/`current_c_a`/`current_view_c_a`/
//!     `end_view_c_a` ve kol `c_a`/`view_c_a` üzerinde jsonb containment
//!     (`wf_wfe::visibility::sql`). Liste ucu, detay kapısı VE portal havuzu
//!     (2026-08-14'ten beri, `routes::portal::pool`) bunu koşar → havuz ayrıca
//!     ölçülmez, aynı parçayı ödünç aldığı için bu raporun kapsamındadır.
//!     Havuzun kendi süzgeçleri (tenant, `status='active'`, `deadline`,
//!     `current_node IS NOT NULL`) görünürlük DEĞİLDİR: "bu satır bir havuz
//!     görevi mi" sorusunu sorarlar, kontrat onları kapsamaz.
//!   * **belge** — `wfe_core::v22::visibility::can_view`, WFD + org portu ile
//!     canlı hesap. Sim ve birim testlerinin yolu.
//! İkisi AYNI kuralı ifade eder; ayrışırlarsa projeksiyon eskimiştir (backfill
//! koşmadı, org ağacı değişti) ya da kural iki yerden birinde güncellenmiştir.
//! Bu araç farkı satır satır basar — DB'li test koşulmayan bu repoda kontratın
//! bekçisi budur.
//!
//! ESKİ BAŞLIK (ölçüm modu) — kural değişiminin faturası:
//!
//! Yeni kural (onaylandı, 2026-08-13):
//! ```text
//! görünür(WFE, viewer) :=
//!      listable/wf_admin grant'i eşleşir           -- KALICI, `when` uygulanmış
//!   OR varılan terminal'in listable'ı eşleşir     -- KALICI, SONUCA BAĞLI (g)
//!   OR (status = 'active' AND (node c_a       eşleşir
//!                           OR node listable  eşleşir  -- DURUMA BAĞLI (f)
//!                           OR WFE/kol claim'i viewer'da))
//! ```
//! Eski kural = `wfe_core::v22::visibility::can_view` (kriter (b) KATILIMCI dahil).
//!
//! Bu araç iki soruyu cevaplar:
//!   1. **Erişim farkı**: hangi (aktör, WFE) çifti eski kuralda görünürken yeni
//!      kuralda görünmez oluyor (ve tersi). Anahtar rakam: kaybedilen erişim.
//!   2. **Projeksiyon sağlamlığı**: `listable`/`wf_admin`, node `listable` VE
//!      terminal `listable` kuralları VIEWER'DAN
//!      BAĞIMSIZ mı? Grant'lar commit anında (viewer bilinmezken) yazılacağı için
//!      viewer'a bağlı iki form projeksiyona SIĞMAZ:
//!        - `c_orgu` düz Selector: `resolve_c_orgu`ya default anchor olarak
//!          VIEWER'ın birimi girer (`resolver.rs:36`) → her viewer için farklı küme.
//!        - `when` içinde `$actor`: `matches_grant_rules` guard'ı viewer ile
//!          değerlendirir (`grants.rs`) → her viewer için farklı sonuç.
//!      Bu formları kullanan WFD varsa, ya validator kapısı gerekir ya da o
//!      belgelere özel canlı değerlendirme. Rapor onları tek tek sayar.
//!
//! Koşum: `DATABASE_URL=... cargo run -p wf-server --bin visibility_report`

use std::sync::Arc;

use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::{COrgu, CaGrantRule, Wfd};
use wfe_core::v22::grants::matches_grant_rules;
use wfe_core::v22::matcher::{authorize_or_delegated, MatchEnv};
use wfe_core::v22::ports::{VisibilityPort, WfdStore, WfeStore};
use wfe_core::v22::visibility::can_view;
use wfe_core::{EngineError, OrgPort};

#[derive(sqlx::FromRow)]
struct ActorRow {
    user_id: Uuid,
    full_name: String,
    orgu_id: Uuid,
    orgu_name: String,
    role: String,
}

#[derive(sqlx::FromRow)]
struct WfeIdRow {
    wfe_id: Uuid,
    orgtnt_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    status: String,
}

/// Projeksiyona SIĞMAYAN grant kuralları — bkz. dosya başlığı (2. soru).
fn viewer_relative_reasons(rules: &[CaGrantRule], kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        if let Some(COrgu::Selector(expr)) = &rule.c_a.c_orgu {
            out.push(format!(
                "{kind}[{i}].c_a.c_orgu düz Selector (\"{expr}\") — anchor VIEWER'ın birimi"
            ));
        }
        if let Some(when) = &rule.when {
            if when.contains("$actor") {
                out.push(format!(
                    "{kind}[{i}].when içinde $actor — guard viewer'a bağlı"
                ));
            }
        }
    }
    out
}

#[tokio::main]
async fn main() {
    let db = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(|c, _| {
            Box::pin(async move {
                c.execute("SET search_path TO org, public").await?;
                Ok(())
            })
        })
        .connect(&db)
        .await
        .expect("db connect");

    // `wf-server` binary-only crate (lib.rs yok) → config sunucuyla AYNI env'den,
    // yalnız ihtiyaç duyulan parça: WFD JSON deposu.
    let storage = wf_wfd::build_operator(&wf_wfd::StorageConfig::from_env()).expect("storage init");
    let wfd_store = Arc::new(wf_wfd::WfdAdapter::new(pool.clone(), storage));
    let wfe_store = wf_wfe::WfeAdapter::new(pool.clone());
    let org: Arc<dyn OrgPort> = Arc::new(wf_wfe::OrgAdapter::new(pool.clone()));

    let wfes_rows: Vec<WfeIdRow> = sqlx::query_as(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, status FROM wf.wfe ORDER BY created_at",
    )
    .fetch_all(&pool)
    .await
    .expect("wfe list");

    println!("=== GÖRÜNÜRLÜK KURALI DEĞİŞİM RAPORU ===");
    println!("WFE sayısı: {}\n", wfes_rows.len());

    // ---- 2. soru: projeksiyon sağlamlığı (WFD başına, tekilleştirilmiş) ----
    let mut seen_wfd = std::collections::HashSet::new();
    let mut unsound = 0usize;
    let mut missing_wfd = Vec::new();
    println!("--- Projeksiyona sığmayan grant kuralları ---");
    for row in &wfes_rows {
        if !seen_wfd.insert((row.wfd_id, row.wfd_version)) {
            continue;
        }
        match wfd_store.fetch(row.wfd_id, row.wfd_version).await {
            Ok(wfd) => {
                let mut reasons = viewer_relative_reasons(&wfd.listable, "listable");
                reasons.extend(viewer_relative_reasons(&wfd.wf_admin, "wf_admin"));
                // 2026-08-13 node listable: kök `listable` ile AYNI şekil, AYNI
                // çapa, AYNI projeksiyon kısıtı → aynı tarama. Kapsanmazsa
                // rapor SAPAR: viewer'a bağlı bir node kuralı `can_view` (f)'de
                // eşleşir ama `current_view_c_a` kolonunda karşılığı olmaz ve
                // "belgede VAR, projeksiyonda YOK" satırının sebebi görünmez
                // kalırdı. Node anahtarları sıralı gezilir (rapor deterministik).
                let mut node_keys: Vec<&String> = wfd.nodes.keys().collect();
                node_keys.sort();
                for key in node_keys {
                    let node = &wfd.nodes[key];
                    if node.listable.is_empty() {
                        continue;
                    }
                    reasons.extend(viewer_relative_reasons(
                        &node.listable,
                        &format!("nodes.{key}.listable"),
                    ));
                }
                // 2026-08-17 terminal listable: yine AYNI şekil/çapa/kısıt →
                // aynı tarama. `terminals[]` zaten belgedeki sırayı taşıyor,
                // ayrıca sıralamaya gerek yok (rapor deterministik kalır).
                for t in &wfd.terminals {
                    if t.listable.is_empty() {
                        continue;
                    }
                    reasons.extend(viewer_relative_reasons(
                        &t.listable,
                        &format!("terminals.{}.listable", t.id),
                    ));
                }
                if !reasons.is_empty() {
                    unsound += 1;
                    println!("  WFD {} v{}:", row.wfd_id, row.wfd_version);
                    for r in reasons {
                        println!("      - {r}");
                    }
                }
            }
            Err(e) => missing_wfd.push(format!("{} v{}: {e}", row.wfd_id, row.wfd_version)),
        }
    }
    if unsound == 0 {
        println!(
            "  (yok — tüm listable/wf_admin/node/terminal listable kuralları viewer'dan \
             bağımsız, projeksiyon sağlam)"
        );
    }
    if !missing_wfd.is_empty() {
        println!("\n--- WFD'si çözülemeyen (öksüz) satırlar ---");
        for m in &missing_wfd {
            println!("  {m}");
        }
    }

    // ---- 1. soru: erişim farkı ----
    let tenants: Vec<Uuid> = {
        let mut t: Vec<Uuid> = wfes_rows.iter().map(|r| r.orgtnt_id).collect();
        t.sort_unstable();
        t.dedup();
        t
    };

    let mut lost: Vec<String> = Vec::new();
    let mut gained: Vec<String> = Vec::new();
    let mut old_total = 0usize;
    let mut new_total = 0usize;

    for tenant in tenants {
        let actors: Vec<ActorRow> = sqlx::query_as(
            "SELECT u.u_id AS user_id, u.full_name, o.orgu_id, o.name AS orgu_name, r.name AS role
               FROM org.ur ur
               JOIN org.u u    ON ur.u_id = u.u_id
               JOIN org.orgu o ON ur.orgu_id = o.orgu_id
               JOIN org.r r    ON ur.r_id = r.r_id
              WHERE ur.orgtnt_id = $1 AND ur.ur_type <> 'excluded'
                AND u.is_active = true AND r.is_active = true
              ORDER BY o.name, u.full_name, r.name",
        )
        .bind(tenant)
        .fetch_all(&pool)
        .await
        .expect("actor list");

        for row in wfes_rows.iter().filter(|r| r.orgtnt_id == tenant) {
            let Ok(wfd) = wfd_store.fetch(row.wfd_id, row.wfd_version).await else {
                continue; // öksüz — yukarıda ayrıca raporlandı
            };
            let Ok(wfes) = wfe_store.load(row.wfe_id).await else {
                continue;
            };
            for a in &actors {
                let viewer = Actor {
                    orgu_id: a.orgu_id,
                    user_id: a.user_id,
                    role: a.role.clone(),
                };
                // Belge okuması (referans) vs projeksiyon okuması (üretim yolu).
                let old = can_view(&wfd, &wfes, &viewer, &*org).await.unwrap_or(false);
                let filters = wf_wfe::visibility::ViewerFilters::build(&viewer, &*org)
                    .await
                    .expect("filters");
                let new = wfe_store
                    .can_view_projection(row.wfe_id, &filters.as_binds())
                    .await
                    .unwrap_or(false);
                old_total += old as usize;
                new_total += new as usize;
                if old && !new {
                    lost.push(format!(
                        "  {} ({}/{}) → WFE {} [{}]",
                        a.full_name,
                        a.orgu_name,
                        a.role,
                        &row.wfe_id.to_string()[..8],
                        row.status
                    ));
                } else if !old && new {
                    gained.push(format!(
                        "  {} ({}/{}) → WFE {} [{}]",
                        a.full_name,
                        a.orgu_name,
                        a.role,
                        &row.wfe_id.to_string()[..8],
                        row.status
                    ));
                }
            }
        }
    }

    println!("\n--- Kontrat: belge okuması vs projeksiyon ---");
    println!("belge (can_view)      görünür (aktör×WFE): {old_total}");
    println!("projeksiyon (SQL)     görünür (aktör×WFE): {new_total}");
    println!(
        "\n{} belgede VAR, projeksiyonda YOK:",
        lost.len()
    );
    for l in lost.iter().take(60) {
        println!("{l}");
    }
    if lost.len() > 60 {
        println!("  … +{} satır", lost.len() - 60);
    }
    println!("\n{} projeksiyonda VAR, belgede YOK:", gained.len());
    for g in gained.iter().take(30) {
        println!("{g}");
    }
    if lost.is_empty() && gained.is_empty() {
        println!("\nKONTRAT SAĞLAM — iki okuma her (aktör × WFE) çiftinde aynı cevabı veriyor.");
    } else {
        println!(
            "\nAYRIŞMA VAR. Sık sebepler: backfill koşmadı (`visibility_backfill --apply`), \
             org ağacı grant yazıldıktan sonra değişti (yeniden projeksiyon gerekir), \
             ya da `listable` kuralı viewer'a bağlı bir selector kullanıyor (yukarıdaki tarama)."
        );
    }
}
