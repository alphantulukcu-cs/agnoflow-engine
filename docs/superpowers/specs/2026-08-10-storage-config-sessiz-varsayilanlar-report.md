# Rapor: Deployment storage konfigürasyonunda sessiz varsayılanlar

**Tarih:** 2026-08-10
**Kapsam:** `agnoflow-backend` — `wf_wfd::StorageConfig` / `build_operator`
**Durum:** Bugün staging DOĞRU yapılandırılmış. Rapor mevcut bir arıza değil, **hatanın sessiz olması** riskini anlatıyor.

## 1. Bulgu

`build_operator` S3 kurarken üç alanı da "yoksa uydur" mantığıyla dolduruyor:

```rust
// crates/wfd/src/storage.rs:52-61
let mut builder = services::S3::default()
    .bucket(cfg.s3_bucket.as_deref().unwrap_or("wf-engine"))      // ← sessiz varsayılan
    .region(cfg.s3_region.as_deref().unwrap_or("us-east-1"));     // ← sessiz varsayılan
if let Some(ep) = cfg.s3_endpoint.as_deref() {
    builder = builder.endpoint(ep).disable_config_load().disable_ec2_metadata();
}                                                                  // ← endpoint YOKSA hiçbiri çağrılmaz
```

Üç ayrı sonuç doğuyor:

1. **`STORAGE_S3_ENDPOINT` boşsa istekler AWS'e gider.** Endpoint verilmediğinde opendal varsayılan AWS uçlarını kullanır. Garage/MinIO kullanan bir kurulumda bu, "depo yanlış hedefe konuşuyor" demektir.
2. **Ambient credential zinciri yalnız endpoint verilince kapanıyor.** `disable_config_load()` (env / `~/.aws/config`) ve `disable_ec2_metadata()` sadece `if let Some(ep)` bloğunun içinde. Endpoint boşken pod/makinedeki AWS ayarları veya instance metadata credential'ı devreye girebilir — konfigürasyon eksikken sistemin **kimlik bulmayı denemesi**, bulmamasından daha kötüdür.
3. **`STORAGE_S3_BUCKET` boşsa `wf-engine` bucket'ına yazılır.** İsim çakışırsa veri var olan yanlış bir bucket'a düşer; çakışmazsa runtime hatası.

`STORAGE_S3_REGION` için de aynısı: verilmezse `us-east-1` varsayılır.

## 2. Neden şimdi gündeme geldi

2026-08-10'da ek-belge deposunun **WFD başına** (`$env`) konfigürasyonu için bir yayın kapısı eklendi: belge toplayan bir akış, depo ayarları girilmeden yayınlanamıyor ve `ATTACHMENT_STORAGE_S3_ENDPOINT` bilinçli olarak **zorunlu** tutuldu — gerekçesi yukarıdaki 1. ve 2. maddeler.

Aynı gerekçe **deployment düzeyindeki iki yol** için hâlâ geçerli ve orada hiçbir kapı yok:

| Yol | Okunduğu yer | Ne tutuyor |
|---|---|---|
| `STORAGE_*` (WFD JSON deposu) | `wfd/src/storage.rs:33-40` → `server/src/main.rs:45` | WFD dokümanlarının TÜM versiyonları, layout companion, senaryo sidecar'ları, tenant logo/favicon baytları |
| `ATTACHMENT_STORAGE_*` (ek-belge deployment varsayılanı) | `server/src/config.rs:53-72` → `main.rs:47` | `$env`'de depo tanımlamamış akışların belgeleri |

## 3. Etki

**WFD JSON deposu kritik yolda.** O bucket okunamazsa `WfdAdapter::fetch` başarısız olur; yani sadece tasarım ekranı değil, **her WFE işlemi** (start, apply, timer sweep, retry, escalation) çalışmaz. Ek-belge deposundan daha geniş bir yüzey.

**Arıza sessiz ve gecikmeli.** `build_operator` ağ çağrısı yapmaz; yanlış/eksik konfigürasyonla da başarıyla döner. `main.rs` sadece `expect("storage init failed")` der ve sunucu **sağlıklı şekilde açılır**. Hata ilk gerçek okuma/yazmada ortaya çıkar — deploy'dan saatler sonra, kullanıcı bir akış açmaya çalıştığında. Flux/K8s hiçbir uyarı vermez, pod Running'dir.

