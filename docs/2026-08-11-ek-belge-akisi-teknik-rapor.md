# Ek Belge Akışı — Teknik Rapor

**Tarih:** 2026-08-11
**Kapsam:** `agnoflow-backend` + `agnoflow-work-pool-portal`
**Tasarım kararları:** `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md` (K1–K13)
**Sözleşme çapası:** `CLAUDE.md` → "Attachments (ek-belge) sözleşmesi", `docs/spec/decisions.md` Madde 8
**Teknik olmayan özet:** `docs/2026-08-11-ek-belge-akisi-genel-rapor.md`

---

## 1. Çıkış noktası

Bildirilen hata: *"portalda attachments'ı olan bir akış başlat dediğinde yetkisi yoksa ya da
yolda hata alsa bile o attachment'ları storage'a yazıyor."*

Kök neden tek bir kusur değil, sıranın kendisiydi. Dosya anahtarı
`attachments/{wfe_id}/{grup}/{item}` ve `wfe_id` start'ın İÇİNDE doğuyordu; bu yüzden
2026-08-07'de sıra tersine çevrilmişti:

```text
POST   /wfe/reserve                            → wfe_id (wf.wfe satırı YOK)
PUT    /wfe/{wfe_id}/attachments/{grup}/{item} → dosya başına bir istek
POST   /wfe {…, wfe_id}                        → kapı → başlat
```

Bu tasarımın üç açığı vardı:

1. **Telafi istemcideydi.** `422 attachment.missing` → rezervasyon korunmalı; `400` → bırakılmalı;
   `5xx` → tekrar denenmeli. Bu ayrım motorun bilgisi. Portal doğru yapsa bile portal dışı
   her istemci aynı disiplini baştan kurmak zorundaydı.
2. **Yetki kapısı yalnız `POST /wfe`deydi.** `reserve` kapısızdı → "rezerve → YÜKLE → 403".
3. **Kısmi yükleme.** N dosyanın k'sı yazılıp k+1'incisi reddedilince tutarsız küme kalıyordu.

---

## 2. Uygulanan değişiklikler (kronolojik)

### 2.1 İlk düzeltme — kapıyı öne almak, temizliği sunucuya vermek

| Değişiklik | Yer |
|---|---|
| `assert_can_start` — start kuralının `from` node'unun `c_a`'sı `Engine::start`'ın kural seçimiyle AYNI testten geçer | `routes/wfe.rs` |
| `POST /wfe/reserve` bu kapıyı çağırır → yetkisiz aktör wfe_id ALAMAZ (403) | `routes/wfe.rs` |
| `reservation::release` (önce depo `remove_all`, sonra defter satırı) | `reservation.rs` |
| `DELETE /wfe/reserve/{wfe_id}` — sahiplik kapısı yüklemeyle aynı, **idempotent 204** | `routes/wfe.rs` |
| `start_wfe` **kalıcı** reddinde otomatik `release`: **4xx bırakır, 5xx DURDURUR** | `routes/wfe.rs` |
| Süpürücü `release`'i kullanır (kod tekrarı kalktı) | `reservation.rs` |

`ReserveBody.action` eklendi — kapı aksiyon bazlı daraltılabiliyor.

**4xx/5xx ayrımının gerekçesi:** 4xx'te aynı istek aynı cevabı verir, o belgelerin bağlanacağı
bir WFE olmayacak. 5xx geçicidir; rezervasyon durur ki istemci belgeleri yeniden yüklemeden
tekrar denesin. `attachment.missing` bu koda hiç gelmez — kapı `start_reserved`dan ÖNCE döner.

### 2.2 Faz 0 — gövde limiti (gerçek bug)

Router'da hiçbir yerde `DefaultBodyLimit` yoktu → axum 0.7'nin **2 MB** varsayılanı
yürürlükteydi ve `body: Bytes` extractor'ı bunu `validate_upload`tan ÖNCE uyguluyordu.
Katalogda `max_size_mb: 20` yazan slot pratikte ~2 MB'ta 413 veriyordu.

`ATTACHMENT_MAX_REQUEST_MB` (varsayılan 200) eklendi ve layer **yalnız `/wfe` + `/portal`**
alt ağaçlarına uygulandı — diğer uçların 2 MB koruması korundu.

