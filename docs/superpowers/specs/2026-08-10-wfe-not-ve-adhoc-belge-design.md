# WFE Notları ve Ad-hoc Belge Paylaşımı — Tasarım

**Tarih:** 2026-08-10
**Durum:** Onaylandı, uygulanmayı bekliyor
**İlgili:** `docs/spec/decisions.md` Madde 8 (attachments), `crates/server/src/attachments.rs`,
`crates/server/src/reservation.rs`, `migrations/wf/20260807000001_wfe_reservation.sql`,
`docs/superpowers/specs/2026-08-04-env-config-design.md`

## Problem

İş elden ele geçerken insanlar birbirine **akış tasarlanırken öngörülemeyen** şeyler söylemek
ve göndermek zorunda kalıyor:

- Müdür işi memura geri gönderirken: *"kredi miktarını biraz daha yükselt, öyle yolla."*
  Memur bu notu görebilmeli.
- Müdür genel müdüre onaya gönderirken: *"şu belgelere de bir bakınız"* deyip katalogda
  tanımlı olmayan birkaç dosya iliştirebilmeli.

Bu içerikler akış tasarımına (WFD) yazılamaz: hangi adımda, kime, ne söyleneceği tasarım
anında bilinemez. Bugün engine'in bu senaryolar için hiçbir yeri yok. Tasarımcıdan öngörü
istemeden, her akışta ve her adımda çalışan bir mekanizma gerekiyor.

İkincil problem, birinciyi çözerken ortaya çıktı: not "hangi aksiyonla gitti" bilgisine
bağlanacak, ama motorun defteri `wf.wfah` **sadece aksiyon adını** tutuyor — hangi adımdan
hangi adıma gidildiği kayıtlı değil. "Bu onaya nereden gelindi" sorusu bugün ancak akışı
baştan yeniden oynatarak cevaplanabiliyor.

## Alınan kararlar ve gerekçeleri

### K1 — Not ve belge ÜÇÜNCÜ bir katmandır; ne context ne wfah

Üç aday yer vardı, ikisi elenir:

**Context (`$ctx` / `wf.wfe_dynctx`) — HAYIR.** Context motorun karar verirken okuduğu
değişkenler kutusudur; tek yazma yolu `wfes_effects`'tir (WOR-70) ve alanların tipi
`collectActionInputCtxMap` üzerinden çıkarılıp `expr_types.rs` ile denetlenir. Ad-hoc
anahtar bu çıkarımı bozar, anahtar çakışması riski taşır ve "kim ne zaman yazdı" bilgisini
zaten tutamaz.

**WFAH (`wf.wfah`) — HAYIR.** Bu motorun resmi defteridir ve `$wfah` olarak ZEN'e akar.
Yayınlanmış akışlar bu defteri **sayarak** karar veriyor (`count($wfah, #.action == "x") >= n`).
Araya sistem-notu satırı koymak bu sayımları kaydırır, `$prev`/`$first` kısayollarının anlamını
değiştirir ve `project_entry` izdüşümünü kirletir. Motorun defterine insan yorumu yazılmaz.

**Ayrı not defteri — EVET.** Örnek (WFE) bazlı, şemasız, insan üretimi içerik kendi tablosunda
durur. Engine core bu katmandan **habersizdir**; tüm iş `server` crate'inde, attachments'ın
bugün yaptığı gibi portal/edge katmanındadır.

### K2 — Motor notları GÖRMEZ; `$notes` diye bir namespace YOK

Not yönlendirmeyi etkilerse artık ad-hoc değil, tasarım verisidir — doğru araç WFD'de tanımlı
bir action input alanıdır. Sınır bilinçli olarak nettir:

| İçerik | Yer | Motor okur mu |
|---|---|---|
| Akışın kararını etkileyen veri | WFD `actions[].input` → `wfes_effects` → `$ctx` | Evet |
| İnsandan insana mesaj / belge | `wf.wfe_note` | Hayır |

İleride gerçek talep gelirse `v22/dollar.rs` (`EXACT`/`PREFIXES`) + `expr_types.rs` genişletilerek
salt-okunur eklenebilir; bu tasarım o kapıyı kapatmıyor, sadece açmıyor.

