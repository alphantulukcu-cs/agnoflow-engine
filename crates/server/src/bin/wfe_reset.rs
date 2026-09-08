//! TAM SIFIRLAMA: v2.3 öncesi koşan/koşmuş BÜTÜN WFE verisi + yetim blob'lar
//! (R01/S2, kuyruk kalemi O1, iş: WOR-107).
//!
//! ## Neden
//!
//! v2.3 üç yeni satır alanı getirdi (`from_node`/`to_node` — Ç2, `branch_entry` — Ç4,
//! `branch_round` — E14) ve hiçbiri için backfill/fallback YAZILMADI (R01/S1: sürüm
//! duyarlı okuyucu yok, sentinel yok, DEFAULT yok). Eski satırlar bu alanları NULL
//! taşır ve R02'nin yeni escalation tabanı (`to_node != null` olan son satır) eski
//! veride satır BULAMAZ → escalation sessizce susar. R02 (WOR-80) koda zaten indi.
//!
//! ## SIRA BAĞLAYICI
//!
//!   bu betik → R07/S2 arşivleme → R07/S4 yeni golden seed'i → sürüm kapısı "2.3"
//!
//! ## Kapsam — `wf` şemasında 13 tablo, LİSTE BAĞLAYICI
//!
//! Aktif + kapanmış WFE'lerin HEPSİ (R01/S2, kullanıcı kapsamı genişletti).
//! DOKUNULMAZ: `wfd_meta`, `wfd_env_var`, `wfd_template*`, `project*`, `environment`,
//! `db_connection`, `app_user` ve BÜTÜN `org` şeması. **Liste dışına çıkan bir silme
//! bu kararla YETKİLENDİRİLMEMİŞTİR** — bu yüzden `TRUNCATE` `CASCADE`'SİZ koşar:
//! yarın listenin dışından bir tablo `wf.wfe`ye bakmaya başlarsa betik sessizce onu
//! da silmek yerine PATLAR.
//!
//! ## Yetim blob'lar
//!
//! `storage_key` taşıyan iki tablo (`wfe_attachment`, `wfe_note_file`) ve staging
//! kayıtları (`upload_staging` → `staging/{upload_id}`, bkz. `staging::staging_key`)
//! silinince nesne depolamada karşılıksız dosya kalır; temizliği R01/S2'ye göre AYNI
//! kalemin parçasıdır. Anahtarlar TAHMİN EDİLMEZ: iki tabloda `storage_key` kolonu
//! yazılı, birebir o anahtarlar silinir.
//!
//! Depo WFD BAŞINA çözülür (2026-08-07, `$env` → `ATTACHMENT_STORAGE_*`): müşterinin
//! S3'ü ya da yerel disk olabilir. Çözüm sunucudakiyle AYNI fonksiyondan geçer
//! (`wf_wfd::storage_config_from_lookup`); `$env`de depo tanımlı değilse deployment
//! varsayılanına düşülür — okuma/silme yolunun bugünkü davranışı (`store_for_wfe`).
//!
//! Blob'lar DB'den ÖNCE silinir: satırlar dururken hata alınırsa iş yeniden koşulabilir.
//! Blob silmede hata varsa `TRUNCATE` KOŞMAZ (`--ignore-blob-errors` ile zorlanır) —
//! yoksa anahtarını kaybettiğimiz dosyalar depoda sessizce kalır.
//!
//! ## Koşum
//!
//! VARSAYILAN KURU KOŞUMDUR — hiçbir şey silinmez, ne silineceği raporlanır:
//!   `DATABASE_URL=… cargo run -p wf-server --bin wfe_reset`
//! Silmek için (GERİ DÖNÜŞSÜZ):
//!   `DATABASE_URL=… cargo run -p wf-server --bin wfe_reset -- --apply`
//!
//! `--apply` ayrıca `WFE_RESET_CONFIRM=<DATABASE_URL'in db adı>` ister: yanlış
//! veritabanına koşmak bu kalemin tek gerçek riski.
//!
//! Ek-belge deposu için `ATTACHMENT_STORAGE_*`, `$env` secret'larını çözmek için
//! `DB_CONN_SECRET` gerekir. Demo/test senaryolarının v2.3 üstünde YENİDEN KURULMASI
//! bu betiğin işi değil — elle yapılır (R01/S2, ayrı kabul kriteri).

