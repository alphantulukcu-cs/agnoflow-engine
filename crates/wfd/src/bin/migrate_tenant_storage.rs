//! Tek seferlik migration: mevcut WFD storage nesnelerini tenant-izole düzene taşır.
//!
//! Eski (tek-tenant) düzen:  `wfd/{wfd_id}/{version}.json`  (+ `.layout.json`)
//! Yeni (tenant-izole) düzen: `{orgtnt_id}/wfd/{wfd_id}/{version}.json`
//!
//! Hem storage nesnesini taşır hem `wf.wfd_meta.s3_key` kolonunu günceller.
//! Layout companion'ı DB'de saklanMAZ; eski türetilmiş anahtardan yeni anahtara taşınır.
//!
//! Backend-agnostik: aynı opendal Operator hem local FS (`STORAGE_PATH` altı) hem
//! S3 (`STORAGE_S3_BUCKET`) için çalışır — `STORAGE_BACKEND` neyse ona yazar.
//!
//! Idempotent: zaten yeni düzende olan satırlar atlanır; tekrar çalıştırmak güvenli.
//! Varsayılan DRY-RUN (hiçbir şey değiştirmez). Gerçek taşıma için `--apply`.
//!
//! Çalıştırma (repo kökünden, `.env` yüklü ortamda):
//!   cargo run -p wf-wfd --bin migrate_tenant_storage            # dry-run
//!   cargo run -p wf-wfd --bin migrate_tenant_storage -- --apply # taşı

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;
use wf_wfd::{build_operator, storage, StorageConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let apply = std::env::args().any(|a| a == "--apply");
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL gerekli");

    let cfg = StorageConfig::from_env();
    let op = build_operator(&cfg)?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await?;

    let rows: Vec<(Uuid, Uuid, i32, String)> = sqlx::query_as(
        "SELECT wfd_id, orgtnt_id, version, s3_key FROM wf.wfd_meta ORDER BY created_at",
    )
    .fetch_all(&pool)
    .await?;

    println!(
        "{} WFD satırı bulundu. Backend: {:?}. Mod: {}",
        rows.len(),
        cfg.backend,
        if apply { "APPLY" } else { "DRY-RUN" }
    );

    let (mut moved, mut layouts, mut skipped, mut problems) = (0u32, 0u32, 0u32, 0u32);

    for (wfd_id, orgtnt_id, version, old_key) in rows {
        let new_key = storage::s3_key(orgtnt_id, wfd_id, version);

        if old_key == new_key {
            skipped += 1;
        } else {
            // --- WFD JSON ---
            match migrate_object(&op, &old_key, &new_key, apply).await {
                Ok(true) => {
                    if apply {
                        sqlx::query(
                            "UPDATE wf.wfd_meta SET s3_key=$1 WHERE wfd_id=$2 AND version=$3",
                        )
                        .bind(&new_key)
                        .bind(wfd_id)
                        .bind(version)
                        .execute(&pool)
                        .await?;
                    }
                    println!("  [json]   {old_key}  ->  {new_key}");
                    moved += 1;
                }
                Ok(false) => {
                    eprintln!("  [json]   KAYNAK YOK: {old_key} — DB s3_key güncellenMEDİ, elle bak");
                    problems += 1;
                }
                Err(e) => {
                    eprintln!("  [json]   HATA {old_key}: {e}");
                    problems += 1;
                }
            }
        }

        // --- layout companion (her satır için; JSON zaten taşınmış olsa bile eski
        //     layout kalmış olabilir) ---
        let old_layout = storage::legacy_layout_key(wfd_id, version);
        let new_layout = storage::layout_key(orgtnt_id, wfd_id, version);
        if old_layout != new_layout {
            match migrate_object(&op, &old_layout, &new_layout, apply).await {
                Ok(true) => {
                    println!("  [layout] {old_layout}  ->  {new_layout}");
                    layouts += 1;
                }
                Ok(false) => {} // layout yok — normal
                Err(e) => {
                    eprintln!("  [layout] HATA {old_layout}: {e}");
                    problems += 1;
                }
            }
        }
    }

    println!(
        "\nÖzet: json taşınan={moved}, layout taşınan={layouts}, zaten-yeni={skipped}, sorun={problems}"
    );
    if !apply {
        println!("DRY-RUN — hiçbir şey değişmedi. Gerçekten taşımak için `-- --apply` ekle.");
    }
    Ok(())
}

/// Kaynağı okuyup hedefe yazar, sonra kaynağı siler (read+write+delete — copy
/// desteği aramadan hem FS hem S3'te çalışır). Kaynak yoksa `Ok(false)`.
/// `apply=false` iken sadece kaynağın varlığını raporlar, dokunmaz.
async fn migrate_object(
    op: &opendal::Operator,
    from: &str,
    to: &str,
    apply: bool,
) -> Result<bool, opendal::Error> {
    match op.stat(from).await {
        Ok(_) => {}
        Err(e) if e.kind() == opendal::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    if !apply {
        return Ok(true);
    }
    let bytes = op.read(from).await?.to_bytes();
    op.write(to, bytes).await?;
    op.delete(from).await?;
    Ok(true)
}