### K3 — Yayınlanmış not DEĞİŞTİRİLEMEZ

Not karar delilidir: "müdür yükselt dedi" bilgisi sonradan düzenlenebilirse denetim değeri
sıfırlanır. Publish sonrası `body` ve dosya kümesi üzerinde UPDATE yoktur. Silme yerine
**gizleme**: `hidden_at`/`hidden_by` dolar, gövde DB'de kalır, API `{hidden: true}` döner.
Düzeltme yolu yeni not yazmaktır.

Draft aşamasında (henüz yayınlanmamış, yalnız yazarı görüyor) serbestçe düzenlenir/silinir.

### K4 — Belge eklemek müşterinin depo ayarını ŞART koşar

`attachment_store::store_for_wfd` bugün `$env`'de anahtar yoksa **sessizce deployment
varsayılanına düşüyor**. Katalog tarafında bu, publish kapısıyla (`assert_attachment_storage_env`)
engelleniyor — ama ad-hoc belge *her* akışta olabileceği için o publish kapısını her WFD'ye
zorlamak yanlış olur: belge iliştirilmeyen yüzlerce akış yayınlanamaz hale gelirdi.

Karar: **kapı publish'te değil, runtime'da ve yalnız not-dosyası rotasında.** Bu rota fallback
yolunu kapatır; `$env` anahtarları eksikse `422 code:"attachment_storage.missing_env"` döner.
Müşterinin belgesi bizim sunucu diskine sessizce yazılmaz.

Sonuç iki ayrı yetenek seviyesi:

| Yetenek | Ön koşul |
|---|---|
| Metin notu | Yok — her akışta, her tenant'ta çalışır (yalnız DB) |
| Nota belge iliştirme | WFD'nin `$env`'inde attachment storage tanımlı olmalı |

### K5 — Not, aksiyonla BİRLİKTE yayınlanır (draft → publish deseni)

Belge yüklemesi ayrı bir HTTP isteğidir; dosya iliştirilecekse notun gövdesinden önce bir
kimliği olmak zorundadır. Aynı problem başlatma-öncesi belge yüklemesinde çözülmüştü
(`POST /wfe/reserve` → upload → `POST /wfe`), aynı deseni tekrar ediyoruz:

1. `POST /wfe/:id/notes` → **draft** not, yalnız yazarı görür → `note_id`
2. (opsiyonel) `PUT /wfe/:id/notes/:note_id/files` → dosya, n kez
3. Yayınlama **YALNIZ aksiyonla:** `POST /wfe/:id/actions` gövdesinde `note_id`. Engine commit
   **başarılıysa** not `published` olur, `wfah_seq` = commit'in ürettiği seq, `node` = geçişin
   `from_node`'u.

**2026-08-11 revizyonu (kural sahibinden):** "not yazılır, dosya eklenir, AKSİYON ALINDIĞINDA
bunlar yayınlanır; claim etmeden, aksiyon almadan not ve dosya eklenemez."

- **Serbest yayın KALDIRILDI.** Başlangıçtaki ikinci yol (aksiyona bağlı olmayan
  `POST .../publish`, `wfah_seq = NULL`) kalktı: yayınlanmış her not artık bir aksiyona
  çapalıdır. `POST .../publish` ucu duruyor ama sözleşmesi daraldı — yalnız "apply BAŞARILI
  oldu, not yayınlanamadı (`note_error`)" arızasının telafisidir ve WFE'nin EN SON wfah
  kaydının çağıran aktöre ait olmasını ŞART koşar (409 `note.requires_action`).
  Uygulama: `notes::republish_after_apply`; `notes::publish` artık modül-içi (`pub` değil).
