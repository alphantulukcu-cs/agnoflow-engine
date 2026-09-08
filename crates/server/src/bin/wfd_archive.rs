//! 2.2 BELGELERİNİ ARŞİVLE + v2.3 golden'ı yeni seed olarak kur (R07/S2 + S4, iş: WOR-104).
//!
//! ## Neden bir betik
//!
//! İşin kendisi `migrations/wf/20260908000002_v2_3_archive_and_seed.sql`tir ve göç
//! dosyası KANONİK olandır — burada koşan SQL onunla birebir aynıdır. Betik iki şey
//! ekler: (1) göçler `psql` ile elle uygulanıyor (CLAUDE.md) ve `psql` her makinede
//! yok; (2) göç yalnız meta satırını yazabiliyor — `s3_key`in gösterdiği JSON'ı SQL
//! koyamaz, blob ELLE konmak zorundaydı. Bu betik ikisini tek koşumda yapar ve
//! öncesinde ne olacağını rakamla gösterir.
//!
//! ## Ne yapar
//!
//! * `wf.wfd_meta` + `wf.wfd_template`in BÜTÜN satırları `is_active = false`.
//!   Hangi satırın 2.2 olduğu ARANMAZ: tabloda wire sürümünü taşıyan kolon yok
//!   (`version` belge revizyonudur) ve bu iş koştuğu anda var olan her belge 2.2'dir.
//!   Taslaklar ve `pending_approval` da kapsamda (R07/S2, kullanıcı hükmü).
//! * v2.3 golden'ı yeni seed satırı olarak girer (yeni `wfd_id`, `version = 2`; eski
//!   satır arşivde kaldığı için ne PK ne de `(project_id, name, version)` yeniden
//!   kullanılabilir) ve belgenin JSON'ını `s3_key`e yazar.
//!
//! **SİLME YOK.** Arşivlenen belgelerin blob'ları, `wfd_env_var` ve
//! `wfd_template_project`/`_user` DOKUNULMAZ — arşivin anlamı kaynak metnin durması:
//! kullanıcı tarifleri yeniden kurarken onlara bakarak dönüşümü ELLE yapar (R07/S3,
//! çeviri aracı yazılmaz).
//!
//! ## Koşum
//!
//! VARSAYILAN KURU KOŞUMDUR — hiçbir şey yazılmaz, ne olacağı raporlanır:
//!   `DATABASE_URL=… STORAGE_…=… cargo run -p wf-server --bin wfd_archive`
//! Uygulamak için (GERİ DÖNÜŞSÜZ):
//!   `… cargo run -p wf-server --bin wfd_archive -- --apply`
//!
//! `--apply` ayrıca `WFD_ARCHIVE_CONFIRM=<DATABASE_URL'in db adı>` ister — `wfe_reset`
//! ile aynı gerekçe: yanlış veritabanına koşmak bu kalemin tek gerçek riskidir.
//!
//! Blob DB'den SONRA yazılır (`wfe_reset`in tersi, bilerek): orada silme vardı ve
//! anahtarını kaybetmemek için blob önce gidiyordu; burada yazma var ve satırı olmayan
//! bir blob yetim kalır. Blob yazılamazsa satır zaten aktiftir ve fetch "bulunamadı"
//! verir — betik yeniden koşulabilir. Arşivleme seed satırını KAPSAM DIŞI bırakır
//! (`wfd_id <> golden`), yoksa ikinci koşum yeni golden'ı da kapatır ve `ON CONFLICT
//! DO NOTHING` onu geri AÇMAZDI: katalog boş kalırdı.
//!
//! ⚠️ `is_active = true` yazarak geri almak belgeyi yeniden koşum yoluna sokar ve sürüm
//! kapısı onu YİNE reddeder: bayrak bir TUZAK DÜĞMESİDİR, geri alma R07 ile
//! yetkilendirilmemiştir.

use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Row};
use uuid::Uuid;
use wf_wfd::{build_operator, StorageConfig};