### 2.3 Faz 1 — tek istekte başlatma

`POST /wfe` artık `Content-Type` ile ikiye ayrılıyor (`start_wfe` dispatcher):

```http
POST /wfe
Content-Type: multipart/form-data; boundary=…

--…  name="payload"            ← İLK part olmak ZORUNDA
{"wfd_id":"…","version":3,"action":"create_application","input":{…},
 "deadline":"P1D","attachments":[{"group":"…","item":"…","sha256":"…"}]}
--…  name="basvuru_belgeleri/kimlik"; filename="kimlik.pdf"
<binary>
```

Sunucu içi sıra (`start_multipart` → `start_multipart_committed` → `write_and_start`):

| # | Adım | Hata |
|---|---|---|
| 1 | `payload` parse; ilk part değilse `400 multipart.payload_first` | bayt okunmadı |
| 2 | `assert_can_start` | `403`, bayt okunmadı |
| 3 | Dedupe claim (`start_dedupe`) | `200` replay / `409 conflict.start_in_progress` |
| 4 | `wfe_id` üret + rezervasyon satırı (crash ağı) | `500` |
| 5 | Part'ları `attachments/{wfe_id}/…`'e **stream**; katalog/boyut/sniff/sha256 | `remove_all` → `413`/`415`/`422` |
| 6 | Zorunlu slot kapısı | `remove_all` → `422 attachment.missing` |
| 7 | `start_reserved` | `remove_all` → 4xx/5xx |
| 8 | Rezervasyon satırı sil → `200` | — |

Teknik notlar:

