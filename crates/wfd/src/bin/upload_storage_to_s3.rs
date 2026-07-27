//! Tek seferlik migration: lokal `storage/` (FS) içindeki tüm WFD nesnelerini
//! anahtarları BİREBİR koruyarak hedef S3'e (Garage) yükler.
//!
//! Lokal dosyalar zaten tenant-izole düzendedir (`{orgtnt_id}/wfd/{wfd_id}/{version}.json`
//! + `.layout.json`), dolayısıyla tenant ayrımı anahtarda korunur — yeniden anahtarlama YOK.
//!
//! Kaynak (FS)  : `LOCAL_STORAGE_PATH` (default `./storage`)
//! Hedef (S3)   : `.env`'deki `STORAGE_S3_*` (build_operator). `STORAGE_BACKEND=s3` olmalı.
//!
//! Idempotent: hedefte zaten VAR olan ve içeriği aynı olan nesneler atlanır; farklıysa uyarı
//! basılır (immutable modelde olmamalı). Varsayılan DRY-RUN; gerçek yükleme için `--apply`.
//!
//! Çalıştırma (repo kökünden, `.env` yüklü):
//!   cargo run -p wf-wfd --bin upload_storage_to_s3            # dry-run
//!   cargo run -p wf-wfd --bin upload_storage_to_s3 -- --apply # yükle
use opendal::{services, Operator};
use wf_wfd::{build_operator, StorageBackend, StorageConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let apply = std::env::args().any(|a| a == "--apply");

    let local_root = std::env::var("LOCAL_STORAGE_PATH").unwrap_or_else(|_| "./storage".into());
    let local: Operator = Operator::new(services::Fs::default().root(&local_root))?.finish();

    let cfg = StorageConfig::from_env();
    if !matches!(cfg.backend, StorageBackend::S3) {
        return Err("STORAGE_BACKEND=s3 değil — hedef S3 olmalı (.env kontrol et)".into());
    }
    let remote = build_operator(&cfg)?;

    println!(
        "Kaynak (FS): {local_root}  ->  Hedef (S3): bucket={:?} endpoint={:?}\nMod: {}",
        cfg.s3_bucket,
        cfg.s3_endpoint,
        if apply { "APPLY" } else { "DRY-RUN" }
    );

    let entries = local.list_with("").recursive(true).await?;
    let (mut uploaded, mut exists_same, mut exists_diff, mut skipped, mut problems) =
        (0u32, 0u32, 0u32, 0u32, 0u32);

    for e in entries {
        let key = e.path();
        // dizinler ve .gitkeep atlanır
        if key.ends_with('/') || key.is_empty() {
            continue;
        }
        if key.rsplit('/').next() == Some(".gitkeep") {
            skipped += 1;
            continue;
        }

        let src = match local.read(key).await {
            Ok(b) => b.to_bytes(),
            Err(err) => {
                eprintln!("  KAYNAK OKUNAMADI {key}: {err}");
                problems += 1;
                continue;
            }
        };

        // hedefte var mı? aynıysa atla, farklıysa dokunma + uyar (immutable ihlali işareti)
        match remote.read(key).await {
            Ok(existing) => {
                if existing.to_bytes() == src {
                    println!("  = zaten var (aynı): {key}");
                    exists_same += 1;
                } else {
                    eprintln!("  ! hedefte FARKLI içerik var, DOKUNULMADI: {key}");
                    exists_diff += 1;
                }
                continue;
            }
            Err(err) if err.kind() == opendal::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("  HEDEF STAT HATASI {key}: {err}");
                problems += 1;
                continue;
            }
        }

        if apply {
            match remote.write(key, src).await {
                Ok(_) => {
                    println!("  + yüklendi: {key}");
                    uploaded += 1;
                }
                Err(err) => {
                    eprintln!("  YAZMA HATASI {key}: {err}");
                    problems += 1;
                }
            }
        } else {
            println!("  + yüklenecek: {key}");
            uploaded += 1;
        }
    }

    println!(
        "\nÖzet: yüklenen={uploaded}, zaten-var-aynı={exists_same}, \
         hedefte-farklı={exists_diff}, atlanan={skipped}, sorun={problems}"
    );
    if !apply {
        println!("DRY-RUN — hiçbir şey yazılmadı. Gerçekten yüklemek için `-- --apply` ekle.");
    }
    Ok(())
}
