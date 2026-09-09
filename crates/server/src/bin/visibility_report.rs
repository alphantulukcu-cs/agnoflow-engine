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
//! Ölçülen kural (2026-08-13'ten beri yürürlükte):
//! ```text
//! görünür(WFE, viewer) :=
//!      listable/wf_admin grant'i eşleşir           -- KALICI, `when` uygulanmış
//!   OR varılan terminal'in listable'ı eşleşir     -- KALICI, SONUCA BAĞLI (g)
//!   OR (status = 'active' AND (node c_a       eşleşir
//!                           OR node listable  eşleşir  -- DURUMA BAĞLI (f)
//!                           OR WFE/kol claim'i viewer'da))
//! ```
//!
//! Bu araç ÜÇ soruyu cevaplar:
//!   1. **Kontrat**: hangi (aktör, WFE) çifti BELGE okumasında (`can_view`) görünürken
//!      PROJEKSİYONDA görünmez oluyor (ve tersi). Sağlam kontratta iki liste de boştur.
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
//!   3. **Grant boyutu** (`E03` → `M2`/WOR-112): predicate'in okuduğu ALTI kolonun
//!      aday sayısı ve bayt büyüklüğü, act kolonlarında taban/grant kırılımı, en büyük
//!      on satır. İlk iki soru kolonların DOĞRU olduğunu söylüyor, BÜYÜKLÜĞÜ hakkında
//!      bir şey söylemiyordu — `E04` yetki kümesini genişlettiği için kolonlar
//!      `≈ U × (1 + G)` ile büyüyor.
//!
//! Koşum: `DATABASE_URL=... cargo run -p wf-server --bin visibility_report`

use std::sync::Arc;

use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::{COrgu, CaGrantRule};
use wfe_core::v22::ports::{VisibilityPort, WfdStore, WfeStore};
use wfe_core::v22::visibility::can_view;
use wfe_core::OrgPort;

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

/// Grant BOYUTU tablosunun bir satırı — kolon başına aday sayısı ve bayt.
#[derive(sqlx::FromRow)]
struct GrantSizeRow {
    col: String,
    rows_total: i64,
    rows_nonempty: i64,
    entries: i64,
    max_entries: i32,
    bytes: i64,
    max_bytes: i32,
}

/// `authority` damgası kırılımı (yalnız act kolonlarında yazılır).
#[derive(sqlx::FromRow)]
struct AuthorityRow {
    col: String,
    authority: String,
    entries: i64,
}

/// En büyük projeksiyon satırları — "hangi WFE şişti" sorusu.
#[derive(sqlx::FromRow)]
struct BiggestRow {
    wfe_id: Uuid,
    col: String,
    entries: i32,
    bytes: i32,
}

