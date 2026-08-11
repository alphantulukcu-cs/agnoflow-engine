# Tek İstekte Akış Başlatma (belgelerle) — Tasarım

**Tarih:** 2026-08-11
**Durum:** Faz 0–4 UYGULANDI (2026-08-11). Faz 0–3'ün iki sapması ve kapsam dışı
bırakılanları için "Uygulamada sapmalar" bölümüne, Faz 4 kararları için K11–K13'e bak.
**Aynı gün ikinci tur** (dokümanın sonu, "Ek"): eski rezervasyon HTTP yolu kaldırıldı
(K9 güncellendi) ve aksiyonsuz çok dosyalı `PUT /wfe/{id}/attachments` eklendi (K14).
**İlgili:** `docs/spec/decisions.md` Madde 8 (attachments), `crates/server/src/reservation.rs`,
`crates/server/src/routes/attachments.rs`, `crates/server/src/routes/wfe.rs`,
`crates/server/src/attachment_store.rs`,
`docs/superpowers/specs/2026-08-04-env-config-design.md`,
`docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md`

## Problem

Belge toplayan bir akışı başlatmak bugün **2+N istek** ve istemcide **telafi mantığı** istiyor:

```text
POST   /wfe/reserve                              → wfe_id
PUT    /wfe/{wfe_id}/attachments/{grup}/{item}   → her dosya için bir istek
POST   /wfe {…, wfe_id}                          → başlat
DELETE /wfe/reserve/{wfe_id}                     → hata olursa istemci temizlemek ZORUNDA
```

Üç ayrı sorun var:

**1. Telafi yanlış katmanda.** İstemci "hangi hatada silmeliyim, hangisinde saklamalıyım"
sorusunu bilmek zorunda: `422 attachment.missing` geldiğinde rezervasyon korunmalı, `400`
geldiğinde bırakılmalı, `5xx` geldiğinde tekrar denenmeli. Bu ayrım motorun bilgisidir.
Portal bunu doğru yapsa bile portal dışı her istemci aynı disiplini baştan kurmak zorunda —
kurmayan depoda sahipsiz belge bırakır. 2026-08-11'de eklenen sunucu-tarafı `release`
(4xx'te bırak, 5xx'te durdur) kalıntı sınıfını daraltmıştır ama **yükleme aşamasındaki**
hataları kapsamaz: sunucu o hatayı hiç görmez, temizliği yine istemci istemek zorundadır.

**2. Yükleme yarıda kalabilir.** N dosyanın 3'ü yazılıp 4'ü reddedilirse (413/415/ağ) ortada
tutarsız bir küme kalır. Kullanıcıya "kısmen yüklendi" diye anlatılabilecek bir durum yok:
ya hepsi ya hiçbiri olmalı.

**3. Gizli tavan.** Router'da `DefaultBodyLimit` layer'ı **yok** → axum 0.7'nin varsayılan
**2 MB** gövde limiti yürürlükte ve `body: Bytes` extractor'ı bu sınırı `validate_upload`
çalışmadan uyguluyor. Katalogda `max_size_mb: 20` yazan slot pratikte ~2 MB'ta 413 veriyor;
kural WFD'de yazıyor, davranış başka. (`docs/spec/examples/belge-onay.json` bu boyutları
kullanıyor.)

Hedef: **UI dosyaları ve girdileri toplasın, TEK istek atsın, hiçbir telafi çağrısı
yapmasın.** Başarısızlıkta hiçbir iz kalmasın; başarıda WFE belgeleriyle birlikte doğsun.

## Alınan kararlar ve gerekçeleri

### K1 — Baytlar transactional DEĞİLDİR; commit metadata'dadır

Object storage'da transaction yoktur; S3 ile PostgreSQL arasında iki-fazlı commit kurulamaz.
"Atomik başlatma" sözünü baytlar üzerinden vermek imkânsızdır. Verilebilecek söz şudur:

> Baytlar, bir DB transaction'ı "görünür" diyene kadar **hiç kimsenin referans veremeyeceği**
> bir anahtarın altına yazılır.

Bu tanımla yarıda kalmış bayt bir *tutarsızlık* değil, *görünmez çöp*tür — ve çöp toplanabilir.
Tasarımın tamamı bu ayrımın üstünde durur: **görünürlük DB'dedir, dosya sistemi yalnız
taşıyıcıdır.** Bugünkü rezervasyon deseni de aslında budur; eksik olan, telafinin istemciden
alınması ve metadata'nın DB'de bir karşılığının olmasıdır (K7).

### K2 — Tek istek `multipart/form-data`; `payload` part'ı İLK olmak zorundadır

İstek gövdesi sırayla okunur. Yetki kararını verebilmek için gereken bilgi (wfd_id, version,
action) baytlardan **önce** gelmelidir; aksi hâlde yetkisiz bir isteğin 200 MB'ını okuyup
sonunda 403 demek gerekir — hem kaynak israfı hem DoS yüzeyi.

Bu yüzden sözleşme katıdır: ilk part `payload` (JSON) değilse `400 multipart.payload_first`.
Sıra bağımlılığı multipart'ın doğasındandır ve maliyetsizce zorlanabilir; bunu istemci
geleneğine bırakmak (bkz. `notes::decode_filename` dersi) sözleşmeyi belirsizleştirirdi.

### K3 — Dosyalar STREAM edilir; hiçbir noktada tam gövde bellekte tutulmaz

`axum::extract::Multipart` her alan için bir stream verir, opendal writer chunk chunk yazar.
Bellek kullanımı **dosya sayısından ve boyutundan bağımsızdır** (~chunk boyutu). Boyut sınırı
extractor'a değil, yazarken tutulan sayaca bakar: sınır aşılınca yazma durdurulur, kısmi nesne
silinir, `413` döner.