use std::collections::HashMap;

use opendal::Operator;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Row};
use uuid::Uuid;

/// R01/S2'nin TAM listesi. Sıra `TRUNCATE` için önemsizdir (tek ifade, tek tx);
/// okunurluk için FK zinciriyle aynı yönde yazıldı: çocuklar önce.
const TABLES: [&str; 13] = [
    "wf.wfe_note_read",
    "wf.wfe_note_file",
    "wf.wfe_note",
    "wf.wfe_attachment",
    "wf.wfe_branch",
    "wf.wfe_call",
    "wf.wfe_dynctx",
    "wf.wfah",
    "wf.wfe_reservation",
    "wf.wfe_start_dedupe",
    "wf.upload_staging",
    "wf.visibility_reprojection",
    "wf.wfe",
];

/// Silinecek bir blob: anahtar + hangi WFD/ortam bağlamında çözüleceği.
struct Blob {
    kind: &'static str,
    key: String,
    wfd_id: Uuid,
    orgtnt_id: Uuid,
    environment_id: Option<Uuid>,
}

/// Depo çözümünün önbellek anahtarı — `(wfd_id, environment_id)` başına bir kez çözülür.
type StoreKey = (Uuid, Option<Uuid>);

#[tokio::main]
async fn main() {
    let apply = std::env::args().any(|a| a == "--apply");
    let ignore_blob_errors = std::env::args().any(|a| a == "--ignore-blob-errors");
    let db = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");

    println!(
        "=== WFE TAM SIFIRLAMA (R01/S2) === (mod: {})",
        if apply { "SİL — GERİ DÖNÜŞSÜZ" } else { "KURU KOŞUM" }
    );
    println!("hedef: {}\n", redact_url(&db));

    if apply {
        let expected = db_name(&db);
        match std::env::var("WFE_RESET_CONFIRM") {
            Ok(v) if v == expected => {}
            _ => {
                eprintln!(
                    "DURDU — `--apply` için onay gerekli:\n  \
                     WFE_RESET_CONFIRM={expected}\n\
                     (DATABASE_URL'deki veritabanı adı; yanlış veritabanına koşmayı önler)"
                );
                std::process::exit(2);
            }
        }
    }

    let pool = connect(&db).await;

    // 1) Ne kaybedileceği rakamla görünsün — kuru koşumun asıl çıktısı bu.
    println!("--- Satır sayıları ---");
    let mut total: i64 = 0;
    for table in TABLES {
        let n = count(&pool, table).await;
        total += n;
        println!("  {n:>8}  {table}");
    }
    println!("  {:>8}  TOPLAM\n", total);

    // 2) Blob envanteri. Anahtarlar DB'de yazılı (staging hariç: anahtar upload_id'den
    //    türer — `staging::staging_key`).
    let blobs = collect_blobs(&pool).await;
    println!("--- Yetim kalacak blob'lar ---");
    if blobs.is_empty() {
        println!("  yok");
    } else {
        let mut per_kind: HashMap<&str, usize> = HashMap::new();
        for b in &blobs {
            *per_kind.entry(b.kind).or_default() += 1;
        }
        let mut kinds: Vec<_> = per_kind.into_iter().collect();
        kinds.sort();
        for (kind, n) in kinds {
            println!("  {n:>8}  {kind}");
        }
    }
    println!();

    if !apply {
        println!("KURU KOŞUM — hiçbir şey silinmedi. Silmek için: --apply");
        return;
    }

    // 3) Blob'lar ÖNCE: satırlar dururken hata alınırsa iş yeniden koşulabilir.
    let mut failed = 0usize;
    if !blobs.is_empty() {
        println!("--- Blob silme ---");
        let mut stores: HashMap<StoreKey, Option<Operator>> = HashMap::new();
        let mut removed = 0usize;
        for b in &blobs {
            let store_key = (b.wfd_id, b.environment_id);
            if !stores.contains_key(&store_key) {
                let op = resolve_store(&pool, b).await;
                if op.is_none() {
                    eprintln!(
                        "  ! depo çözülemedi (wfd={} env={:?}) — bu WFD'nin blob'ları ATLANDI",
                        &b.wfd_id.to_string()[..8],
                        b.environment_id.map(|e| e.to_string()[..8].to_string())
                    );
                }
                stores.insert(store_key, op);
            }
            let Some(op) = stores.get(&store_key).and_then(|o| o.as_ref()) else {
                failed += 1;
                continue;
            };
            match op.delete(&b.key).await {
                Ok(()) => removed += 1,
                Err(e) => {
                    failed += 1;
                    eprintln!("  ! silinemedi {} ({}): {e}", b.key, b.kind);
                }
            }
        }
        println!("  {removed} blob silindi, {failed} başarısız\n");
    }

    if failed > 0 && !ignore_blob_errors {
        eprintln!(
            "DURDU — {failed} blob silinemedi ve TRUNCATE koşmadı. Satırlar duruyor, iş\n\
             yeniden koşulabilir. Depo erişimi düzeltilemiyorsa: --ignore-blob-errors\n\
             (o blob'lar depoda anahtarsız kalır)."
        );
        std::process::exit(1);
    }

    // 4) TEK ifade, TEK transaction. CASCADE YOK: liste dışı bir referans doğarsa
    //    sessizce silmek yerine patlamalı (R01/S2 kapsam kilidi).
    let stmt = format!("TRUNCATE TABLE {}", TABLES.join(", "));
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query(&stmt)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("TRUNCATE başarısız: {e}\n  ifade: {stmt}"));
    tx.commit().await.expect("commit");

    println!("--- Bitti ---");
    println!("  {} tablo boşaltıldı ({total} satır gitti)", TABLES.len());
    println!("\nSIRADAKİ (bağlayıcı sıra): R07/S2 arşivleme → R07/S4 golden seed →");
    println!("sürüm kapısı \"2.3\". Demo/test senaryoları v2.3 üstünde ELLE kurulur.");
}