/// R07/S4'ün seed satırı. Göç dosyasındaki değerlerle BİREBİR aynı olmak zorunda.
const GOLDEN_WFD_ID: &str = "9fe82344-b477-4af9-bc73-0afed2c56c4f";
const GOLDEN_ORGTNT_ID: &str = "3c1811a6-1e63-4261-a1ce-658da1fbfa6b";
const GOLDEN_PROJECT: &str = "Test Project";
const GOLDEN_NAME: &str = "Kredi Başvurusu";
const GOLDEN_VERSION: i32 = 2;
const GOLDEN_DOC_ID: &str = "kredi-basvuru-v2";
const GOLDEN_DOC_VERSION: &str = "2.1.0";
const GOLDEN_DESCRIPTION: &str = "v2.3 golden fixture (Ç5 düz aksiyon gövdesi + A05 escalation grant)";

/// Belge metni derlemeye gömülür: betiğin yazdığı gövde ile motorun parite testinden
/// geçen gövde AYNI dosyadır, "hangi kopya" sorusu doğmasın.
const GOLDEN_JSON: &str = include_str!("../../../../docs/spec/examples/kredi-basvuru.golden.json");

#[tokio::main]
async fn main() {
    let apply = std::env::args().any(|a| a == "--apply");
    let db = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");

    println!(
        "=== 2.2 BELGELERİNİ ARŞİVLE + v2.3 golden seed (R07/S2+S4) === (mod: {})",
        if apply { "UYGULA — GERİ DÖNÜŞSÜZ" } else { "KURU KOŞUM" }
    );
    println!("hedef: {}\n", redact_url(&db));

    if apply {
        let expected = db_name(&db);
        match std::env::var("WFD_ARCHIVE_CONFIRM") {
            Ok(v) if v == expected => {}
            _ => {
                eprintln!(
                    "DURDU — `--apply` için onay gerekli:\n  \
                     WFD_ARCHIVE_CONFIRM={expected}\n\
                     (DATABASE_URL'deki veritabanı adı; yanlış veritabanına koşmayı önler)"
                );
                std::process::exit(2);
            }
        }
    }

    let pool = connect(&db).await;

    let orgtnt_id: Uuid = GOLDEN_ORGTNT_ID.parse().expect("orgtnt uuid");
    let wfd_id: Uuid = GOLDEN_WFD_ID.parse().expect("wfd uuid");
    let key = wf_wfd::storage::s3_key(orgtnt_id, wfd_id, GOLDEN_VERSION);

    // 1) Arşivlenecek satırlar — kuru koşumun asıl çıktısı.
    println!("--- Arşivlenecek satırlar (is_active = true olanlar) ---");
    let meta_by_status = sqlx::query(
        "SELECT status, count(*) AS n FROM wf.wfd_meta
          WHERE is_active AND wfd_id <> $1 GROUP BY status ORDER BY status",
    )
    .bind(wfd_id)
    .fetch_all(&pool)
    .await
    .expect("wfd_meta sayımı");
    let mut meta_total: i64 = 0;
    if meta_by_status.is_empty() {
        println!("  {:>8}  wf.wfd_meta", 0);
    } else {
        for row in &meta_by_status {
            let status: String = row.get("status");
            let n: i64 = row.get("n");
            meta_total += n;
            println!("  {n:>8}  wf.wfd_meta  status={status}");
        }
    }
    let template_total = count_active(&pool, "wf.wfd_template").await;
    println!("  {template_total:>8}  wf.wfd_template");
    println!("  {:>8}  TOPLAM\n", meta_total + template_total);

    // 2) Seed'in ön koşulları. Proje satırı yoksa göçün `INSERT … SELECT`i hiçbir şey
    //    yazmaz — bunu apply'dan ÖNCE görmek gerekir, sonra sessizce eksik kalır.
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM wf.project WHERE orgtnt_id = $1 AND name = $2",
    )
    .bind(orgtnt_id)
    .bind(GOLDEN_PROJECT)
    .fetch_optional(&pool)
    .await
    .expect("proje sorgusu");

    let seed_exists: bool = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(wfd_id)
    .fetch_one(&pool)
    .await
    .expect("seed sorgusu")
        > 0;

    println!("--- v2.3 golden seed'i ---");
    println!("  wfd_id      {GOLDEN_WFD_ID}");
    println!("  ad/sürüm    {GOLDEN_NAME} v{GOLDEN_VERSION}  (doc {GOLDEN_DOC_ID}@{GOLDEN_DOC_VERSION})");
    match project_id {
        Some(p) => println!("  proje       {GOLDEN_PROJECT} ({p})"),
        None => println!("  proje       ! BULUNAMADI — seed satırı YAZILMAZ"),
    }
    println!("  meta satırı {}", if seed_exists { "zaten var — atlanacak" } else { "yazılacak" });
    println!("  blob        {key}  ({} bayt)", GOLDEN_JSON.len());

    let store = build_operator(&StorageConfig::from_env()).expect("storage operator");
    let blob_exists = store.exists(&key).await.unwrap_or(false);
    println!("  blob durumu {}\n", if blob_exists { "zaten var — ÜZERİNE YAZILACAK" } else { "yok — yazılacak" });

    if !apply {
        println!("KURU KOŞUM — hiçbir şey yazılmadı. Uygulamak için: --apply");
        return;
    }

    // 3) Arşivleme + seed satırı TEK transaction: yarım arşivlenmiş katalog, motorun
    //    okuyamadığı belgelerin bir kısmını aktif bırakırdı.
    let mut tx = pool.begin().await.expect("tx");
    // Seed satırı arşivlemenin DIŞINDA: betik iki kez koşarsa (blob hatası sonrası
    // yeniden koşum gibi) `WHERE is_active` yeni golden'ı da kapatır ve `ON CONFLICT
    // DO NOTHING` onu geri AÇMAZ — katalog boş kalırdı.
    let archived_meta = sqlx::query(
        "UPDATE wf.wfd_meta SET is_active = false, updated_at = now()
          WHERE is_active AND wfd_id <> $1",
    )
    .bind(wfd_id)
    .execute(&mut *tx)
    .await
    .expect("wfd_meta arşivleme")
    .rows_affected();
    let archived_template = sqlx::query(
        "UPDATE wf.wfd_template SET is_active = false, updated_at = now() WHERE is_active",
    )
    .execute(&mut *tx)
    .await
    .expect("wfd_template arşivleme")
    .rows_affected();

    let seeded = sqlx::query(
        "INSERT INTO wf.wfd_meta (
             wfd_id, orgtnt_id, project_id, name, version, s3_key, status, is_active,
             description, owner, doc_id, doc_version
         )
         SELECT $1, p.orgtnt_id, p.project_id, $2, $3, $4, 'published', true, $5, 'admin', $6, $7
           FROM wf.project p
          WHERE p.orgtnt_id = $8 AND p.name = $9
         ON CONFLICT (project_id, name, version) DO NOTHING",
    )
    .bind(wfd_id)
    .bind(GOLDEN_NAME)
    .bind(GOLDEN_VERSION)
    .bind(&key)
    .bind(GOLDEN_DESCRIPTION)
    .bind(GOLDEN_DOC_ID)
    .bind(GOLDEN_DOC_VERSION)
    .bind(orgtnt_id)
    .bind(GOLDEN_PROJECT)
    .execute(&mut *tx)
    .await
    .expect("golden seed")
    .rows_affected();
    tx.commit().await.expect("commit");

    println!("--- DB ---");
    println!("  {archived_meta} wfd_meta + {archived_template} wfd_template satırı arşivlendi");
    println!("  {seeded} seed satırı yazıldı\n");

    // 4) Blob SONRA: satır yazılamadıysa yetim blob bırakmayalım.
    store
        .write(&key, GOLDEN_JSON.as_bytes().to_vec())
        .await
        .unwrap_or_else(|e| panic!("golden blob yazılamadı ({key}): {e}\n  satırlar YAZILDI, betiği yeniden koş"));
    println!("--- Storage ---");
    println!("  {key} yazıldı\n");

    println!("--- Bitti ---");
    println!("  Katalogda aktif kalan tek belge v2.3 golden'ıdır. Arşivlenen belgelerin");
    println!("  metni storage'da DURUYOR; tarifler v2.3 diliyle yeniden kurulur (R07/S5).");
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

async fn count_active(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table} WHERE is_active"))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("sayım başarısız ({table}): {e}"))
}

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