/// **Grant BOYUTU** (`E03` → `M2`/WOR-112). `E04` yetki kümesini
/// `node.c_a ∪ açılmış grantlar` yaptı; her açık grant kuralı çözüldüğü BİRİM SAYISI
/// kadar aday üretiyor, yani act kolonları `≈ U × (1 + G)` ile büyüyor (`U` = ORGTRVLANG
/// selector'ının çözdüğü birim sayısı, `G` = açık grant). Rapor bunu ölçmüyordu:
/// kontrat bölümü kolonların DOĞRU olduğunu söylüyor, BÜYÜKLÜĞÜ hakkında bir şey
/// söylemiyordu.
///
/// Ölçülen altı kolon = görünürlük predicate'inin okuduğu kolonların TAMAMI
/// (`wf_wfe::visibility::sql`): `wfe.view_c_a` · `current_c_a` · `current_view_c_a` ·
/// `end_view_c_a` + `wfe_branch.c_a` · `view_c_a`. `bayt` = `pg_column_size` (satırda
/// duran, sıkıştırılmış hâl — GIN indeksleri buna dahil DEĞİL).
///
/// `authority` kırılımı yalnız ACT kolonlarında anlamlıdır (`R03`/S1-c: görünürlük
/// kolonlarında bu alan hiç yazılmaz) ve asıl soruyu o cevaplar: kolonun ne kadarı
/// TABANDAN, ne kadarı açık GRANT'tan geliyor. `(yok)` = alan eklenmeden önce yazılmış
/// satır; `R01` gereği backfill YAZILMAZ, bir sonraki commit'te damga gelir.
async fn grant_size_report(pool: &PgPool) {
    println!("\n--- Grant boyutu: projeksiyon kolonları ---");

    // Kolon başına tek geçiş; `UNION ALL` altı kolonu aynı çıktıda sıralı tutar.
    let per_col = "
        SELECT $1::text AS col, count(*)::bigint AS rows_total,
               count(*) FILTER (WHERE jsonb_array_length(%C) > 0)::bigint AS rows_nonempty,
               coalesce(sum(jsonb_array_length(%C)), 0)::bigint AS entries,
               coalesce(max(jsonb_array_length(%C)), 0)::int    AS max_entries,
               coalesce(sum(pg_column_size(%C)), 0)::bigint     AS bytes,
               coalesce(max(pg_column_size(%C)), 0)::int        AS max_bytes
          FROM %T";
    let cols: &[(&str, &str, &str)] = &[
        ("wfe.view_c_a", "view_c_a", "wf.wfe"),
        ("wfe.current_c_a", "current_c_a", "wf.wfe"),
        ("wfe.current_view_c_a", "current_view_c_a", "wf.wfe"),
        ("wfe.end_view_c_a", "end_view_c_a", "wf.wfe"),
        ("wfe_branch.c_a", "c_a", "wf.wfe_branch"),
        ("wfe_branch.view_c_a", "view_c_a", "wf.wfe_branch"),
    ];

    println!(
        "\n| kolon | satır | dolu satır | aday | aday/dolu satır | en çok aday | \
         toplam bayt | en büyük bayt |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for (label, column, table) in cols {
        // Kolon/tablo adı BİND EDİLEMEZ (identifier), ama ikisi de bu dosyada sabit
        // bir listeden gelir — dışarıdan gelen tek değer `$1` ile bağlanan ETİKETtir.
        let sql = per_col.replace("%C", column).replace("%T", table);
        let row: GrantSizeRow = match sqlx::query_as(&sql).bind(label).fetch_one(pool).await {
            Ok(r) => r,
            Err(e) => {
                println!("| {label} | — | — | — | — | — | — | HATA: {e} |");
                continue;
            }
        };
        let per_row = if row.rows_nonempty > 0 {
            format!("{:.1}", row.entries as f64 / row.rows_nonempty as f64)
        } else {
            "—".into()
        };
        println!(
            "| `{}` | {} | {} | {} | {per_row} | {} | {} | {} |",
            row.col,
            row.rows_total,
            row.rows_nonempty,
            row.entries,
            row.max_entries,
            row.bytes,
            row.max_bytes
        );
    }

    // `authority`: kolonun ne kadarı tabandan, ne kadarı açık grant'tan.
    let auth: Result<Vec<AuthorityRow>, _> = sqlx::query_as(
        "SELECT 'wfe.current_c_a' AS col,
                coalesce(e->>'authority', '(yok)') AS authority,
                count(*)::bigint AS entries
           FROM wf.wfe, jsonb_array_elements(current_c_a) e
          GROUP BY 1, 2
         UNION ALL
         SELECT 'wfe_branch.c_a' AS col,
                coalesce(e->>'authority', '(yok)') AS authority,
                count(*)::bigint AS entries
           FROM wf.wfe_branch, jsonb_array_elements(c_a) e
          GROUP BY 1, 2
         ORDER BY 1, 2",
    )
    .fetch_all(pool)
    .await;
    println!("\n--- Act kolonlarında `authority` kırılımı (taban vs açık grant) ---");
    match auth {
        Ok(rows) if rows.is_empty() => println!("  (act kolonları boş)"),
        Ok(rows) => {
            println!("\n| kolon | authority | aday |");
            println!("|---|---|---:|");
            for r in &rows {
                println!("| `{}` | `{}` | {} |", r.col, r.authority, r.entries);
            }
            let grant: i64 = rows
                .iter()
                .filter(|r| r.authority == "grant")
                .map(|r| r.entries)
                .sum();
            let total: i64 = rows.iter().map(|r| r.entries).sum();
            if total > 0 {
                println!(
                    "\nAçık grant'ın payı: {grant}/{total} = {:.1}%",
                    100.0 * grant as f64 / total as f64
                );
            }
        }
        Err(e) => println!("  HATA: {e}"),
    }

    // En büyük satırlar: hangi WFE'nin hangi kolonu şişmiş.
    let biggest: Result<Vec<BiggestRow>, _> = sqlx::query_as(
        "SELECT wfe_id, col, entries, bytes FROM (
             SELECT wfe_id, 'wfe.view_c_a' AS col,
                    jsonb_array_length(view_c_a) AS entries,
                    pg_column_size(view_c_a) AS bytes
               FROM wf.wfe
             UNION ALL
             SELECT wfe_id, 'wfe.current_c_a', jsonb_array_length(current_c_a),
                    pg_column_size(current_c_a) FROM wf.wfe
             UNION ALL
             SELECT wfe_id, 'wfe.current_view_c_a', jsonb_array_length(current_view_c_a),
                    pg_column_size(current_view_c_a) FROM wf.wfe
             UNION ALL
             SELECT wfe_id, 'wfe.end_view_c_a', jsonb_array_length(end_view_c_a),
                    pg_column_size(end_view_c_a) FROM wf.wfe
             UNION ALL
             SELECT wfe_id, 'wfe_branch.c_a', jsonb_array_length(c_a),
                    pg_column_size(c_a) FROM wf.wfe_branch
             UNION ALL
             SELECT wfe_id, 'wfe_branch.view_c_a', jsonb_array_length(view_c_a),
                    pg_column_size(view_c_a) FROM wf.wfe_branch
         ) x
         WHERE entries > 0
         ORDER BY entries DESC, bytes DESC, wfe_id, col
         LIMIT 10",
    )
    .fetch_all(pool)
    .await;
    println!("\n--- En büyük on projeksiyon satırı ---");
    match biggest {
        Ok(rows) if rows.is_empty() => println!("  (dolu kolon yok)"),
        Ok(rows) => {
            println!("\n| WFE | kolon | aday | bayt |");
            println!("|---|---|---:|---:|");
            for r in &rows {
                println!(
                    "| {} | `{}` | {} | {} |",
                    &r.wfe_id.to_string()[..8],
                    r.col,
                    r.entries,
                    r.bytes
                );
            }
        }
        Err(e) => println!("  HATA: {e}"),
    }
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
                let wf_admin_grants: Vec<_> = wfd.wf_admin.iter().map(|r| r.grant.clone()).collect();
                reasons.extend(viewer_relative_reasons(&wf_admin_grants, "wf_admin"));
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

    // ---- Grant boyutu (`M2`/WOR-112) ----
    grant_size_report(&pool).await;

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