Bu karar "tüm dosyaları belleğe al, hepsi geçerse yaz" alternatifini eler (bkz. Reddedilen
alternatifler): o yaklaşım atomikliği bellek pahasına satın alır ve eşzamanlı 10 kullanıcıda
sunucuyu OOM'a taşır.

`DefaultBodyLimit` ayrıca ve açıkça düzeltilir (Faz 0) — streaming rota için sayaç yeterlidir
ama **mevcut** `PUT .../attachments/...` rotası hâlâ `Bytes` kullanıyor ve 2 MB tavanı orada
gerçek bir hatadır.

### K4 — Geri alma SUNUCUNUN işidir; istemci hiçbir telafi çağrısı yapmaz

İstek hangi adımda başarısız olursa olsun, cevap dönmeden önce sunucu şunu garanti eder:
**o istekte yazılmış hiçbir bayt ve hiçbir satır kalmaz.** İstemcinin görevi yalnız hatayı
kullanıcıya göstermektir.

Sonuç olarak `DELETE /wfe/reserve/{id}` yeni yolun **normal akışında hiç çağrılmaz**;
(**2026-08-11 ikinci tur:** eski yol da HTTP olarak kaldırıldığından bu uç artık HİÇBİR
yoldan çağrılmaz — bkz. dokümanın sonundaki ek bölüm.)

### K5 — Crash ağı olarak rezervasyon satırı KORUNUR (istemciye görünmeden)

K4 sunucu süreci yaşadığı sürece geçerlidir. Süreç isteğin ortasında ölürse (deploy, OOM,
kill) yazılmış baytlar geride kalır ve onları kimse bilmez — silinemeyen çöp, K1'in "toplanabilir"
şartını bozar.

Bu yüzden istek başında `wf.wfe_reservation`'a bir satır yazılır, başarıda silinir. Satır
istemciye **hiç görünmez** (wfe_id cevapta ancak başarıyla döner); tek işlevi mevcut saatlik
süpürücüye tutamak vermektir. Maliyeti tek INSERT + tek DELETE'tir.

### K6 — Çift başlatma koruması SUNUCUDADIR; istemci hiçbir şey göndermez

Tek istek büyüdükçe süresi uzar, süre uzadıkça timeout/bağlantı kopması olasılığı artar.
En kötü senaryo: WFE commit oldu, cevap istemciye ulaşamadı, kullanıcı "Başlat"a tekrar
bastı → **ikinci bir kredi başvurusu**. Çok-istekli akışta bu risk küçüktü (start gövdesi
minikti); tek istekli akışta baş risktir.

Standart çözüm istemcinin `Idempotency-Key` üretmesidir (Stripe/PayPal/AWS `ClientToken`).
**Alınmadı.** Gerekçe K4'ün devamıdır: bu tasarımın sözü "UI dosyaları ve girdileri toplar,
tek istek atar, başka hiçbir şey bilmez". Anahtar üretmek tek satırlık bir header olsa da
yine istemci disiplinidir — üretmeyen her istemci korumasız kalır ve bunu fark etmez.

Bunun yerine anahtar **isteğin kendisinden türetilir**:

```text
fingerprint = sha256(actor_user_id, wfd_id, version, action,
                     canonical_json(input),
                     canonical_json(payload.attachments))   // varsa bildirilen sha256'lar
```

Aynı parmak izi `DEDUPE_WINDOW` (60 sn) içinde tekrar gelirse iş tekrar koşmaz, ilk `wfe_id`
`Idempotent-Replay: true` başlığıyla döner.

**Parmak izi YALNIZ `payload`tan türetilir, dosya baytlarından değil.** İki gerekçe: (1) baytların
özeti ancak dosya okunduktan sonra bilinir — dedupe o zaman yapılsaydı tekrar isteğin 200 MB'ı
boşuna aktarılırdı; `payload` ilk part olduğu için (K2) karar baytlardan önce verilir ve replay
bedavaya gelir. (2) Aynı girdiyle farklı dosya göndermek gerçek bir senaryo değildir; olduğunda
istemci `payload.attachments[].sha256` bildirir ve parmak izi ayrışır.

Sınır ve kaçış kapısı: kullanıcı 60 sn içinde **bilerek** birebir aynı ikinci akışı başlatmak
isterse sessizce ilkine yönlendirilir. `X-Allow-Duplicate: true` gönderen istemci dedupe'u
atlar. Varsayılan koruma tarafındadır — çünkü 60 sn içinde birebir aynı payload'ın kasıtlı
olması istisna, kaza olması kuraldır.

Exactly-once ağ üzerinden **imkânsızdır**: "cevabı kaybolan isteğin tekrarı" ile "gerçekten
ikinci kez başlatma isteği" telde birebir aynıdır, bu ayrımı yalnız istemci bilir. Parmak izi
bu ayrımı yapmaz, pratikteki tüm kaza sınıfını (çift tıklama, ağ retry'ı, proxy retry'ı,
timeout sonrası tekrar deneme) kapatır. Kesin ayrım gerekiyorsa seam hazırdır: `Idempotency-Key`
gelirse parmak izi yerine o kullanılır (bkz. Reddedilen alternatifler — bugün uygulanmıyor).

### K7 — `wf.wfe_attachment` metadata tablosu (gerçek atomikliğin oturduğu yer)