- **Not/dosya EKLEMEK claim ister** (`notes::assert_actor_holds_claim`; create note, draft
  güncelleme, dosya yükleme). Kapı `Engine::apply` §7.1'in sorduğu soruyu sorar
  (`NotClaimed`/`NotOwner`) — 409 `note.requires_claim`. Gerekçe: yayını aksiyona bağladıktan
  sonra, claim'i olmayan aktörün taslağı HİÇBİR zaman yayınlanamaz; K6 görünürlüğü tek kapı
  bırakılsaydı sistem yalnızca süpürücüye yem üretirdi. Paralel modda WFE-seviyesi `claimed_by`
  anlamsızdır: aktif kollardan biri o aktörde olmalı.
- **Claim İSTEMEYENLER**: okuma uçları, okundu işaretleme, kendi taslağını silme / published
  notu gizleme. Claim düştükten sonra da kendi taslağını temizleyebilmeli.

Apply 422/409 alırsa not draft kalır; kullanıcı düzeltip tekrar dener. Yayınlama motorun
transaction'ının **içinde değil, sonrasındadır** — `WfeStore::commit`'in atomikliğine
dokunmuyoruz. Not yazımı başarısız olursa apply sonucu yine döner, cevaba `note_error` eklenir;
not draft kalır ve kullanıcı yeniden yayınlayabilir. Ters sıra (notu önce yayınla) daha kötüdür:
gerçekleşmemiş bir geçişe bağlı not üretirdi.

Yetim draft'lar mevcut saatlik süpürücüye (`server/src/reservation.rs`) eklenir, TTL 24 saat,
dosyalarıyla birlikte.

### K6 — Yetki: WFE'yi görebilen notu okur

Baseline `executor.query(wfe_id, actor)` — attachment rotalarının bugün kullandığı görünürlük
kapısının aynısı. Ayrı bir yetki modeli icat edilmiyor: not, bağlı olduğu işin görünürlüğünü
miras alır. Üstüne opsiyonel `audience` süzgeci (K9) gelir. Draft yalnız yazarına görünür.

### K7 — WFAH'a akış izi eklenir (`from_node` / `to_node`)

Not `wfah_seq` ile motorun defterine çapa atar. Ama defter bugün sadece aksiyon adını tutuyor:

```
1  gönder
2  onayla
3  reddet
4  gönder
5  onayla   ← not burada
```

"5. onayla hangi adımdaydı, oraya nereden gelindi" cevapsız. İki kolon eklenince tam yol çıkar:

```
5  onayla   [müdür onayı → genel müdür onayı]
```

**Motor tipine (`WfahEntry`) alan EKLENMEZ.** O tip `project_entry` ile `$wfah`'a akıyor ve
golden fixture'da serileşiyor; alan eklemek spec yüzeyini ve fixture'ı değiştirirdi. Bilgi
`WfeAdapter` seviyesinde türetilir — `CommitOutcome` zaten hedefi, commit tx'i içindeki
`wfe.current_node` (paralelde outcome varyantının `from_node`'u) zaten kaynağı biliyor.

Bu ekleme **yalnız kayıt ve ekran** içindir: `$wfah` izdüşümü aynı kalır, yayınlanmış akışların
koşul ifadeleri etkilenmez.

Bir commit'te birden çok wfah satırı varsa (trigger marker'ları, `_branch_cancelled` vb.) hepsi
aynı from/to alır — satırlar tek bir geçişin parçasıdır.

### K8 — Alt akış (WFC) notlarının görünürlüğü TASARIMCI kararıdır

Çağıran akışın, çağırdığı alt akışın notlarını görmesi bazen doğru (aynı ekip, uçtan uca takip),
bazen yanlıştır (alt akış başka bir birimin iç yazışması). Sabit bir kural yerine node bazında
anahtar: WFD şemasında **`callRef`** üzerinde (yani `nodes.<key>.call` — "nasıl çağrıldı"
tarafı, katalog değil) `notes_visible_to_caller`, varsayılan `false`.

Açıksa çağıranın `GET /wfe/:id/notes` cevabına alt akışın **published** notları da girer,
`from_call` etiketiyle. Draft'lar hiçbir koşulda sızmaz.

### K9 — `audience` ile hedefleme, ayrı tabloyla okundu takibi

`audience jsonb`: `{"kind":"all"}` (varsayılan) | `{"kind":"users","ids":[…]}`. Kolon Faz 1'de
şemaya girer, süzgeç Faz 3'te devreye alınır — sonradan migration gerekmesin.