async fn connect(db: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(3)
        .after_connect(|c, _| {
            Box::pin(async move {
                c.execute("SET search_path TO org, public").await?;
                Ok(())
            })
        })
        .connect(db)
        .await
        .expect("db connect")
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("sayım başarısız ({table}): {e}"))
}

/// Üç kaynaktan blob envanteri. İlk ikisinde anahtar DB'de yazılı; staging'de
/// anahtar `upload_id`den türer (tek format dizesi, `staging::staging_key` ile aynı).
async fn collect_blobs(pool: &PgPool) -> Vec<Blob> {
    let mut out = Vec::new();

    let rows = sqlx::query(
        "SELECT a.storage_key, e.wfd_id, e.orgtnt_id, e.environment_id
           FROM wf.wfe_attachment a
           JOIN wf.wfe e ON e.wfe_id = a.wfe_id",
    )
    .fetch_all(pool)
    .await
    .expect("ek-belge blob envanteri");
    for r in rows {
        out.push(Blob {
            kind: "ek-belge (wfe_attachment)",
            key: r.get("storage_key"),
            wfd_id: r.get("wfd_id"),
            orgtnt_id: r.get("orgtnt_id"),
            environment_id: r.get("environment_id"),
        });
    }

    let rows = sqlx::query(
        "SELECT f.storage_key, e.wfd_id, e.orgtnt_id, e.environment_id
           FROM wf.wfe_note_file f
           JOIN wf.wfe_note n ON n.note_id = f.note_id
           JOIN wf.wfe e ON e.wfe_id = n.wfe_id",
    )
    .fetch_all(pool)
    .await
    .expect("not dosyası blob envanteri");
    for r in rows {
        out.push(Blob {
            kind: "not dosyası (wfe_note_file)",
            key: r.get("storage_key"),
            wfd_id: r.get("wfd_id"),
            orgtnt_id: r.get("orgtnt_id"),
            environment_id: r.get("environment_id"),
        });
    }

    let rows = sqlx::query(
        "SELECT upload_id, wfd_id, orgtnt_id, environment_id FROM wf.upload_staging",
    )
    .fetch_all(pool)
    .await
    .expect("staging blob envanteri");
    for r in rows {
        let upload_id: Uuid = r.get("upload_id");
        out.push(Blob {
            kind: "staging (upload_staging)",
            key: format!("staging/{upload_id}"),
            wfd_id: r.get("wfd_id"),
            orgtnt_id: r.get("orgtnt_id"),
            environment_id: r.get("environment_id"),
        });
    }

    out
}