Bugün bir dosyanın DB'de **hiçbir kaydı yoktur**; tek gerçeklik storage'daki nesnedir. Bunun
üç sonucu var: (1) kapı kontrolü her seferinde N adet `exists()` çağrısıdır, (2) "kim ne zaman
yükledi, hangi ad, hangi boyut" sorusunun cevabı yoktur, (3) K1'in "görünürlük DB'dedir"
sözü teknik olarak boştur — görünürlük hâlâ storage'a bakar.

Tablo bunları birden çözer: dosya satırları **WFE'yi yaratan transaction'ın içinde** yazılır.
Commit olmadıysa dosya *yoktur*, bayt nerede olursa olsun. Kapı kontrolü tek SQL olur, audit
bedava gelir.

### K8 — Upload handle (`upload_id`) sözleşmesi ŞİMDİ tanımlanır, sonra uygulanır

Tarayıcı sunucuya "dosya yolu" veremez (`C:\fakepath`); bu fikrin kurumsal karşılığı, baytların
**önceden** bir staging alanına konup isteğe yalnız tutamağının (handle) girmesidir. 500 MB'lık
bir raporu engine üzerinden geçirmek yanlıştır: bellek değil ama bant genişliği, timeout ve
retry maliyeti engine'e biner.

`payload.attachments[]` alanı Faz 1'de şemaya girer ve `{group, item, upload_id}` biçimini
kabul eder; Faz 3'te gerçek karşılığı (presigned PUT + server-side COPY) uygulanır. Alanı
sonradan eklemek istemci sözleşmesini kırardı; şimdi tanımlamak bedavadır.

Aynı endpoint iki biçimi **birlikte** kabul eder: küçük dosyalar inline part, büyükler handle.

### K9 — Eski yolun HTTP ucu KALDIRILDI (2026-08-11 ikinci tur; başta "KALDIRILMAZ" kararıyla çelişir)

**Orijinal karar (bu tasarımın ilk turu, sabah):** eski yol (rezerve → yükle → başlat)
kaldırılmaz. Üç gerekçe: (1) portal dışı istemciler ve mevcut entegrasyonlar, (2) parça
parça / devam eden yükleme senaryosu — kullanıcı bugün üç belgeden ikisini yükleyip yarın
dönebilir, (3) `422 attachment.missing` sonrası "eksiği aynı id'ye yükleyip devam et"
akışı. Tek istek bu üçünü karşılamaz sanılmıştı; **tamamlayıcı** olarak eklendi, ikamesi
değil.

**Güncelleme (aynı gün, ikinci tur):** portal HER yerde toplu multipart'a geçtikten sonra
yapılan bir tarama, bu workspace'te yukarıdaki üç senaryonun (eski istemci, parça parça
yükleme, `attachment.missing` sonrası devam) fiilen hiçbir çağıranı olmadığını gösterdi —
`POST /wfe/reserve`, `DELETE /wfe/reserve/{id}` ve direkt X-Actor
`PUT /wfe/{id}/attachments/{grup}/{item}` üçü de tüketicisizdi. **Üç gerekçe YANLIŞ
değildi — bu workspace'te öngörülen tüketici hiç oluşmadı.** Sonuç: bu üç HTTP ucu
KALDIRILDI, `POST /wfe` gövdesindeki `wfe_id` alanı da kaldırıldı (wfe_id'yi artık DAİMA
engine üretir, rezerve edilmiş id dışarıdan alınamaz). Gerekçe kendisi geçersiz değil;
portal dışı bir istemci bu ihtiyaçla gelirse aynı üç gerekçe yeniden değerlendirilir, körü
körüne geri getirilmez.

**Ne kaldı, ne gitti:**
- Kalan: `application/json` gövdeli `POST /wfe` ve `POST /wfe/{id}/actions` (K9'un asıl
  çekirdek kararı — iki yol aynı commit mantığını (K7) paylaşır, ayrıldıkları yer yalnız
  baytların ne zaman geldiğidir). `wf.wfe_reservation` tablosu + `reservation.rs` +
  saatlik süpürücü **crash ağı olarak KALDI** (K5) — istek ortasında sunucu ölürse
  yazılmış baytların sahibini süpürücüye bildirmek için; satır istemciye hiç görünmez.
- Giden: rezervasyonun HTTP yüzeyi (`POST /wfe/reserve`, `DELETE /wfe/reserve/{id}`) ve
  direkt X-Actor ağacındaki tek-dosya `PUT .../attachments/{grup}/{item}`. Bu üçü artık
  hiçbir istemciden çağrılamaz. JWT ağacındaki tek-dosya `PUT /portal/wfe/{wfe_id}/
  attachments/{grup}/{item}` DURUYOR — o ağacın bu workspace dışında tüketicisi olabilir,
  tarama onu kapsamadı.

Yerine gelen: aksiyonlu çok dosyalı yükleme zaten Faz 4'te vardı; aksiyonsuz çok dosyalı
yükleme için K14'e (dokümanın sonu) bak.

### K10 — İçerik tipi SNIFF edilir, bütünlük sha256 ile doğrulanır

Bugün `validate_upload` istemcinin gönderdiği `Content-Type` başlığına güvenir. `application/pdf`
diyen bir `.exe` katalog kapısından geçer. Yeni yolda ilk chunk'ın magic byte'ları ile
`formats[].accept` karşılaştırılır (`infer` benzeri bir tablo yeter).

`payload.attachments[].sha256` verilirse yazarken hesaplanan özetle karşılaştırılır; tutmuyorsa
`422` — yarım/bozuk yüklenmiş dosya sessizce kabul edilmez. Verilmezse yine hesaplanır ve
metadata'ya yazılır (audit + Faz 3'te dedup için).

### K11 — Kapı depo ∪ staging BİRLEŞİMİNE bakar (Faz 4)