`node:` / `role:` hedefleme kapsam dışı: matcher çağrısı gerektirir ve senaryolar bunu
istemiyor. Gerekirse aynı kolon içinde yeni `kind` olarak eklenir.

Kişi bazlı okundu takibi (`wf.wfe_note_read`) Faz 3'e bırakılır. Faz 1'de liste ekranlarında
kayıt tutmayan basit bir **sayaç** (`note_count`) yeterlidir: memur listeye bakınca notun
varlığını görür, tıklayıp okur.

## Veri modeli

```sql
-- Faz 0
ALTER TABLE wf.wfah
    ADD COLUMN from_node text,   -- NULL = start (öncesi yok) veya eski satır
    ADD COLUMN to_node   text;

-- Faz 1
CREATE TABLE wf.wfe_note (
    note_id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id          uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    orgtnt_id       uuid        NOT NULL,
    author_orgu_id  uuid        NOT NULL,
    author_user_id  uuid        NOT NULL,
    author_role     text        NOT NULL,
    body            text        NOT NULL,
    -- Yazıldığı/yayınlandığı andaki adım; paralelde kol node'u.
    node            text,
    -- Motorun defterine çapa: bu not hangi aksiyonla gitti. NULL = serbest not.
    wfah_seq        integer,
    audience        jsonb       NOT NULL DEFAULT '{"kind":"all"}',
    status          text        NOT NULL CHECK (status IN ('draft','published')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    published_at    timestamptz,
    hidden_at       timestamptz,
    hidden_by       uuid
);
CREATE INDEX wfe_note_wfe_idx    ON wf.wfe_note(wfe_id);
CREATE INDEX wfe_note_author_idx ON wf.wfe_note(author_user_id) WHERE status = 'draft';

-- Faz 2
CREATE TABLE wf.wfe_note_file (
    file_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    note_id     uuid        NOT NULL REFERENCES wf.wfe_note(note_id) ON DELETE CASCADE,
    filename    text        NOT NULL,
    mime        text        NOT NULL,
    size_bytes  bigint      NOT NULL,
    storage_key text        NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wfe_note_file_note_idx ON wf.wfe_note_file(note_id);

-- Faz 3
CREATE TABLE wf.wfe_note_read (
    note_id uuid        NOT NULL REFERENCES wf.wfe_note(note_id) ON DELETE CASCADE,
    user_id uuid        NOT NULL,
    read_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (note_id, user_id)
);
```

**Storage anahtarı:** `notes/{wfe_id}/{file_id}` — katalog `attachments/{wfe_id}/{grup}/{item}`
prefiksinden **ayrı**. Ad-hoc dosyanın katalog karşılığı (grup/item, format kuralı) yoktur;
aynı ağaca karıştırmak `status_for_node` ve gate mantığını yanıltır. `AttachmentStore::remove_all`
iki prefiksi de süpürecek şekilde genişler.

## API yüzeyi

Her uç **iki route ağacında** da yaşar (kod tekrarı değil, ortak modül + iki ince kabuk —
`crate::attachments` / `routes/attachments.rs` / `routes/portal/attachments.rs` deseninin aynısı):

| Uç | Açıklama |
|---|---|
| `POST   /wfe/:id/notes` | Draft not yarat → `note_id` (2026-08-11: claim ŞART) |
| `PATCH  /wfe/:id/notes/:note_id` | Draft gövdesini düzenle (yalnız yazarı, yalnız draft; claim ŞART) |
| `DELETE /wfe/:id/notes/:note_id` | Draft sil / published gizle (`hidden_at`) — claim istemez |
| `POST   /wfe/:id/notes/:note_id/publish` | 2026-08-11: yalnız apply sonrası yeniden deneme (son wfah kaydı çağıranın olmalı) |
| `GET    /wfe/:id/notes` | Görünür notlar + dosya metadata, `wfah_seq` ile |
| `PUT    /wfe/:id/notes/:note_id/files` | Dosya yükle (binary; `X-Filename` + `Content-Type`; claim ŞART) |
| `GET    /wfe/:id/notes/:note_id/files/:file_id` | İndir |
| `DELETE /wfe/:id/notes/:note_id/files/:file_id` | Sil (yalnız draft) |

