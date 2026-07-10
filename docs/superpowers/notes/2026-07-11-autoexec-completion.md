# Autoexec tamamlama — kararlar ve doğrulama (2026-07-11)

Görev: sql/rest/calc autoexec'lerin engine + UI seviyesinde tam implementasyonu;
SQL'e günlük hayatta çok kullanılan DB çeşitlerinin eklenmesi. Kesintisiz çalışma
istendi; ara kararlar burada — değerlendirmen için.

## Alınan kararlar

1. **DB çeşitleri = 9 seçilebilir tür, 4 wire protokolü.** PostgreSQL, MySQL,
   MariaDB, SQL Server, SQLite, CockroachDB, Amazon Redshift, TimescaleDB, TiDB.
   Alias'lar engine'de wire protokolüne çözülür (`DbDriver::parse`), UI'da ayrı
   türler olarak sunulur (doğru varsayılan port + etiketle). Gerekçe: gerçek
   sürücü sayısını şişirmeden en yaygın DB'leri kapsamak.
2. **Oracle bilinçli atlandı**: Rust `oracle` crate'i Oracle Instant Client native
   kütüphanesi ister (deploy yükü). İstersen ayrı iş olarak ele alalım.
   MongoDB/Redis SQL olmadığı için kapsam dışı.
3. **`config.result` UI'dan kaldırıldı** (REST/SQL modallarındaki "Result" tablosu).
   Engine bu alanı hiç okumuyordu — ölü alandı. Yerine üç tipte de config modalına
   "Sonuç → ctx (wfes_effects)" tablosu kondu; gerçek mekanizma olan
   `wfes_effects.set` + `$exec.result.*`'ı düzenliyor. `config.result` tipi ve
   export/import'u eski dokümanların round-trip'i için korunuyor.
4. **REST auth kısayolu** (`bearer/basic/api_key`) header'lardan ayrı bir config
   bloğu olarak eklendi; headers ile birlikte kullanılabilir, çakışmada auth kazanır.
5. **tiberius'a `tds73` feature'ı eklendi** — `default-features=false` bunu
   düşürmüştü; SQL Server 2008+ `date/time2/datetimeoffset` tipleri ve decimal
   FromSql bunun arkasında. Davranış değişikliği: TDS 7.3 protokolü (SQL 2008+
   gerektirir; 2005 ve öncesi desteklenmez — kabul edilebilir).
6. **Migration**: `wf.db_connection.driver` CHECK constraint'i 9 türe genişletildi
   (`20260711000001_db_connection_drivers.sql`) — psql ile canlı DB'ye uygulandı.
7. **SQLite test bağlantısı dosyayı OLUŞTURMAZ** (mode=rwc verilmedi) — var olmayan
   yol test hatası döner; "test" semantiği için doğru buldum.
8. **AUTOEXEC_GUIDE.md yeniden yazıldı** — eski içerik var olmayan
   `crates/wfe/src/autoexec/` modülünü ve v2.1 formatını anlatıyordu.

## Bulunan ve düzeltilen bug'lar (testler yakaladı)

- **bind_params sıralama bug'ı (kritik)**: değerler map (alfabetik) sırasında
  push ediliyordu; `?` işaretli MySQL/SQLite'ta yer tutucu metin sırası ile
  eşleşmiyordu → parametreler yanlış kolonlara bağlanıyordu. Metin-sıralı
  tarayıcıyla yeniden yazıldı; `::cast` ve string literal koruması eklendi.
- **sqlx-mysql bool decode tuzağı**: bool decode TÜM int tiplerini kabul ediyor
  (`SMALLINT 2` → `true`, `COUNT(*)` → `true`). bool sayısal zincirin sonuna alındı.
  MariaDB e2e ile canlı yakalandı/doğrulandı.
- **SQLite NULL decode**: NULL, `i64`'e 0 olarak decode oluyor — değerin gerçek
  tipi (`try_get_raw().type_info()`) üzerinden ayrıştırıldı.
- **pg int4/numeric/uuid/timestamp null bug'ı** (bilinen eksik): try_get zinciri
  genişletilerek giderildi; canlı doğrulandı.
- **AutoexecTestModal**: hardcoded `localhost:3000` → Çalıştırma ayarlarındaki
  baseUrl; hata durumu artık gösteriliyor (önceden sessizce yutuluyordu).

## Doğrulama

- `cargo test --workspace` yeşil (yeni: bind_params 5 birim, sqlite in-memory 2,
  driver alias 1, REST auth 1).
- Editör: `npm run build` (tsc dahil) + vitest 347/347 (yeni round-trip dosyası:
  headers/auth/form/PATCH korunumu + ajv + fixed-point).
- Canlı e2e (release binary yeniden başlatıldı):
  - pg default pool: `int2/int4/numeric/timestamptz/uuid` gerçek değerler; param
    metin-sırası + `::cast` doğru.
  - SQLite: bağlantı oluştur/test/parametreli sorgu/çoklu satır ✓ (scratchpad dosyası).
  - MariaDB (geçici docker konteyneri): bağlantı testi, decimal/datetime/smallint,
    COUNT/SUM ✓ — konteyner ve test bağlantıları temizlendi.
  - REST (lokal echo sunucusu): PATCH + headers($wfe_id) + bearer($ctx) + JSON body,
    api_key + form-urlencoded, 404 hata gövdesi, JSON-olmayan yanıt `{body}` ✓.
  - `/autoexec/test` `request_info` (çözülmüş config) dönüyor; modal gösteriyor.

## Senin değerlendirmen için açık uçlar

- MSSQL canlıda smoke-test edilmedi (lokal SQL Server yok; tiberius yolu birim
  seviyesinde hazır). İstersen bir `mcr.microsoft.com/mssql/server` konteyneriyle
  koşarım (~1.5GB imaj).
- SQLite dosya yolu engine sürecinin dosya sistemine göredir — çok-tenant'lı
  üretimde path allowlist'i düşünülebilir (şu an kısıt yok).
- `request_info` çözülmüş auth token'ını içerir (test aracı, WFD sahibinin kendi
  config'i). Maskeleme istersen söyle.
- REST'te OAuth2 client-credentials, retry-aware cache, GraphQL executor — gelecek.