**En kötü senaryo veri kaybı değil, veri sızması.** Endpoint boş + ortamda AWS credential'ı varsa, tenant dokümanları ve marka varlıkları niyet edilmeyen bir hedefe yazılır. Bu, tespit edilmesi en zor hata sınıfı: hiçbir yerde hata log'u yoktur, sadece veri "başka yerdedir".

**Tetikleyici sıradan.** `agnoflow-infra/apps/agnoflow-backend/configmap.yaml`da bir satırın silinmesi, bir anahtarın yazım hatası, SOPS secret'ının eksik uygulanması ya da yeni bir ortam kurulurken bir satırın atlanması yeter. Bugün doğru olması, yarın doğru kalacağının garantisi değil — kod bunu zorlamıyor.

## 4. Öneri

**A — Açılışta fail-fast doğrulama (asıl iş).** `StorageConfig` için saf bir `validate()`: `backend == S3` iken `s3_bucket`, `s3_region`, `s3_endpoint` ve credential'lar zorunlu; eksikse sunucu okunabilir bir mesajla açılmayı REDDETSİN. `JWT_SECRET env var required` ile aynı desen ve aynı gerekçe. Hem `cfg.storage` hem `cfg.attachment_storage` aynı fonksiyondan geçer. Ağ çağrısı yok, birim testi kolay, K8s tarafında CrashLoopBackOff + net log = deploy anında yakalanır.

**B — Sessiz varsayılanları kaldır.** `unwrap_or("wf-engine")` ve `unwrap_or("us-east-1")` düşer; A maddesi bunları zaten zorunlu kılar. "Uydurulmuş" bir hedefe yazma imkânı tamamen kapanır.

**C — Ambient credential zincirini koşulsuz kapat.** `disable_config_load()` / `disable_ec2_metadata()` `if let Some(ep)` bloğundan ÇIKARILIP her S3 kurulumunda çağrılsın; gerçekten IRSA/instance-role ile çalışmak istenirse bu açık bir tercih olsun (ör. `STORAGE_S3_AMBIENT_CREDENTIALS=1`). Konfigürasyon eksikken kimlik "bulunması" bir kolaylık değil, risktir.

**D — Opsiyonel, ayrı iş: açılışta duman testi.** Tek bir `stat`/`list` çağrısıyla endpoint+credential+bucket üçlüsü boot'ta doğrulanır. Ağ bağımlılığı getirir (depo yavaşsa açılışı bekletir), bu yüzden A/B/C'den ayrı ve muhtemelen "uyarı logu + `/health` degraded" biçiminde düşünülmeli.

Tahmini iş: A+B+C tek küçük PR — `wfd/src/storage.rs` + `server/src/config.rs`/`main.rs`, yanına birkaç birim testi.

## 5. İlişkili ikinci konu (aynı PR'a girmesi gerekmez)

Ek-belge deposunun `$env` kapısı **yayın anında** çalışıyor, **koşum anında** değil. Yayınlandıktan sonra ortam değişkeni silinir ya da boşaltılırsa `config_from_env` `None` döner ve depo sessizce deployment varsayılanına düşer — yayın sırasında doğru olan bir akış, sonradan başka bir hedefe yazmaya başlar. Olası önlem: `wf.wfd_env_var` üzerinde silme/boşaltma sırasında "bu anahtar yayınlanmış bir akışın deposunu tanımlıyor" kontrolü, ya da runtime'da fallback yerine hata.

## Yer imleri

| Dosya | Ne var |
|---|---|
| `crates/wfd/src/storage.rs:44-69` | `build_operator` — sessiz varsayılanlar + koşullu `disable_*` |
| `crates/wfd/src/storage.rs:24-42` | `StorageConfig::from_env` — `STORAGE_*` okuması |
| `crates/server/src/config.rs:53-72` | `attachment_storage_from_env` — ek-belge deployment varsayılanı |
| `crates/server/src/main.rs:45-49` | iki Operator'ün kurulduğu yer (`expect` dışında doğrulama yok) |
| `crates/server/src/attachment_store.rs:54-78` | 2026-08-10 yayın kapısının zorunlu anahtar listesi + gerekçesi |
| `agnoflow-infra/apps/agnoflow-backend/configmap.yaml:12-20` | staging'in mevcut (doğru) değerleri — Garage, in-cluster endpoint |