Mevcut uçlarda değişiklik:

- `POST /wfe/:id/actions` gövdesine opsiyonel `note_id` (K5). Göndermeyen istemci için davranış
  **hiç değişmez**.
- `GET /wfe/:id` → `WfeView`'a `path` (Faz 0 izi; `WfahEntry`'ye değil, view'a) ve `note_count`.
- `GET /wfe`, portal pool listesi → satır başına `note_count`. Tek sorguda toplanır, N+1 yok
  (`repo::wfah::max_seq_by_wfe` deseni).

Yanıt/hata kodları:

| Kod | Durum |
|---|---|
| `attachment_storage.missing_env` | 422 — nota dosya yükleniyor ama WFD'nin `$env`'inde depo tanımsız (K4) |
| `note.immutable` | 409 — yayınlanmış nota düzenleme/dosya girişimi (K3) |
| `note.not_draft` | 409 — zaten yayınlanmış notu tekrar yayınlama |
| `note.too_large` / `note.unsupported_type` | 413 / 415 — ad-hoc dosya limitleri |

## Ad-hoc dosya limitleri

Katalog `AttachmentItem.formats` kuralları ad-hoc dosyaya uygulanamaz (tanım yok), yerine
sunucu tarafı sabitleri konur — aksi halde sınırsız yükleme yüzeyi doğar:

- dosya başı boyut sınırı
- not başı dosya sayısı sınırı
- WFE başı toplam kota
- MIME blocklist (çalıştırılabilirler)
- dosya adı sanitizasyonu (yol ayracı, kontrol karakterleri)
- indirmede `Content-Disposition: attachment` + `X-Content-Type-Options: nosniff` —
  `crate::branding`'in SVG servis deseninin aynısı

Virüs taraması kapsam dışı; ileride edge katmanına eklenebilir.

## Değişmezler (bu tasarımın çapası)

- `wfe-core` pipeline / validator / matcher / visibility **dokunulmaz**.
- Golden fixture (`docs/spec/examples/kredi-basvuru.golden.json`) değişmez.
- `$wfah` izdüşümü aynı kalır: `{seq, action, actor, input, at}`.
- Context'e tek yazma yolu `wfes_effects` olarak kalır.
- Not/belge hiçbir ZEN namespace'ine girmez.
- WFD dokümanına giren tek şey Faz 4'teki `notes_visible_to_caller` anahtarıdır; not İÇERİĞİ
  hiçbir zaman WFD'ye yazılmaz.

## Fazlar

### Faz 0 — WFAH akış izi

| Dosya | İş |
|---|---|
| `migrations/wf/…_wfah_path.sql` | `from_node` / `to_node` kolonları (nullable) |
| `crates/wfe/src/wfe_adapter.rs` | `insert_wfah_entries` imzası + `commit`/`create` yollarında türetme |
| `crates/wfe/src/models.rs`, `repo/wfah.rs` | `WfahRow` + SELECT |
| `crates/wfe/src/executor.rs` | `WfeView`'a `path` listesi |

Test: düz geçiş, fork, join (AND/quorum/expr), collapse, terminal, start — her birinde from/to
doğru; mevcut `$wfah` testleri değişmeden yeşil.

### Faz 1 — Not defteri (metin)

| Dosya | İş |
|---|---|
| `migrations/wf/…_wfe_note.sql` | tablo + indeksler |
| `crates/server/src/notes.rs` *(yeni)* | ortak mantık — `crate::attachments`'ın kardeşi |
| `crates/server/src/routes/notes.rs` *(yeni)* | `/wfe/*` ağacı (X-Actor) |
| `crates/server/src/routes/portal/notes.rs` *(yeni)* | `/portal/wfe/*` ağacı (JWT) |
| `crates/server/src/routes/wfe.rs` | `ApplyBody.note_id` + commit sonrası bağlama |
| `crates/server/src/reservation.rs` | süpürücüye yetim draft temizliği |
| `crates/server/src/openapi.rs` | yeni uçlar |
| work-pool-portal | `InstanceDetail` timeline (wfah + not tek liste), aksiyon formunda not kutusu, havuzda `note_count` rozeti |

