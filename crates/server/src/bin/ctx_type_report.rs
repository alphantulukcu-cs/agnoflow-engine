//! Context TİP raporu (salt okuma, hiçbir şey yazmaz).
//!
//! Motor 2026-08-19'a kadar çalışma anında tip denetlemiyordu: sayı beklenen bir alana
//! gönderilen metin ctx'e AYNEN yazılıyordu. Denetim açılmadan ÖNCE sahada ne kadar
//! bozuk veri olduğu bilinmeli — bu araç onu sayar. `visibility_report` deseninin
//! aynısı: DB'li test koşulmayan bu repoda kontratın ölçüm aracı budur.
//!
//! Ölçtükleri:
//!   1. **İhlal** — WFE'nin son `dynctx` snapshot'ı, kendi WFD sürümünün context
//!      şemasına uymuyor (`wfe_core::v22::ctx_types::validate_dynctx`). Kapı B'nin
//!      `warn`dan `reject`e çevrilme zamanı bu sayının sıfırlanmasıyla belirlenir.
//!   2. **Bildirilmemiş alan** — ctx'te şemada karşılığı olmayan kök alan. İHLAL DEĞİL:
//!      bugün `wfes_effects` bildirilmemiş bir hedefe yazabiliyor
//!      (`check_effect_value_types` şemasız hedefi atlıyor). Ölçülmesi gereken bir olgu.
//!   3. **Eski sözdizimi** — context şemasında `$ref` kullanan WFD sürümleri. `$ref`
//!      artık yazılamaz (`context_ref_removed`) ama okunur; kaç belgenin göçe ihtiyacı
//!      olduğunu bu sayı söyler.
//!
//! Koşum: `DATABASE_URL=... cargo run -p wf-server --bin ctx_type_report`
//! (WFD JSON deposu için `STORAGE_*` env'leri de gerekir — belge oradan okunur.)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool};
use uuid::Uuid;
use wfe_core::v22::ports::WfdStore;
use wfe_core::v22::ctx_types;

#[derive(sqlx::FromRow)]
struct WfeRow {
    wfe_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    status: String,
}

/// `$ref` context ağacında herhangi bir yerde geçiyor mu (göç sayacı için).
fn uses_ref(node: &Value) -> bool {
    match node {
        Value::Object(map) => {
            map.contains_key("$ref") || map.values().any(uses_ref)
        }
        Value::Array(arr) => arr.iter().any(uses_ref),
        _ => false,
    }
}

async fn connect() -> PgPool {
    let db = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|c, _| {
            Box::pin(async move {
                c.execute("SET search_path TO org, public").await?;
                Ok(())
            })
        })
        .connect(&db)
        .await
        .expect("db connect")
}

