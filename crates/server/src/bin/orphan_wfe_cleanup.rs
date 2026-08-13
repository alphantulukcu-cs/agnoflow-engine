//! Öksüz WFE temizliği: WFD SATIRI OLMAYAN iş akışı örneklerini siler.
//!
//! Öksüz = `wf.wfe.wfd_id` için `wf.wfd_meta`'da HİÇ satır yok. Bu, motor
//! açısından "tarifi kaybolmuş iş" demektir: node etiketleri, c_a kuralları,
//! görünürlük grant'ları — hiçbiri hesaplanamaz. Detay ucu (WFD'yi okuyan yol)
//! 500 verir, projeksiyon üretilemez, backfill/worker satırı atlar.
//!
//! DİKKAT — "pasifleştirilmiş" ile "silinmiş" AYNI ŞEY DEĞİL: `WfdStore::fetch`
//! yalnız `is_active AND status='published'` satırı görür, dolayısıyla motor
//! ikisini de `wfd not found` diye bildirir. Bu araç ikisini AYRI raporlar,
//! çünkü çözümleri farklıdır: satır DURUYORSA sorun yayın durumundadır (silmek
//! veri kaybı olur), satır YOKSA gerçekten öksüzdür.
//!
//! VARSAYILAN KURU KOŞUMDUR. Silmek için `--apply`:
//!   `DATABASE_URL=... cargo run -p wf-server --bin orphan_wfe_cleanup -- --apply`
//!
//! Silme sırası FK zincirini izler (`wf.wfe`ye bakan tablolar): önce not
//! defteri, sonra çağrı bağları, WFAH ve dynctx, en son WFE satırı. `wfe_branch`
//! ve `wfe_attachment` ON DELETE CASCADE olduğu için kendiliğinden gider.
//! TEK TRANSACTION: yarım temizlik, öksüz satırdan daha kötü bir durum olurdu.

use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Row};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let apply = std::env::args().any(|a| a == "--apply");
    let db = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(|c, _| {
            Box::pin(async move {
                c.execute("SET search_path TO org, public").await?;
                Ok(())
            })
        })
        .connect(&db)
        .await
        .expect("db connect");

    println!(
        "=== ÖKSÜZ WFE TEMİZLİĞİ === (mod: {})\n",
        if apply { "SİL" } else { "KURU KOŞUM" }
    );

    // 1) WFD satırı HİÇ olmayanlar = gerçek öksüzler.
    let orphans = sqlx::query(
        "SELECT e.wfe_id, e.wfd_id, e.wfd_version, e.status, e.created_at
           FROM wf.wfe e
           LEFT JOIN wf.wfd_meta m ON m.wfd_id = e.wfd_id
          WHERE m.wfd_id IS NULL
          ORDER BY e.created_at",
    )
    .fetch_all(&pool)
    .await
    .expect("öksüz sorgusu");

    // 2) Satırı DURAN ama motorun okuyamadıkları (pasif / yayında değil).
    //    Bunlar öksüz DEĞİL — silinmemeli, yayın durumu düzeltilmeli.
    let inactive = sqlx::query(
        "SELECT e.wfe_id, m.status, m.is_active
           FROM wf.wfe e
           JOIN wf.wfd_meta m ON m.wfd_id = e.wfd_id
          WHERE m.is_active = false OR m.status <> 'published'
          ORDER BY e.created_at",
    )
    .fetch_all(&pool)
    .await
    .expect("pasif sorgusu");

    if !inactive.is_empty() {
        println!("--- WFD'si DURUYOR ama motor okuyamıyor (SİLİNMEZ) ---");
        for r in &inactive {
            println!(
                "  {} — wfd status={} is_active={}",
                &r.get::<Uuid, _>("wfe_id").to_string()[..8],
                r.get::<String, _>("status"),
                r.get::<bool, _>("is_active")
            );
        }
        println!("  → Çözüm silmek DEĞİL: sürümü yeniden yayınlamak ya da fetch\n     kapısını gözden geçirmek.\n");
    }

    if orphans.is_empty() {
        println!("Öksüz WFE yok.");
        return;
    }

    println!("--- Öksüz WFE'ler (WFD satırı HİÇ yok) ---");
    let mut ids: Vec<Uuid> = Vec::new();
    for r in &orphans {
        let wfe_id: Uuid = r.get("wfe_id");
        ids.push(wfe_id);
        // Silinecek çocuk satırlar sayılır: ne kaybedildiği rakamla görünsün.
        let counts: Vec<(String, i64)> = vec![
            ("wfah", count(&pool, "wf.wfah", wfe_id).await),
            ("dynctx", count(&pool, "wf.wfe_dynctx", wfe_id).await),
            ("not", count(&pool, "wf.wfe_note", wfe_id).await),
            ("kol", count(&pool, "wf.wfe_branch", wfe_id).await),
            ("belge", count(&pool, "wf.wfe_attachment", wfe_id).await),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let detail: Vec<String> = counts
            .iter()
            .filter(|(_, v)| *v > 0)
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        println!(
            "  {} [{}] wfd={} v{}  {}",
            &wfe_id.to_string()[..8],
            r.get::<String, _>("status"),
            &r.get::<Uuid, _>("wfd_id").to_string()[..8],
            r.get::<i32, _>("wfd_version"),
            if detail.is_empty() {
                "(çocuk satır yok)".into()
            } else {
                detail.join(" ")
            }
        );
    }

    if !apply {
        println!("\nKURU KOŞUM — hiçbir şey silinmedi. Silmek için: --apply");
        return;
    }

    let mut tx = pool.begin().await.expect("tx");
    // Sıra FK zinciridir; CASCADE'li tablolar (wfe_branch, wfe_attachment) ve
    // not çocukları (wfe_note_file/read) kendiliğinden gider.
    for stmt in [
        "DELETE FROM wf.wfe_note WHERE wfe_id = ANY($1)",
        "DELETE FROM wf.wfe_call WHERE caller_wfe_id = ANY($1) OR callee_wfe_id = ANY($1)",
        "DELETE FROM wf.wfah WHERE wfe_id = ANY($1)",
        "DELETE FROM wf.wfe_dynctx WHERE wfe_id = ANY($1)",
        "DELETE FROM wf.wfe WHERE wfe_id = ANY($1)",
    ] {
        let n = sqlx::query(stmt)
            .bind(&ids)
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("silme başarısız ({stmt}): {e}"))
            .rows_affected();
        println!("  {n:>4} satır ← {stmt}");
    }
    tx.commit().await.expect("commit");
    println!("\n{} öksüz WFE silindi.", ids.len());
}

async fn count(pool: &PgPool, table: &str, wfe_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table} WHERE wfe_id = $1"))
        .bind(wfe_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}