Faz 4'te dosyalar aksiyon başarılı olana kadar nihai anahtara taşınmaz (staging'de kalır),
kapı ise aksiyondan ÖNCE koşar. Birleştirilmezse kullanıcı eksik belgeyi AYNI istekte gönderse
bile kapı "eksik" der, aksiyon reddedilir, dosya hiç yerine konmaz — çıkışsız döngü. Çözüm:
kapı depodaki ∪ staging'deki kümeye bakar (`attachments::missing_required_with_pending` /
`satisfied_with_pending`); `pending` YALNIZ kapıyı gevşetir, `AttachmentItemStatus.uploaded`
alanını DEĞİŞTİRMEZ — o alan hâlâ deponun gerçeğidir (K7/Faz 2'nin "uploaded'ın kaynağı DEPO"
kararıyla aynı çizgide, bkz. Uygulamada sapmalar).

### K12 — Dedupe çapası (Faz 4) `expected_rev`tir, "o anki rev" DEĞİL

Gerekçe ters çalışır: ilk apply başarılı olunca rev ilerler; tekrar isteği o anki rev'e
bakarsa parmak izi DEĞİŞİR ve aksiyon ikinci kez uygulanır — K6'nın kaçınmaya çalıştığı şeyin
aynısı. İstemcinin gönderdiği `expected_rev` tekrar denemede AYNI kalır, iz tutar.
**Gönderilmemişse dedupe HİÇ koşmaz:** çapasız tahmin, aynı girdiyle meşru şekilde tekrarlanan
bir aksiyonu (örn. "revizyon iste" akışın 2. ve 5. adımında) sessizce yutardı. K6'nın "istemci
hiçbir şey göndermesin" ilkesiyle ÇELİŞMEZ: `expected_rev` zaten var olan bir alandır (WOR-65,
eşzamanlılık için), yeni bir istemci yükü değil. Replay cevabı bu uçta `{result, note_error}`
sarmalıyla döner (`apply_replay_response`) — düz `WfeStartResult` döndürmek istemcinin
ayrıştırmasını kırardı; `Idempotent-Replay: true` başlığı konur, `current_c_a` yeniden
kurulamaz.

### K13 — Kabul edilen tek boşluk (Faz 4): commit SONRASI taşıma hatası GERİ ALINMAZ