Test: draft başkasına görünmez · publish `wfah_seq` bağlar · published düzenlenemez (409) ·
gizlenen not `{hidden:true}` döner · görünürlüğü olmayan aktör 403 · süpürücü yetim draft'ı siler.

### Faz 2 — Belge iliştirme

| Dosya | İş |
|---|---|
| `migrations/wf/…_wfe_note_file.sql` | tablo |
| `crates/server/src/attachment_store.rs` | `notes/` prefiksi + `remove_all` genişletmesi + **fallback'siz** resolve varyantı |
| `crates/server/src/notes.rs` | yükleme doğrulaması, limitler |
| rotalar | `PUT/GET/DELETE …/files` (iki ağaç) |
| work-pool-portal | not kutusuna dosya ekleme, timeline'da dosya listesi |

Test: `$env` eksikken 422 `attachment_storage.missing_env` (deployment varsayılanına
**düşmediği** açıkça doğrulanır) · boyut/tip/kota redleri · published nota dosya eklenemez ·
WFE silinince dosyalar süpürülür.

### Faz 3 — Hedefleme + okundu

`audience` süzgeci devreye alınır, `wf.wfe_note_read` eklenir, havuz rozeti "okunmamış"a döner.

### Faz 4 — Alt akış notu görünürlüğü

| Dosya | İş |
|---|---|
| `docs/spec/schema.json` + frontend kopyası `src/schema/wfd.schema.json` | `callRef.notes_visible_to_caller` (ikisi BİRLİKTE — CLAUDE.md kuralı) |
| `crates/wfe-core/src/types/wfd_v22.rs` | tip alanı (motor okumaz, taşır) |
| `crates/server/src/notes.rs` | çağıran görünümüne alt akış notlarını `from_call` etiketiyle ekleme |
| agnoflow-frontend | call node ayarlarında anahtar |

## Sıra ve bağımlılık

Faz 0 → Faz 1 (not, izin çapasına bağlı). Faz 2 / 3 / 4 birbirinden bağımsız ve Faz 1'den sonra
herhangi bir sırada. **Faz 0 + Faz 1 tek başına sevk edilebilir** ve problemdeki iki senaryonun
metin ayağını tamamen karşılar.

## Reddedilen alternatifler

| Yaklaşım | Neden reddedildi |
|---|---|
| Notu `wf.wfah`'a sistem aksiyonu (`__note`) olarak yazmak | `$wfah` sayımlarını kaydırır, yayınlanmış akışları bozar (K1) |
| Notu `wfes_effects` ile `$ctx`'e yazmak | Tek yazma yolu sözleşmesini ve tip çıkarımını bozar; yazar/zaman bilgisi tutulamaz (K1) |
| WFD'ye her node'a "not alanı" eklemek | Tam da öngörülemeyen şeyi öngörmeyi ister; yeni versiyon publish etmeyi gerektirir |
| Ad-hoc dosyayı katalog prefiksine (`attachments/{wfe_id}/…`) yazmak | Katalogda karşılığı olmayan anahtar `status_for_node`/gate mantığını yanıltır |
| Ad-hoc dosyada deployment varsayılan deposuna düşmek | Müşteri belgesini sessizce bizim diske yazar — `attachment_storage` kararının koruduğu şeyin aynısı (K4) |
| Ad-hoc belge için WFD'ye zorunlu opt-in + publish kapısı | Belge iliştirmeyen yüzlerce akışı yayınlanamaz hale getirir; runtime reddi aynı güvenliği ucuza verir (K4) |
| Notu apply'dan ÖNCE yayınlamak | Gerçekleşmemiş geçişe bağlı not üretir (K5) |
| Notu engine commit transaction'ının içine almak | `WfeStore::commit` atomikliğini insan içeriğine bağlar; engine core'a I/O sızdırır |