- **Stream:** `axum::extract::Multipart::next_field()` → `field.chunk()` döngüsü →
  `AttachmentStore::writer` (opendal). Bellek = chunk; dosya sayısı/boyutu bellek profilini
  etkilemez. Aşımda `writer.abort()` — yarım nesne kalmaz (S3'te multipart upload abort edilir).
- **Tip kapısı baytlardan ÖNCE:** `check_upload(def, ct, 0)` — `check_upload` önce tipe göre
  kuralı seçer, boyutu sonra denetler; reddedilecek dosya hiç yazılmaz.
- **`slot_cap_bytes`** akış sınırı için AYRI: gösterim helper'ı `slot_max_size_mb` `round()`
  uygular ve `max_size_mb: 0.4` gibi MB altı bir kural 0'a yuvarlanırdı → o slotun her dosyası
  ilk chunk'ta reddedilirdi.
- **`POST /wfe/preflight`:** gövdesiz ön kontrol (yetki + slot kuralları + bildirilen
  boyut/tip). YAN ETKİSİZ, **kapı değil** — hata olsa bile 200, gerçek denetim `POST /wfe`
  içinde yeniden koşar. Varlık sebebi: tarayıcı `fetch` büyük gövde gönderirken sunucunun
  erken 403'ünü çoğu zaman ağ hatasına çevirir.

### 2.4 Faz 2 — dosya metadata'sı

`wf.wfe_attachment` (`migrations/wf/20260811000002_wfe_attachment.sql`):

```sql
wfe_id uuid NOT NULL REFERENCES wf.wfe(wfe_id) ON DELETE CASCADE,
grp text, item text, version integer DEFAULT 1,
storage_key text, filename text, content_type text,
size_bytes bigint, sha256 text, uploaded_by uuid, uploaded_at timestamptz,
PRIMARY KEY (wfe_id, grp, item, version)
```

- `version`: aynı slota tekrar yükleme üzerine YAZMAZ, yeni sürüm açar; okuma en yükseği alır.
- Modül: `crates/server/src/wfe_attachment.rs` → `insert_many` (tek transaction),
  `list_by_wfe`. **Elle silme fonksiyonu kasten yok** — FK CASCADE tek yol.
- `filename` `notes::decode_filename` + `sanitize_filename` ile temizlenir (kopyalanmadı).
- Okuma uçları `attachments::enrich_with_meta` ile zenginleştirilir; her iki route ağacı
  (`routes/attachments.rs::status`, `routes/portal/wfe.rs` detay ucu) aynı fonksiyonu çağırır.

### 2.5 Faz 3 — staging

`wf.upload_staging` (`migrations/wf/20260811000003_upload_staging.sql`) + `staging.rs` +
`routes/uploads.rs`:

```text
POST   /uploads {wfd_id, version, group, item, environment?} → {upload_id, url?, expires_at}
PUT    <presigned url>  (s3)  |  PUT /uploads/{upload_id}  (local, stream)
DELETE /uploads/{upload_id}   (idempotent 204)
POST   /wfe  payload.attachments[].upload_id → staging::take → server-side COPY → start
```

- Anahtar `staging/{upload_id}` — `attachments/` ve `notes/` köklerinden AYRI. Karışsaydı
  `remove_all`/`status_for_node` henüz hiçbir WFE'ye ait olmayan dosyayı "yüklenmiş" sayardı.
- `environment_id` staging satırında tutulur: depo WFD başına `$env` ile çözülüyor, staging
  nihai anahtarla AYNI bucket'ta olmalı ki taşıma server-side copy olsun.
- Presign yeteneği `Operator::info().full_capability().presign_write` ile sorulur; yoksa
  `url: None` → istemci sunucuya PUT eder.
- `move_to_final(op, src, dest)` ortak yardımcısı: `Operator::copy`, desteklenmiyorsa oku-yaz
  fallback. `take` ve `promote` bunu paylaşır.
- GC: `staging::sweep_expired` (TTL 24s) mevcut saatlik süpürücüde — bucket lifecycle DEĞİL
  (ayrı repoda, local backend'de karşılığı yok).

### 2.6 Faz 4 — akış ortasında çok dosyalı aksiyon

`POST /wfe/{id}/actions` de `Content-Type` ile ikiye ayrılıyor (`apply_action` dispatcher →
`apply_json` / `apply_multipart`).

Başlatmadan farkı: **nihai anahtar zaten DOLU olabilir.** Nihai anahtar tektir; üzerine
yazılıp aksiyon patlarsa eski baytlar geri getirilemez (`wf.wfe_attachment` SATIRI sürümlenir
ama NESNE sürümlenmez). Bu yüzden:

```text
part'lar → staging::stage_part  (nihai anahtara DOKUNULMAZ)
kapı     → missing_required_with_pending(groups, pending)   ← depodaki ∪ staging'deki
aksiyon  → executor.apply
   başarı → staging::promote (×3 deneme) + wfe_attachment::insert_many
   hata   → staging::discard; nihai anahtar HİÇ dokunulmamış
```

`discard` çağrılan dört nokta: yazma yarıda kaldı, item reddi, kapı kapandı, apply hatası.

**Kapı neden birleşim sorar (K11):** dosyalar aksiyon başarılı olana kadar nihai anahtara
taşınmıyor, kapı ise aksiyondan ÖNCE koşuyor. Birleştirilmezse kullanıcı eksiği aynı istekte
gönderse bile kapı "eksik" der ve dosya hiç yerine konmaz — çıkışsız döngü. `pending` YALNIZ
kapıyı etkiler; `AttachmentItemStatus.uploaded` deponun gerçeği olarak kalır.

### 2.7 Aynı gün ikinci tur — eski yolun HTTP ucu kaldırıldı, aksiyonsuz toplu uç eklendi

Faz 0-4 sabah tamamlandıktan sonra portal HER yerde toplu multipart'a geçmiş olduğu
görülünce bir tarama yapıldı: aşağıdaki üç ucun bu workspace'te hiçbir çağıranı kalmamıştı.

**Kaldırılan:**

| Uç | Neden |
|---|---|
| `POST /wfe/reserve` | tek kullanıcısı portal'ın eski adım-adım akışıydı; portal bulk'a geçti |
| `DELETE /wfe/reserve/{wfe_id}` | rezervasyon iptali gerekmiyor artık (istek hiç başlamıyor) |
| `PUT /wfe/{id}/attachments/{grup}/{item}` (direkt X-Actor ağacı) | tek kullanıcısı `routes/attachments.rs::validate_upload` + portal `uploadAttachment` (TS) idi, ikisi de gitti |
| `POST /wfe` body `wfe_id` alanı | rezerve edilmiş id kabul eden bir yol kalmadığından anlamsızlaştı; wfe_id'yi DAİMA engine üretir |

JWT ağacındaki tek-dosya karşılığı (`PUT /portal/wfe/{wfe_id}/attachments/{grup}/{item}`)
**DURUYOR** — o ağacın bu workspace dışında tüketicisi olabileceği için tarama onu kapsam
dışı tuttu; GET/DELETE tek-dosya uçları (indirme + tekil silme, iki ağaç) da DURUYOR.

`wf.wfe_reservation` tablosu, `reservation.rs`, saatlik süpürücü DURUYOR ama artık yalnız
**crash ağı**: `assert_can_start` doğrudan `POST /wfe` (her iki content-type) içinde koşar,
satır istek başında yazılıp başarıda silinir, istemciye hiç görünmez — tek işlevi sunucu
istek ortasında ölürse (deploy/OOM) yazılmış baytların sahibini süpürücüye bildirmek.

**Eklenen — `PUT /wfe/{id}/attachments` (çok dosyalı, AKSİYONSUZ):**

Multipart, alan adları `{grup}/{slot}`, `payload` part'ı YOK (aksiyon/girdi taşımaz).
Atomik: `staging::stage_part` ile yazılır, hepsi doğrulanınca `promote`, biri reddedilirse
`discard` — Faz 4'ün staging altyapısı yeniden kullanıldı, ortak mantık
`upload_multi_shared`. **Kapı (`gates_action`) UYGULANMAZ**: bu yükleme bir aksiyona bağlı
değil, katalog referansının "bu grup burada toplanır" sözü yeterli — hangi aksiyonun bu
grubu kapattığı sorusu yalnız `apply_action`/`submit_action`'da sorulur. JWT simetriği
`PUT /portal/wfe/{wfe_id}/attachments` aynı gün eklendi.

Detay ve K9 güncellemesi: `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md`
"Ek — aynı gün ikinci tur" bölümü + K14.

---

## 3. Depo çözümünde yazma/okuma asimetrisi

Publish kapısı (`routes::wfd::assert_attachment_storage_env`) belge toplayan bir akışın depo
ayarı olmadan yayınlanmasını engelliyordu, ama **runtime'da sessiz fallback duruyordu**:
`$env`de depo tanımsızsa deployment varsayılanına (bizim diskimize) düşülüyordu. Publish
kapısı tek savunma olamaz — kapıdan önce yayınlanmış akışlar, sonradan silinen `$env`
satırları ve anahtarları eksik yeni ortamlar arkasından geçer.

| Yol | Çözücü | Gerekçe |
|---|---|---|
| `PUT .../attachments/...` (iki ağaç) | `*_strict` | yazma |
| multipart `POST /wfe` | `store_for_wfd_strict` | yazma |
| `POST /uploads`, `PUT /uploads/{id}` | `store_for_wfd_strict` | yazma |
| `staging::take`, `staging::stage_part` | `store_for_wfd_strict` | yazma (hedef nihai anahtar) |
| `GET .../attachments`, download, status, apply gate | `store_for_wfd`/`_wfe` | **okuma** |
| `DELETE .../attachments/...`, `reservation::release`, süpürücüler | `store_for_wfd`/`_wfe` | **temizlik** |

Okumayı da katı yapmak, eski davranışla deployment deposuna yazılmış mevcut dosyaları bir
anda erişilemez ve silinemez yapardı. Katılık yeni yanlış yazımı durdurur, geçmişi
kilitlemez. `resolve_target(s, actor, wfe_id, for_write)` bu ayrımı taşır.

---

## 4. Çift işlem koruması — iki farklı çapa

### Başlatma (K6): parmak izi, istemci hiçbir şey göndermez

```text
fingerprint = sha256(actor_user_id, wfd_id, version, action,
                     canonical_json(input), canonical_json(payload.attachments))
```

`wf.wfe_start_dedupe` (PK `fingerprint`, `wfe_id NULL` = koşuyor). `claim` tek
`INSERT … ON CONFLICT DO UPDATE … WHERE created_at < now() - window` ile atomik.
Pencere `WFE_START_DEDUPE_WINDOW_SECS` (60). Tekrar → ilk `wfe_id` + `Idempotent-Replay: true`.
Hâlâ koşuyorsa `409 conflict.start_in_progress`. Hata yolunda satır silinir.
Kaçış: `X-Allow-Duplicate: true`.

**Parmak izi YALNIZ `payload`tan türer, baytlardan değil** — `payload` ilk part olduğu için
karar baytlar okunmadan verilir, tekrar istek 200 MB aktarmadan yanıtlanır.

İstemcinin ürettiği `Idempotency-Key` **reddedildi**: anahtar üretmeyen istemci sessizce
korumasız kalır ve bunu fark etmez. Seam korunuyor — başlık gelirse aynı tablo taşır.

### Aksiyon (K12): çapa `expected_rev`, "o anki rev" DEĞİL

Gerekçe ters çalışır: ilk apply başarılı olunca rev ilerler; tekrar isteği o anki rev'e
bakarsa parmak izi DEĞİŞİR ve aksiyon ikinci kez uygulanır. İstemcinin gönderdiği
`expected_rev` tekrar denemede AYNI kalır.

`expected_rev` **gönderilmemişse dedupe hiç koşmaz**. Çapasız tahmin, aynı girdiyle meşru
olarak tekrarlanan bir aksiyonu ("revizyon iste", akışın 2. ve 5. adımında) sessizce yutardı.
K6 ile çelişmez: `expected_rev` zaten var olan bir alandır (WOR-65), yeni istemci yükü değil.

Replay cevabı bu uçta `apply_replay_response` ile üretilir (`note_error`/`attachment_error`
alanlarını da taşıyan sarmal); `current_c_a` yeniden kurulamaz, `Idempotent-Replay: true` konur.

---

## 5. İçerik güvenliği

- **Magic-byte sniff** (`attachments::sniff_content_type` / `detect_mismatch`): istemcinin
  `Content-Type` beyanına güvenilmiyordu → `.exe`nin `application/pdf` diye geçmesi kapatıldı.
  Tespit: pdf, png, jpeg, gif, webp, zip, rar, 7z, gzip, **ELF / PE (`MZ`) / Mach-O**.
  Bilinmeyen imza `None` → reddetme sebebi DEĞİL (katalogdaki serbest tipler kullanılabilir kalsın).
  Zip ailesi (docx/xlsx/pptx/odt/jar/apk) aynı imzayı paylaştığından allow-list ile ayrıldı.
- `UploadReject::TypeMismatch { declared, detected }` → **415**, üç route'ta da aynı metin.
- **`Sha256Stream`**: chunk chunk özet; `payload.attachments[].sha256` bildirilirse doğrulanır
  (`checksum_mismatch`), bildirilmese de metadata'ya yazılır.

---

## 6. Hata sözleşmesi

`AppError` opsiyonel `items` alanı kazandı; `error`/`code` **geriye uyumlu**, `items` yalnız EKLENİR.

```json
{ "error": "2 belge reddedildi", "code": "attachment.rejected",
  "items": [ {"group":"…","item":"…","code":"too_large","message":"…"} ] }
```

| Kod | Statü | Anlam |
|---|---|---|
| `multipart.payload_first` | 400 | İlk part `payload` değil |
| `attachment.rejected` + `items[]` | 422 | Slot bazında ret |
| `attachment.missing` | 422 | Zorunlu belge eksik |
| `attachment.multipart_required` | 422 | Belge isteyen akış JSON gövdesiyle başlatılmaya çalışıldı (2026-08-11: eski adı `attachment.reservation_required`) |
| `attachment_storage.missing_env` | 422 | Yazma yolunda depo ayarı yok |
| `conflict.start_in_progress` | 409 | Aynı istek işleniyor |
| item: `too_large` / `unsupported_type` / `type_mismatch` / `checksum_mismatch` / `unknown_slot` / `empty` / `upload_not_found` | — | Slot bazında sebep |

---

## 7. Tasarımdan sapmalar ve kabul edilen boşluklar

**K7 "aynı transaction" TUTULAMADI.** WFE'yi yaratan transaction `wf_wfe` crate'inin içinde
açılıp kapanıyor; `server` ona katılamıyor. Crate'ler arası seam açmak yerine değişmez FK ile
korundu: `ON DELETE CASCADE` → **satır varsa WFE vardır**. Satırlar `start_reserved` BAŞARILI
olduktan sonra yazılır; `insert_many` hatası `warn`lanır ve başarı cevabı yine döner —
metadata denetim/gösterim katmanıdır.

**Kapı SQL'e taşınmadı.** `uploaded` gerçeğinin kaynağı DEPO olarak kaldı; metadata yalnız
gösterim. Kaynak yapmak, tablo eklenmeden önce yüklenmiş bütün belgeleri "yok" gösterirdi.
`status_for_node` imzası bu yüzden değişmedi — kapı yolunda DB bağımlılığı yok.

**Commit sonrası taşıma hatası (K13).** Aksiyon uygulandıktan sonra `promote` başarısız olursa
aksiyon GERİ ALINMAZ (`wfah` yazıldı, `$wfah` sayan ifadeler etkilendi). Üç önlem: 3 deneme,
cevapta `attachment_error`, metadata satırı yazılmaz → sonraki kapı dosyayı yok görüp akışı
durdurur. Hata sessiz değil.

**Kapsam dışı:** AV/ICAP + `quarantined`, tenant başına KMS, retention/WORM/legal hold,
bucket lifecycle. Üçü servis/altyapı/uyum kararı, biri ayrı repoda.

---

## 8. Dosya envanteri

**Yeni:** `start_dedupe.rs`, `wfe_attachment.rs`, `staging.rs`, `routes/uploads.rs`,
`migrations/wf/20260811000001_wfe_start_dedupe.sql`, `…000002_wfe_attachment.sql`,
`…000003_upload_staging.sql`

**Değişen (backend):** `routes/wfe.rs` (dispatcher'lar, multipart start/apply, preflight,
release, `assert_can_start`, `upload_multi_shared` + `PUT /wfe/{id}/attachments`),
`routes/attachments.rs` (**`validate_upload` SİLİNDİ** — tek kullanıcısı giden endpoint'ti),
`routes/portal/attachments.rs`, `routes/portal/wfe.rs` (+`PUT /portal/wfe/{wfe_id}/attachments`),
`attachments.rs` (sniff, `Sha256Stream`, `writer`, `enrich_with_meta`, `*_with_pending`),
`attachment_store.rs`, `reservation.rs` (rol crash-ağına daraldı; `release` çağıran HTTP ucu
kalmadı), `error.rs` (`items`), `config.rs`, `main.rs`, `routes/mod.rs` (**`POST /wfe/reserve`,
`DELETE /wfe/reserve/{id}`, direkt X-Actor `PUT /wfe/{id}/attachments/{grup}/{item}` SİLİNDİ**),
`openapi.rs`, `Cargo.toml` (`sha2`, `futures-util`, axum `multipart`)

**Değişen (portal):** `features/workflows/api.ts` (+`startWorkflowWithFiles`, `preflightStart`,
`attachmentErrorLines`; −`reserveWfe`, `releaseWfe`), `features/workflows/WorkflowsPage.tsx`,
`features/instances/api.ts` (+`applyActionWithFiles`, `actionAttachmentSlots`,
`applyAttachmentErrorLines`; **`uploadAttachment` SİLİNDİ** — tek kullanıcısı giden endpoint'ti,
yerini toplu multipart çağrıları aldı), `features/instances/InstanceDetail.tsx`,
`AttachmentPanel` (çoklu seçim + tek "Yükle" düğmesi, metadata gösterimi — ad/boyut/tarih/
kısaltılmış sha256/`version > 1` rozeti)

**Dokümantasyon:** tasarım dokümanı (K1–K13 + fazlar + sapmalar), `CLAUDE.md`,
`docs/spec/decisions.md` Madde 8

---

## 9. Doğrulama durumu

**Geçti:** `cargo test --workspace` → 24 test binary, 0 fail (45 server unit testi dahil).
`npx tsc --noEmit` → temiz. Yeni ölü kod yok.

**Yeni testler:** sniff/mismatch/`Sha256Stream` (13), `enrich_with_meta` (3),
`*_with_pending` (4), `start_dedupe` kanonik parmak izi (3).

**Migration durumu:** üç migration (`20260811000001_wfe_start_dedupe`,
`…000002_wfe_attachment`, `…000003_upload_staging`) kullanıcı tarafından BUGÜN
uygulandı — şema artık gerçek DB'de mevcuttur. **Bu, aşağıdaki DOĞRULANMADI listesini
KAPATMAZ:** şemanın var olması, yeni yolların gerçek bir istekle o şemaya karşı
çalıştığının kanıtı değildir — "şema hazır" ile "akış uçtan uca çalıştı" ayrı iki iddiadır.

**Canlı koşuldu (§12'ye bak):** sunucu gerçek DB + Garage (S3) ile ayağa kaldırıldı, 12
senaryo curl ile koşuldu — başlatma yetki kapısı, eksik belge, magic-byte, boyut, mutlu yol,
indirme, dedupe, toplu yükleme, atomik red, payload-first, JSON kapısı. Bu tur ÜÇ gerçek
hata çıkardı (biri birim testlerin göremeyeceği açılış panic'i).

**HÂLÂ DOĞRULANMADI:** presigned PUT (Garage presign yolu), dedupe YARIŞI (eşzamanlı iki
istek — sıralı replay test edildi, yarış değil), staging TTL süpürmesi (24s beklemeden),
`writer.abort()`in S3 multipart upload'ı iptali, Faz 4 multipart-aksiyon (rol eşleşmesi
kurulmadı — kod yolu birim testli ama canlı denenmedi), portal UI'ından uçtan uca akış.

---

## 10. Operasyon

**Migration sırası — BUGÜN UYGULANDI (elle, sırayla):**
1. `20260811000001_wfe_start_dedupe.sql`
2. `20260811000002_wfe_attachment.sql` ← `wf.wfe`ye FK bağlar
3. `20260811000003_upload_staging.sql`

Uygulanmadan önce tek istekli başlatma, metadata ve staging **500** verirdi; şema artık
devrede. (Bu, uçtan uca canlı bir denemenin yapıldığı anlamına GELMEZ — bkz. §9.)

**Yeni env:** `ATTACHMENT_MAX_REQUEST_MB` (200), `WFE_START_DEDUPE_WINDOW_SECS` (60)

**Davranış değişikliği 1:** `$env`de depo ayarı olmayan bir akışa belge yüklemek artık
`422 attachment_storage.missing_env`. Publish kapısı yeni akışları zaten koruyordu; etkilenecek
olanlar kapıdan önce yayınlanmış ya da ayarı sonradan bozulmuş akışlardır.

**Davranış değişikliği 2 (aynı gün, ikinci tur) — üç uç KALDIRILDI:** `POST /wfe/reserve`,
`DELETE /wfe/reserve/{wfe_id}`, direkt X-Actor `PUT /wfe/{id}/attachments/{grup}/{item}`.
Bu üçünü çağıran bir istemci varsa artık `404` alır. Tarama bu workspace'te çağıranı
kalmadığını gösterdi (portal HER yerde toplu multipart'a geçti); portal dışı bir entegrasyon
bu uçlara hâlâ bağımlıysa devreye almadan önce KONTROL EDİLMELİDİR. `POST /wfe` body'sindeki
`wfe_id` alanı da kaldırıldı (artık yok sayılmaz, `deny_unknown_fields` reddeder).

**Yeni uç (aynı gün):** `PUT /wfe/{id}/attachments` (+ JWT `PUT /portal/wfe/{wfe_id}/attachments`)
— çok dosyalı, aksiyonsuz, atomik yükleme.

**Geriye uyum:** `application/json` gövdeli `POST /wfe` ve `POST /wfe/{id}/actions` aynen
çalışır (wfe_id alanı hariç). JWT ağacındaki tek-dosya `PUT /portal/wfe/{wfe_id}/
attachments/{grup}/{item}` ve iki ağaçtaki `GET`/`DELETE` tek-dosya uçları DURUYOR; direkt
X-Actor tek-dosya `PUT` ve rezervasyon HTTP yüzeyi (K9, güncellendi) KALKTI.

---

## 11. Sıradaki iş

- Portal `startAttachmentSlots` ile katalog kurallarını hâlâ kendi yorumluyor; preflight'ın
  `slots` cevabına geçirilirse kural tek kaynakta kalır.
- Faz 3'ün kapsam dışı bırakılanları (AV, KMS, retention) ayrı kararlar bekliyor.

## 12. Canlı doğrulama (gerçek DB + Garage/S3)

Sunucu `.env` (test tenant QNB, Garage S3 bucket) ile `PORT=3011`'de ayağa kaldırıldı,
belge toplayan bir test WFD (`e2b0633d`, start node zorunlu `kimlik` pdf + opsiyonel `ek_not`
txt) yayınlanıp 12 senaryo curl ile koşuldu.

**Çıkan üç gerçek hata (birim testler görmüyordu):**

1. **Açılış panic'i — rota çakışması** (bu turda girmişti, DÜZELTİLDİ). `routes/portal/
   attachments.rs` dört handler'ı (`download, upload, remove, upload_multi`) TEK `routes!`
   makrosuna koymuş; `routes!` yolu ilk handler'dan alıp hepsini o yola bağlıyor → iki PUT
   (`upload` ve `upload_multi`) aynı yola düşüp axum'u açılışta paniğe sokuyordu. Direkt
   ağaç doğru desende (`upload_multi` ayrı `.routes()` çağrısında), portal ağacı değildi —
   iki ağaç ayrı yazıldığı için. Birim testleri yakalamaz: panic ancak `main` gerçek
   router'ı bağlarken oluşur. Düzeltme: `.routes(routes!(download, upload, remove))
   .routes(routes!(upload_multi))`.

2. **`POST /wfd` publish kapısını atlıyordu** (mevcut, DÜZELTİLDİ). Adapter (`wfd::upload`)
   doğrudan `status='published'` yazıyor ve `assert_attachment_storage_env` çağrılmıyordu;
   submit/approve/publish yolu çağırıyordu. Belge toplayan bir akış depo ayarı olmadan
   yayınlanabiliyordu. Katı yazma (2026-08-11) runtime'da ikinci savunma ama tek savunma
   olmamalı. Düzeltme: kapının `$env` denetim kısmı `assert_storage_env_for(orgtnt, project,
   ad)` yardımcısına ayrıldı; `upload_wfd` satırı YARATMADAN önce, JSON'u parse edip
   `collects_attachments` ise bunu çağırıyor. Canlı doğrulandı: depo ayarsız belge-WFD
   upload'u artık `422 attachment_storage.missing_env`.

3. **Ortam adı verilmeyen HER başlatma NULL constraint'e çarpıyordu** (mevcut, benim işimden
   önce — migration 2026-08-04, DÜZELTİLDİ). `wfe.environment_id` NOT NULL ama
   `executor::start_reserved` çağıranın `None`'ını satıra yazıyordu. `load_run_env`
   varsayılan ortamı RUNTIME için içeride çözüyor ama o id geri dönmüyordu, satıra
   yazılmıyordu. Portal `environment` göndermediği için **portal'dan hiçbir akış
   başlatılamıyordu** (JSON, multipart, JWT ağacı — hepsi). Düzeltme: `EnvPort`'a
   `resolve_environment_id(orgtnt, env_id)` metodu eklendi (`None` → tenant varsayılanını
   çözer); `start_reserved` bunu ÖNCE çağırıp çözülen id'yi hem `load_run_env`'e hem
   `new.environment_id`'ye veriyor. Tek funnel — JSON, multipart, WFC çocuğu, JWT ağacı
   hepsi düzelir. `NoEnv` (store'suz) `None`'ı aynen döner. Canlı doğrulandı: ortam
   vermeden başlatma artık 200.

**Geçen 12 senaryo:** yetkisiz başlatma (403), eksik zorunlu belge (`attachment.missing`),
magic-byte çelişkisi (`type_mismatch`, "application/x-elf bulundu"), boyut aşımı
(`too_large`), mutlu yol (WFE başladı + ilerledi), indirme (`%PDF` doğru), dedupe (aynı
`wfe_id` + `Idempotent-Replay: true`), toplu aksiyonsuz yükleme (200), atomik red (geçerli
dosya da yazılmadı, mevcut belge değişmedi — 21 bayt korundu), payload-first (`400`), JSON
ile belge-akışı başlatma (`422 attachment.multipart_required`).