/// Blob'un deposunu WFD'nin `$env`inden çözer, tanımsızsa deployment varsayılanına
/// düşer — sunucunun OKUMA/SİLME yolundaki davranışın aynısı (`store_for_wfe`,
/// fallback KORUNUR: eski davranışla sunucu diskine yazılmış dosyalar da süpürülebilmeli).
async fn resolve_store(pool: &PgPool, b: &Blob) -> Option<Operator> {
    let cfg = wfd_env_config(pool, b)
        .await
        .unwrap_or_else(wf_wfd::attachment_storage_from_env);
    match wf_wfd::build_operator(&cfg) {
        Ok(op) => Some(op),
        Err(e) => {
            eprintln!("  ! depo kurulamadı: {e}");
            None
        }
    }
}

/// WFD başına `$env` depo konfigürasyonu. `None` = bu WFD'nin kendi deposu yok
/// (projesi yok, ortam çözülemedi ya da `ATTACHMENT_STORAGE_BACKEND` tanımsız).
async fn wfd_env_config(pool: &PgPool, b: &Blob) -> Option<wf_wfd::StorageConfig> {
    let owner = sqlx::query_as::<_, (Option<Uuid>, String)>(
        "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(b.wfd_id)
    .fetch_optional(pool)
    .await
    .ok()??;
    let (project_id, wfd_name) = (owner.0?, owner.1);

    let env_id = match b.environment_id {
        Some(id) => id,
        None => {
            wf_wfe::repo::env::resolve_environment(pool, b.orgtnt_id, None)
                .await
                .ok()?
                .id
        }
    };
    let run_env =
        wf_wfe::repo::env::load_run_env(pool, project_id, &wfd_name, env_id, true)
            .await
            .ok()?;

    wf_wfd::storage_config_from_lookup(
        wf_wfd::ATTACHMENT_ENV_PREFIX,
        |key| {
            let value = &run_env.full().get(key)?.value;
            match value {
                serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
                serde_json::Value::String(_) => None,
                other => Some(other.to_string()),
            }
        },
        wf_wfd::DEFAULT_ATTACHMENT_PATH,
    )
}

/// `DATABASE_URL`in veritabanı adı — onay dizesi bu.
fn db_name(url: &str) -> String {
    url.rsplit('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Şifreyi gizleyerek hedefi yazdırır: rapor kopyalanıp yapıştırılıyor.
fn redact_url(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            let creds = &url[scheme_end + 3..at];
            let user = creds.split(':').next().unwrap_or("");
            format!("{}://{}:***@{}", &url[..scheme_end], user, &url[at + 1..])
        }
        _ => url.to_string(),
    }
}
