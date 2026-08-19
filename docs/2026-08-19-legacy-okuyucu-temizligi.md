# Geriye uyum (legacy) okuyucu temizliği — görev listesi

**Karar (2026-08-19, kullanıcı):** *"Yayınlanmış işlerin sonradan okunabilmesi için o
zamanki kuralları tutmak gerekiyor. Ama bu durum production'a çıktıktan sonra geçerli.
Şu anda hâlâ uygulamayı geliştiriyoruz ve oluşturduğumuz tüm WFD/WFE'ler sadece test
amaçlı — production'a çıkmadan sıfırlanacak."*

Yani **"wire formatı değişince okuyucu kalır" kuralı production'dan SONRA yürürlüktedir.**
Şu an geriye uyum kodu saf borçtur: okunmayan bir şekli anlamak için taşınan kod, test ve
belge. Bu dosya kalan örnekleri iki gruba ayırır.

Aynı kararla **zaten silinenler** (2026-08-19, bu oturumda yapıldı):

- GLB `__gt__` anahtar ailesi okuyucusu — `agnoflow-frontend/src/utils/legacyGlobalAction.ts`
  (188 satır) + `src/tests/globalAction.legacy.test.ts` + `wfdImport`'un ön-geçişi.
- Context şemasındaki `$ref` sözdizimi — şemanın üç kopyasından çıkarıldı, motor
  (`ctx_types::def_name`, `validator::deref_defs`), editör (`contextDefs`) ve portal
  (`lib/contextTypes`) çözücülerinden kaldırıldı; `validator` `context_ref_removed` ile
  reddediyor. Adlandırılmış tipin tek sözdizimi `format`.

---

## A grubu — ŞİMDİ silinebilir (wire/şekil okuyucusu)

Bunların hiçbiri veri göçü değil: hepsi "eski şekilde yazılmış bir belgeyi/kaydı
anlamak" için duran kod. Belgeler sıfırlanacağı için karşılığı kalmıyor.

| # | Yer | Ne okuyor / neden borç |
|---|---|---|
| A1 | `wfe-core/src/types/wfd_v22.rs` → `Wfd::terminal_when` | v1'den kalan alan. Motor DEĞERLENDİRMİYOR, validator `terminal_when_ignored` uyarısı basıyor, yeniden serileştirmede düşüyor. Alan + uyarı + `skip_serializing` kalkabilir; `docs/spec/schema.json`'un üç kopyasından da çıkarılır. |
| A2 | `wfe-core/src/types/wfd_v22.rs` → `AttachmentRef::Group(String)` | `untagged` çift biçim: düz `"grup"` = "v2.2'nin ilk biçimi; eski dosyalar aynen çalışır". Tek biçim (`{group, actions?}`) bırakılıp `AttachmentRef` sade bir struct'a indirilebilir. Editör tarafı (`store/wfd.store.ts` `AttachmentRefMeta`, `useExport` düz-string üretimi) birlikte güncellenir. **DİKKAT:** `actions` `Option<Vec>` ayrımı (verilmedi = tümü, `[]` = hiçbiri) KORUNMALI — o legacy değil, anlam. |
| A3 | `agnoflow-frontend/src/utils/contextDefs.ts` → `normalizeNamedTypes` | 2026-08-19'da `format` göçü için eklendi: `$defs`'te karşılığı olmayan STANDART `format` değerlerini düşürüyor, `data-url` → `x-wf-document` çeviriyor. Aynı sınıf borç; silinirse `format: "date-time"` taşıyan eski taslak `context_format_unknown` ile reddedilir (istenen davranış). |
| A4 | `agnoflow-frontend/src/utils/scenarioSidecar.ts` → `loadLegacyScenarios` + `ScenarioSection`'daki "Bu tarayıcıdaki senaryoları içeri aktar" kartı + `scenarioServerMigration.test.tsx`'in göç bölümü | Senaryolar 2026-08-07'de localStorage'dan sunucu sidecar'ına taşındı; bu yol eski tarayıcı verisini aktarıyor. |
| A5 | `wfd/src/storage.rs` → `legacy_layout_key` + `wfd/src/bin/migrate_tenant_storage.rs` | Tenant-öncesi layout anahtarı ve göç aracı. |
| A6 | `wfe/src/sim.rs` → `SimState`'in `#[serde(default)]` alanları (`end_terminal`, `branches`, `join_target`, `join_threshold`, `join_when`, `pending_calls`, `attachments`, `notes`) | Yorumlar "bu alandan önce üretilmiş sim_state blob'ları onsuz da parse edilir" diyor. `sim_state` istemcide taşınan GEÇİCİ durumdur (DB'de değil) — eski blob endişesi yalnız açık bir tarayıcı sekmesi için geçerli. `default`lar teknik olarak zararsız; YORUMLAR yanıltıcı, en azından güncellenmeli. Alanları zorunlu yapmak istemci-sunucu sürüm uyumunu sıkılaştırır — düşük öncelik. |
| A7 | `agnoflow-work-pool-portal/src/features/workflows/api.ts` → `startWorkflow(..., wfeId?)` | Kaldırılmış rezerve akışının kullanılmayan parametresi ("imza korunuyor"). |
| A8 | `agnoflow-work-pool-portal/src/components/DynamicForm.tsx` → `parseEngineFieldError`'ın `context zorunlu alanı '…'` kalıbı | WOR-70'te kalkan motor mesajına karşı geriye uyum; yorumu da "yeni sürümde hiç eşleşmez" diyor. |
| A9 | `agnoflow-work-pool-portal/src/features/instances/NoteTimeline.tsx` → "Serbest" rozeti | 2026-08-11 öncesi yayınlanmış, aksiyona bağlı olmayan notlar için. |
| A10 | `agnoflow-work-pool-portal/src/components/DynamicForm.tsx` → `isDoc`'un `contentMediaType` / `contentEncoding` dalları | Doküman alanını işaretlemenin eski yolları; güncel yol `x-wf-document` (ve A3 silinirse `format: 'data-url'` de gider). |