#[tokio::main]
async fn main() {
    let pool = connect().await;

    // `wf-server` binary-only crate → config sunucuyla AYNI env'den; yalnız WFD deposu.
    let storage = wf_wfd::build_operator(&wf_wfd::StorageConfig::from_env()).expect("storage init");
    let wfd_store = Arc::new(wf_wfd::WfdAdapter::new(pool.clone(), storage));

    let wfes: Vec<WfeRow> = sqlx::query_as(
        "SELECT wfe_id, wfd_id, wfd_version, status FROM wf.wfe ORDER BY created_at",
    )
    .fetch_all(&pool)
    .await
    .expect("wfe list");

    // Son dynctx snapshot'ları — WFE başına tek satır, tek sorgu (N+1 yok).
    let ids: Vec<Uuid> = wfes.iter().map(|w| w.wfe_id).collect();
    let ctx_rows: Vec<(Uuid, Value)> = sqlx::query_as(
        "SELECT DISTINCT ON (wfe_id) wfe_id, ctx FROM wf.wfe_dynctx
         WHERE wfe_id = ANY($1) ORDER BY wfe_id, seq DESC",
    )
    .bind(&ids)
    .fetch_all(&pool)
    .await
    .expect("dynctx list");
    let ctx_by_wfe: HashMap<Uuid, Value> = ctx_rows.into_iter().collect();

    println!("=== CONTEXT TİP RAPORU ===");
    println!("WFE sayısı: {}", wfes.len());

    // WFD sürümü başına context şeması, bir kez okunur (adapter'ın kendi cache'i de var).
    let mut schemas: HashMap<(Uuid, i32), Option<Value>> = HashMap::new();
    let mut unreadable: Vec<String> = Vec::new();

    let mut violating_wfes = 0usize;
    let mut total_violations = 0usize;
    let mut undeclared_fields: HashMap<String, usize> = HashMap::new();
    let mut no_snapshot = 0usize;
    let mut legacy_ref_versions: HashSet<(Uuid, i32)> = HashSet::new();
    let mut violations_by_path: HashMap<String, usize> = HashMap::new();
    let mut samples: Vec<String> = Vec::new();

    for wfe in &wfes {
        let key = (wfe.wfd_id, wfe.wfd_version);
        if let std::collections::hash_map::Entry::Vacant(slot) = schemas.entry(key) {
            let context = match wfd_store.fetch(wfe.wfd_id, wfe.wfd_version).await {
                Ok(wfd) => {
                    if uses_ref(&wfd.context) {
                        legacy_ref_versions.insert(key);
                    }
                    Some(wfd.context.clone())
                }
                Err(e) => {
                    unreadable.push(format!("{} v{}: {e}", wfe.wfd_id, wfe.wfd_version));
                    None
                }
            };
            slot.insert(context);
        }
        let Some(Some(context)) = schemas.get(&key) else {
            continue; // belge okunamadı — ayrıca raporlanıyor
        };

        let Some(ctx) = ctx_by_wfe.get(&wfe.wfe_id) else {
            no_snapshot += 1;
            continue;
        };

        let report = ctx_types::validate_dynctx(context, ctx);
        for name in &report.undeclared {
            *undeclared_fields.entry(name.clone()).or_insert(0) += 1;
        }
        if report.violations.is_empty() {
            continue;
        }
        violating_wfes += 1;
        total_violations += report.violations.len();
        for v in &report.violations {
            *violations_by_path.entry(v.path.clone()).or_insert(0) += 1;
            if samples.len() < 25 {
                samples.push(format!(
                    "  wfe {} ({}) · wfd {} v{} · {v}",
                    wfe.wfe_id, wfe.status, wfe.wfd_id, wfe.wfd_version
                ));
            }
        }
    }

    println!("dynctx snapshot'ı olmayan WFE: {no_snapshot}");
    println!("okunamayan WFD sürümü: {}", unreadable.len());
    for line in &unreadable {
        println!("  {line}");
    }

    println!("\n--- 1. İHLAL (kapı B'nin reject'e çevrilmesi buna bağlı) ---");
    println!("ihlalli WFE: {violating_wfes} / {}", wfes.len());
    println!("toplam ihlal: {total_violations}");
    if !violations_by_path.is_empty() {
        let mut by_path: Vec<_> = violations_by_path.iter().collect();
        by_path.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        println!("yol bazında:");
        for (path, count) in by_path.iter().take(20) {
            println!("  {count:>5}× {path}");
        }
    }
    if !samples.is_empty() {
        println!("örnekler (en çok 25):");
        for line in &samples {
            println!("{line}");
        }
    }

    println!("\n--- 2. BİLDİRİLMEMİŞ ctx alanı (ihlal DEĞİL, olgu) ---");
    if undeclared_fields.is_empty() {
        println!("yok");
    } else {
        let mut fields: Vec<_> = undeclared_fields.iter().collect();
        fields.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (name, count) in fields.iter().take(20) {
            println!("  {count:>5}× {name}");
        }
    }

    println!("\n--- 3. ESKİ SÖZDİZİMİ ($ref) ---");
    println!(
        "context şemasında `$ref` kullanan WFD sürümü: {}",
        legacy_ref_versions.len()
    );
    for (id, ver) in &legacy_ref_versions {
        println!("  {id} v{ver}");
    }

    println!("\n=== SONUÇ ===");
    if total_violations == 0 {
        println!("Sahada tip ihlali YOK — kapı B doğrudan `reject` olarak açılabilir.");
    } else {
        println!(
            "Sahada {total_violations} ihlal var ({violating_wfes} WFE). Kapı B `warn` \
             başlamalı; bu sayı sıfırlanınca `reject`e çevrilir."
        );
    }
}