Aksiyon commit edildikten sonra dosya taşıma başarısız olursa aksiyon GERİ ALINMAZ (motorun
defteri yazıldı, geçiş gerçekleşti). Taşıma birkaç kez denenir; yine olmazsa cevaba
`attachment_error` alanı eklenir (`note_error`'ın kardeşi) ve metadata satırı yazılmaz — hata
SESSİZ değildir, sonraki kapı kontrolü dosyayı yok görüp akışı durdurur. Gerekçe not
yayınlamadaki (K5) gerekçenin aynısı: başarıyla alınmış bir aksiyonu bir kopyalama hatası
yüzünden geri almak daha büyük zarardır.

## Veri modeli

### `wf.wfe_attachment` (Faz 2)

```sql
CREATE TABLE wf.wfe_attachment (
    wfe_id        uuid        NOT NULL,
    grp           text        NOT NULL,   -- katalog grup key'i
    item          text        NOT NULL,   -- slot id
    version       integer     NOT NULL DEFAULT 1,
    storage_key   text        NOT NULL,   -- attachments/{wfe_id}/{grp}/{item}
    filename      text,                   -- kullanıcının verdiği ad (sanitize edilmiş)
    content_type  text        NOT NULL,
    size_bytes    bigint      NOT NULL,
    sha256        text        NOT NULL,
    uploaded_by   uuid        NOT NULL,
    uploaded_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (wfe_id, grp, item, version)
);
CREATE INDEX wfe_attachment_wfe_idx ON wf.wfe_attachment(wfe_id);
```

- `version`: aynı slota tekrar yükleme üzerine yazmaz, yeni sürüm açar — kapı daima en yüksek
  `version`'a bakar. Denetimde "hangi belge hangi karar anında oradaydı" cevaplanabilir olur.
- Satırlar WFE'yi yaratan transaction'da yazılır (K7). Rezervasyon yolunda (K9) dosya yüklenirken
  yazılamaz — o aşamada WFE yoktur; orada satırlar start commit'inde toplu yazılır, kaynak
  storage listelemesidir.
- `filename` sunucuda çözülür ve sanitize edilir; `notes::decode_filename` / `sanitize_filename`
  ile aynı kural (yüzde-kodlu ad, yol ayracı/`..`/kontrol karakteri temizliği).

### `wf.wfe_start_dedupe` (Faz 1)

```sql
CREATE TABLE wf.wfe_start_dedupe (
    fingerprint   text        PRIMARY KEY,  -- K6: istekten türetilir, istemciden GELMEZ
    actor_user_id uuid        NOT NULL,
    wfe_id        uuid,                     -- NULL = iş hâlâ koşuyor (satır anahtarı sahiplenir)
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wfe_start_dedupe_created_idx ON wf.wfe_start_dedupe(created_at);
```

Akış:

1. `INSERT ... ON CONFLICT DO NOTHING` ile satır sahiplenilir. Çakışma varsa mevcut satıra bakılır:
   `wfe_id` doluysa **sonuç dönülür** (iş tekrar koşmaz), boşsa aynı istek hâlâ işleniyordur →
   `409 conflict.start_in_progress`.
2. Başarıda `wfe_id` yazılır.
3. Başarısızlıkta satır **silinir** — hata düzeltilip aynı payload'la tekrar denenebilsin. Silme
   K4 rollback'inin parçasıdır.

`created_at` yaşı `DEDUPE_WINDOW`'u (60 sn) geçmiş satır yok sayılır ve üzerine yazılır; fiziksel
temizlik mevcut saatlik süpürücüde (rezervasyon + taslak not ile aynı tur), 1 saatten eski satırlar.

## API yüzeyi

### `POST /wfe` — `multipart/form-data` (yeni)

Aynı rota, ikinci bir content-type. `application/json` gövdeler **aynen çalışır** (K9).

```http
POST /wfe
Content-Type: multipart/form-data; boundary=…
X-Actor-Orgu: …  X-Actor-User: …  X-Actor-Role: branchClerk

--…
Content-Disposition: form-data; name="payload"
Content-Type: application/json

{ "wfd_id": "…", "version": 3, "action": "create_application",
  "input": { "applicant": {…}, "credit_info": {…} },
  "deadline": "P1D",
  "environment": "prod",
  "attachments": [                                  // opsiyonel — yalnız handle/sha bildirimi
    { "group": "basvuru_belgeleri", "item": "kimlik", "sha256": "…" },
    { "group": "onay_belgeleri",    "item": "kredi_raporu", "upload_id": "u_7f3…" }
  ] }
--…
Content-Disposition: form-data; name="basvuru_belgeleri/kimlik"; filename="kimlik.pdf"
Content-Type: application/pdf

<binary>
--…--
```

Part adı **`{grup}/{slot}`**'tur (katalog anahtarı). `filename` yalnız metadata'dır, anahtar
üretmez — dosya adının storage anahtarına karışması yol enjeksiyonu yüzeyidir.

### Sunucu içi sıra

| # | Adım | Başarısızlıkta |
|---|---|---|
| 1 | `payload` part'ı parse edilir (ilk part değilse `400`) | bayt okunmadı |
| 2 | Aktör + **start yetkisi** (`assert_can_start`) | `403`, bayt okunmadı |
| 3 | Dedupe: `payload` parmak izi hesaplanır, satır sahiplenilir (K6) | `200` replay ya da `409 conflict.start_in_progress` — **bayt okunmadan** |
| 4 | `wfe_id` üretilir + rezervasyon satırı (K5) | `500` |
| 5 | Her file part'ı `attachments/{wfe_id}/{grup}/{item}`'a **stream** edilir; katalog + boyut + magic-byte + sha256 denetimi yazarken koşar | `remove_all` → `413`/`415`/`422` |
| 6 | Zorunlu slot kapısı (`missing_required`) | `remove_all` → `422 attachment.missing` |
| 7 | Engine `start_reserved` + `wf.wfe_attachment` satırları — **tek transaction** | `remove_all` → 4xx/5xx |
| 8 | Rezervasyon satırı silinir → `200` | — |

5–7 arasındaki her çıkış yolunda `remove_all(wfe_id)` çağrılır (K4). 5xx'te de silinir:
tek istekli sözleşmede "dosyalar duruyor, tekrar dene" diye bir ara durum YOKTUR — istemcinin
elinde dosyalar zaten var, isteği aynen tekrarlar.

### Hata gövdesi — per-item

Mevcut `{"error": …, "code"?: …}` şekli korunur, çok-dosyalı reddi anlatmak için `items` eklenir:

```json
{ "error": "2 belge reddedildi",
  "code": "attachment.rejected",
  "items": [
    { "group": "basvuru_belgeleri", "item": "gelir_belgesi",
      "code": "too_large", "message": "dosya 5 MB sınırını aşıyor" },
    { "group": "basvuru_belgeleri", "item": "kimlik",
      "code": "unsupported_type", "message": "application/pdf bekleniyordu, image/gif bulundu" }
  ] }
```

Kodlar: `too_large`, `unsupported_type`, `checksum_mismatch`, `unknown_slot`, `empty`,
`upload_not_found` (Faz 3).

### `POST /wfe/preflight` — gövdesiz ön kontrol (Faz 1)

Baytlar yola çıkmadan önce **yetki + slot kuralları** sorulur. Gövde `payload` ile aynı JSON'dır,
dosya part'ı yoktur; hiçbir yan etkisi yoktur (satır yazmaz, dedupe satırı sahiplenmez).

```http
POST /wfe/preflight
Content-Type: application/json

{ "wfd_id": "…", "version": 3, "action": "create_application",
  "input": {…},
  "attachments": [ {"group":"basvuru_belgeleri","item":"kimlik",
                    "size_bytes": 4200000, "content_type":"application/pdf"} ] }
```

```json
{ "ok": true,
  "max_request_bytes": 209715200,
  "slots": [ { "group":"basvuru_belgeleri", "item":"kimlik",
               "required": true, "accept":["application/pdf","image/*"], "max_size_mb": 5 } ] }
```

Üç işi birden görür:

1. **Erken 403.** Yetkisiz aktör 200 MB göndermeden öğrenir. K2 sunucu tarafında bu korumayı
   zaten veriyor, ama tarayıcı erken cevabı çoğu zaman ağ hatasına çeviriyor (bkz. Bilinen
   riskler) — preflight aynı bilgiyi **temiz bir cevapla** verir.
2. **Erken boyut/tip reddi.** `attachments[]`'ta bildirilen `size_bytes`/`content_type` katalogla
   karşılaştırılır; uyumsuzsa yükleme hiç başlamaz.
3. **Slot keşfi.** İstemci `accept`/`max_size_mb` değerlerini dosya seçicisini kurmak için buradan
   alır — bugün portal bunu WFD dokümanını kendisi ayrıştırarak yapıyor (`startAttachmentSlots`),
   yani katalog kuralları iki yerde yorumlanıyor. Preflight tek kaynak hâline gelir.

**Kapı değildir, kolaylıktır.** Preflight `ok: true` dese bile gerçek denetim `POST /wfe` içinde
yeniden koşar (durum arada değişmiş olabilir; istemci preflight'ı atlayabilir). Sunucu asla
preflight sonucuna güvenmez.

### `POST /uploads` — staging (Faz 3)

```text
POST /uploads {wfd_id, version, group, item}  → {upload_id, url?, expires_at}
PUT  <url>                                     → S3'e doğrudan (local'de sunucuya)
POST /wfe  payload.attachments[].upload_id     → HEAD doğrula → server-side COPY → start
```

Staging prefix'i bucket lifecycle kuralıyla süresi dolunca silinir — ayrı süpürücü kodu yok.

## Limitler ve koruma

| Konu | Karar |
|---|---|
| Dosya başı boyut | Katalog `formats[].max_size_mb`; yazarken sayaçla |
| İstek başı toplam | Yeni env: `ATTACHMENT_MAX_REQUEST_MB` (varsayılan 200) |
| Part sayısı | Katalogdaki slot sayısı + küçük pay; fazlası `400` |
| `DefaultBodyLimit` | Eski `Bytes` rotaları için katalog tavanına çekilir (Faz 0) |
| İstek timeout'u | Gövde boyutuyla ölçeklenir; sabit timeout büyük yüklemeyi keser |
| Yavaş istemci | Minimum aktarım hızı / idle timeout (slowloris) |
| Virüs | Faz 3: staging'de ICAP/ClamAV kancası, `quarantined` durumu |
| Dedupe penceresi | `WFE_START_DEDUPE_WINDOW_SECS` (varsayılan 60) |

## Değişmezler (bu tasarımın çapası)

- **Engine core hâlâ dosya I/O YAPMAZ.** Tüm iş `server` crate'inde, edge katmanındadır
  (Madde 8'in değişmezi).
- **`payload` ilk part.** Yetki baytlardan önce sorulur.
- **İstemci telafi çağrısı yapmaz.** `DELETE /wfe/reserve/{id}` normal akışta hiç yoktu;
  2026-08-11 ikinci turda HTTP ucu olarak da tamamen kaldırıldı (bkz. K9, güncellendi).
- **Depo WFD başına `$env` ile çözülür** (2026-08-07); yeni yol da `store_for_wfd` kullanır,
  deployment varsayılanına sessiz düşüş yoktur.
- **JSON gövdeli `POST /wfe` bozulmaz** (aynen çalışır — `wfe_id` alanı hariç, o da aynı
  gün kaldırıldı). Rezervasyon tabanlı ESKİ HTTP yolu (reserve/delete + direkt X-Actor
  tek-dosya PUT) ise aynı gün ikinci turda kaldırıldı — "eski yol KALDIRILMAZ" bu değişmez
  artık yalnız çekirdek JSON yol için geçerli, HTTP rezervasyon yüzeyi için değil (K9).
- **Görünürlük DB'dedir.** Referansı olmayan bayt çöptür ve toplanır.
- **Çift başlatma koruması istemciden bağımsızdır.** Hiçbir header göndermeyen istemci de korunur.
- **Preflight kapı değildir.** Her denetim `POST /wfe` içinde yeniden koşar.

## Fazlar

### Faz 0 — Tavanı düzelt (bağımsız, hemen)
`DefaultBodyLimit` layer'ı + katalog tavanıyla hizalama. Mevcut `PUT .../attachments/...`
rotasının 2 MB tavanı kalkar. Tek başına sevk edilebilir.

### Faz 1 — Tek istek
Multipart `POST /wfe`, stream'li yazma, K4 rollback, K5 crash ağı, K6 parmak izi dedupe
(`wf.wfe_start_dedupe`), K10 sniff + sha256, per-item hata gövdesi, **`POST /wfe/preflight`**.
`payload.attachments[]` şeması (handle alanı tanımlı ama `upload_id` verilirse `501`).
Portal bu yola geçer; `reserveWfe`/`releaseWfe`/yükleme döngüsü portal kodundan düşer,
slot kuralları `startAttachmentSlots` yerine preflight'tan gelir.

### Faz 2 — Metadata ✅
`wf.wfe_attachment` tablosu (`migrations/wf/20260811000002_wfe_attachment.sql`),
`crates/server/src/wfe_attachment.rs`, multipart yolunda satır yazımı, iki route ağacının
okuma uçlarının `enrich_with_meta` ile zenginleştirilmesi.

### Faz 3 — Ölçek ✅ (kısmi, bkz. sapmalar)
`wf.upload_staging` (`migrations/wf/20260811000003_upload_staging.sql`),
`crates/server/src/staging.rs`, `routes/uploads.rs` (`POST/PUT/DELETE /uploads`),
presigned PUT (s3) / sunucuya stream'li PUT (local), server-side COPY, staging süpürücüsü.
`payload.attachments[].upload_id` başlatmada çözülür.

### Faz 4 — Akış ortasında çok dosyalı aksiyon ✅

`POST /wfe/{id}/actions` artık `multipart/form-data` da kabul eder: `payload` part'ı
`ApplyBody` JSON'u (action, input, node, expected_rev, note_id), kalan part'lar `{grup}/{slot}`
adıyla dosyalar; `application/json` gövdeli eski yol AYNEN çalışır. Faz 3'te kurulan staging
altyapısı yeniden kullanıldı (`staging::stage_part`/`promote`/`discard`, `take` ile ortak
taşıma mantığı tek yerde): dosyalar önce staging'e yazılır (nihai anahtara DOKUNULMAZ), kapı
depodaki ∪ staging'deki birleşimine bakar (K11), aksiyon uygulanır — başarıda staging nihai
anahtara taşınır (server-side copy) + `wf.wfe_attachment` satırı yazılır, hatada staging
silinir ve nihai anahtar HİÇ dokunulmamış kalır. Dedupe çapası Faz 1'in istek-fingerprint'i
değil `expected_rev`tir (K12). Kabul edilen tek boşluk K13'tedir.

## Uygulamada sapmalar

**K7 "aynı transaction" TUTULAMADI.** Tasarım dosya satırlarının WFE'yi yaratan
transaction'ın içinde yazılacağını söylüyordu; o transaction `wf_wfe` crate'inin içinde
açılıp kapanıyor ve `server` ona katılamıyor. Crate'ler arası bir seam açmak yerine
değişmez FK ile korundu: `wfe_id REFERENCES wf.wfe ON DELETE CASCADE` → **satır varsa WFE
vardır**, WFE silinince satırlar da gider. Satırlar `start_reserved` BAŞARILI olduktan
sonra yazılır; yazım başarısız olursa `warn` loglanır ve başarı cevabı yine döner —
metadata denetim/gösterim katmanıdır, onun yazılamaması yüzünden başarıyla başlamış bir
akışı iptal etmek daha büyük zarardır.

**Kapı kontrolü SQL'e TAŞINMADI.** `uploaded` gerçeğinin kaynağı DEPO olarak kaldı
(`status_for_node` → `exists`), metadata yalnız GÖSTERİM için eklenir
(`attachments::enrich_with_meta`). Metadata'yı kaynak yapmak, tablo eklenmeden ÖNCE
yüklenmiş bütün belgeleri "yok" gösterirdi. `status_for_node`un imzası da bu yüzden
değişmedi — kapı yolunda DB bağımlılığı yok.

**Staging GC bucket lifecycle DEĞİL, kendi süpürücümüz.** Tasarım S3 lifecycle kuralına
yaslanıyordu; o kural ayrı bir repoda (`agnoflow-infra`) yaşıyor ve local backend'de
karşılığı yok. `staging::sweep_expired` mevcut saatlik süpürücüye eklendi (TTL 24 saat) —
iki backend'de de çalışır, tek yerde durur.

**Yapılmadı** (bu repoda karşılığı yok, ayrı iş): AV/ICAP taraması ve `quarantined`
durumu, tenant başına KMS anahtarı, retention/WORM/legal hold, `Content-Type` sniff
tablosunun genişletilmesi.

## Sıra ve bağımlılık

Faz 0 bağımsızdır. Faz 1 Faz 0'ı ister (limit hizalaması). Faz 2 Faz 1'den bağımsız
uygulanabilir ama birlikte sevk edilirse tek migration turu olur. Faz 3 Faz 2'yi ister
(`upload_id` → metadata eşlemesi).

**Sevk notu:** üç migration ELLE uygulanır ve uygulanmadan ilgili yollar 500 verir —
`20260811000001_wfe_start_dedupe`, `20260811000002_wfe_attachment`,
`20260811000003_upload_staging` (bu sırayla; ikincisi `wf.wfe`ye FK bağlar).

## Reddedilen alternatifler

**JSON array + base64 içerik** (`[{group, item, content_b64}]`). Payload %33 şişer, hem
istemci hem sunucu tüm dosyaları belleğe alır, stream edilemez; katalogdaki 20 MB'lık slot
27 MB'lık JSON'a döner. Tek avantajı istemcide yazım kolaylığıydı; multipart aynı kolaylığı
`FormData` ile zaten veriyor.

**Tüm part'ları belleğe alıp hepsi geçerse yazmak.** Atomikliği ucuza alır gibi görünür;
eşzamanlı birkaç kullanıcıda sunucuyu OOM'a taşır. Atomiklik bellekten değil, görünürlük
kuralından (K1) gelmelidir.

**Baytları PostgreSQL'e (`bytea` / large object).** Gerçek transaction verirdi. Ama WAL'i
şişirir, yedek/replikasyon maliyetini dosya boyutuyla çarpar, tenant başına S3/KMS
yönlendirmesini (`$env` depo sözleşmesi) imkânsız kılar. Reddedildi.

**Faz 1'de ayrı `staging/` prefix'i + COPY.** Gereksiz: `attachments/{yeni_uuid}/` prefix'i
tahmin edilemez ve WFE satırı doğana kadar hiçbir uçtan referans edilemez — zaten görünmezdir.
Ekstra kopyalama maliyeti karşılığında hiçbir şey kazandırmaz. Staging Faz 3'te, baytlar
istekten ÖNCE geldiği için anlam kazanır.

**Rezervasyonu tamamen kaldırmak.** K9'daki üç senaryo (eski istemciler, parça parça yükleme,
`attachment.missing` sonrası devam) karşılıksız kalırdı.

**Saga / iki-fazlı commit.** Storage tarafı hazırlık-onay protokolü konuşmuyor; kurulacak
her şey sonunda "yaz, olmazsa sil"e indirgeniyor. Karmaşıklık karşılığı sıfır kazanç.

**İstemcinin ürettiği `Idempotency-Key`** (Stripe/PayPal/AWS deseni). Kesin niyet ayrımı
verirdi — "cevabı kaybolan isteğin tekrarı" ile "bilerek ikinci başvuru" ancak böyle ayrılır.
Reddedildi çünkü bu tasarımın sözü UI'dan **hiçbir şey istememektir** (K4/K6): anahtar üretmeyen
istemci sessizce korumasız kalır ve bunu fark etmez. Parmak izi kaza sınıfının tamamını
istemciden bağımsız kapatıyor. Seam korunur: `Idempotency-Key` başlığı gelirse parmak izi
yerine o kullanılabilir, `wf.wfe_start_dedupe` şeması değişmeden taşır.

**WFD'de doğal anahtar** (`start[].idempotency: "$ctx.applicant.tckn + '/' + $ctx.basvuru_no"`
→ `wf.wfe` üzerinde unique index). En sağlam garanti, pencere/süre yok. Reddedildi çünkü her
akışta doğal anahtar bulunmaz ve yük akış tasarımcısına geçer; ayrıca bu bir *iş kuralıdır*
(aynı başvuru iki kez açılmasın), tekrar-teslimat korumasıyla aynı problem değildir. Gerçek
talep gelirse ayrı bir tasarım konusudur.

## Bilinen riskler

- **Erken cevap ve istemci davranışı.** Sunucu 2. adımda `403` dönse bile tarayıcı `fetch`
  gövdeyi göndermeye devam edip cevabı ancak sonda okuyabilir (veya `ERR_CONNECTION_RESET`
  görebilir). Sunucu tarafı doğru davranır (bayt okumaz, bağlantıyı reset eder) ama istemcide
  hata mesajı ağ hatasına dönüşebilir. **Azaltma: `POST /wfe/preflight` (Faz 1).** Yine de
  garanti değildir — preflight'ı atlayan istemci ham davranışı görür.
- **Tek istek = tek yumurta sepeti.** 200 MB'ın 190'ında kopan bağlantı her şeyi tekrarlatır.
  Faz 3 (handle) bu riski dosya bazına indirir; Faz 1'de kabul edilen bir sınırlamadır.
- **Dedupe penceresi kasıtlı tekrarı yutar.** 60 sn içinde birebir aynı payload'la ikinci akış
  başlatmak isteyen istemci ilkine yönlendirilir; kaçış `X-Allow-Duplicate: true`. Pencere env
  ile ayarlanabilir (`WFE_START_DEDUPE_WINDOW_SECS`).
- **Parmak izi dosyaları görmez.** Aynı girdi + farklı dosyalarla 60 sn içinde gelen ikinci
  istek tekrar sayılır. İstemci `payload.attachments[].sha256` bildirerek ayrıştırabilir;
  bildirmeyen istemci için kabul edilen bir sınırlamadır.

## Ek — aynı gün ikinci tur (2026-08-11): eski yolun kaldırılması + K14

Faz 0-4 sabah uygulandıktan sonra, portal HER yerde toplu multipart'a geçmiş olduğu
görülünce bir tarama yapıldı: bu workspace'te `POST /wfe/reserve`, `DELETE
/wfe/reserve/{id}` ve direkt X-Actor `PUT /wfe/{id}/attachments/{grup}/{item}` uçlarının
hiçbir çağıranı kalmamıştı. K9 bu bulguyla güncellendi (yukarı bak) — üçü KALDIRILDI,
`POST /wfe` gövdesindeki `wfe_id` alanı da kaldırıldı. JSON gövdeli `POST /wfe`/`POST
/wfe/{id}/actions`, `wf.wfe_reservation` + `reservation.rs` + saatlik süpürücü (artık
yalnız crash ağı olarak) ve JWT ağacındaki tek-dosya `PUT /portal/wfe/{wfe_id}/
attachments/{grup}/{item}` (o ağacın bu workspace dışında tüketicisi olabileceğinden)
DOKUNULMADAN kaldı.

### K14 — Aksiyonsuz çok dosyalı yükleme: `PUT /wfe/{id}/attachments`

Faz 4 akış-ortası **aksiyonlu** yüklemeyi çözdü (K11-K13), ama "aksiyon almadan belge
ekle" senaryosu (örn. sonradan gelen bir ek evrak) hâlâ tek-tek `PUT .../:group/:item`
gerektiriyordu — K1-K4'ün çözdüğü atomiklik sorunu bu yolda hâlâ vardı: N dosyanın biri
reddedilirse öncekiler depoda sahipsiz kalırdı.

**Karar:** aynı desen, aksiyon katmanı olmadan. Multipart, alan adları `{grup}/{slot}`,
`payload` part'ı YOK — bu yüklemenin taşıyacağı bir aksiyon/girdi yok, JSON gövdeye
gerek kalmıyor. **Atomik**: dosyalar önce staging'e yazılır, hepsi doğrulanınca hepsi
nihai anahtara promote edilir; biri reddedilirse hiçbiri yazılmaz. Ortak mantık
`upload_multi_shared` — Faz 4'ün `staging::stage_part`/`promote`/`discard` altyapısını
paylaşır (yeni bir staging mekanizması icat edilmedi).

**Kapı muafiyeti — K11'in bilinçli istisnası.** K11 aksiyon kapısının depo ∪ staging
birleşimine bakmasını söylüyordu, çünkü orada bir AKSİYONUN geçip geçemeyeceği
soruluyordu. Bu uçta soru farklı: aksiyon yok, "hangi aksiyon bu grubu kapatıyor" burada
hiç sorulmaz. Katalog referansının söylediği "bu grup burada toplanır" yeterli —
`gates_action` süzmesi bilerek UYGULANMADI. Kapı, ancak bir aksiyon submit edildiğinde
(`apply_action`/`submit_action`) ayrıca sorulur.

**JWT simetrisi:** `PUT /portal/wfe/{wfe_id}/attachments` aynı gün eklendi — direkt
X-Actor ve JWT ağaçları bu uçta paritede (tek-dosya PUT'un aksine, orada JWT DURUYOR ama
X-Actor karşılığı K9'da kaldırıldı; burada ikisi de yeni ve simetrik).