**Yapılırken:** her madde için (a) okuyucu kodu, (b) ona bağlı testler/fixture'lar,
(c) şema/validator karşılığı, (d) yorumlardaki "eski dosyalar aynen çalışır" ifadeleri
birlikte temizlenir. Silme sonrası: `cargo test --workspace`, editörde
`npm run typecheck && npx vitest run --project=unit`, portalda `npx tsc -b && npx vitest run`.

---

## B grubu — WFD/WFE SIFIRLAMASINDAN SONRA silinebilir (veri göçü)

Bunlar wire okuyucusu değil; **var olan DB satırlarını** kurtarmak/anlamak için var.
Test verisi silinince gerekçeleri de bitiyor.

| # | Yer | Neyi kurtarıyor | Sıfırlama sonrası |
|---|---|---|---|
| B1 | `wfe-core/src/v22/end_terminal.rs` (+ `visibility_backfill`in ön geçişi) | `wf.wfe.end_terminal` kolonu 2026-08-17'de eklendi; ondan önce sonlanmış satırlarda "hangi bitişe varıldı" yazmıyordu. Kanıtlardan (end_response + WFAH) tek aday kalırsa çıkarıyor. | Modül + backfill ön geçişi silinebilir (kolon kalır, artık her commit yazıyor). |
| B2 | `server/src/attachments.rs` → `enrich_with_meta` + "`uploaded` gerçeği DAİMA DEPODA" duruşu | `wf.wfe_attachment` tablosu 2026-08-11'de eklendi; ondan önce yüklenmiş dosyaların metadata satırı yok. | Metadata gerçeğin kaynağı yapılabilir (`uploaded` DB'den okunur), depo sorgusu kalkabilir → kapı yolunda bir I/O eksilir. |
| B3 | `wf.wfe.origin_orgu_id IS NULL` = "backfill bekliyor → eski davranış" (`matcher::authorize_anchored`, `visibility::sql`) | Çapa kolonu eklenmeden önce başlamış WFE'ler. | NULL dalı kaldırılıp kolon NOT NULL yapılabilir. |
| B4 | `wf.wfe_reservation` + saatlik süpürücü | Crash ağı (HTTP yüzeyi yok). Legacy DEĞİL ama sıfırlamada tablonun boşalacağı not edilsin. | Kalır. |

**Sıralama önerisi:** A grubu bağımsız, hemen yapılabilir. B grubu için önce
sıfırlama komutu (WFD sürümleri + WFE satırları + Garage'daki JSON/attachment
anahtarları), sonra B1-B3 temizliği.

---

## Silinmemesi gerekenler (yanlış pozitif listesi)

Bunlar "legacy" gibi okunuyor ama değil — dokunulmaz:

- `AttachmentRef.actions: Option<Vec>` ayrımı → "verilmedi" ile "boş verildi" ZIT anlamlı.
- `OrgPort::delegations` default `Ok(vec![])` → sim/mock portları için, wire değil.
- Token'sız okuma yolları (`routes/wfd.rs`, `routes/auth.rs` "eski davranış" yorumları)
  → araç/sim erişimi, geriye uyum değil yetki modeli.
- `resolver.rs`'deki "eski davranış bir mantık hatasıydı" yorumu → tarihçe notu, kod değil.
- `WfahView` sınıflandırması (`api-contract-v2 §2d`) → eski şekil zaten KIRILDI, okuyucu yok.
