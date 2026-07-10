# WFD Versiyonlama — Draft/Published (Enterprise) — Tasarım

**Tarih:** 2026-07-10
**Kapsam:** `workflow-engine` (wfd crate + server /wfd + migration) ve `WFD-EDITOR` (create ekranı, versiyon tab'ı, read-only editör).

## Amaç

Tasarımcı, WFD'yi editör ekranından enterprise seviyede versiyonlayabilsin:

- "Yeni WFD" artık doğrudan editörü açmaz; önce name/description/tags toplayan bir create ekranı gelir, "Oluştur" v1 **draft** üretir.
- WFD engine'e yüklenmese de **draft** olarak kaydedilebilir; tasarımcı yarım bıraktığını sonradan sürdürür.
- Her WFD'ye tıklanınca versiyonları listelenir; her versiyon **Draft** veya **Published** olarak görünür.
- Geçmiş versiyonlar **read-only** açılıp incelenebilir.
- Published bir versiyonu read-only'den çıkarıp düzenlemek **yeni versiyon (draft)** üretir.

## Kimlik modeli (mevcut gerçek — korunuyor)

Versiyon soyağacı `(orgtnt_id, name)` ile gruplanır. Her versiyon kendi `wfd_id`'sine sahip **ayrı** bir `wf.wfd_meta` satırıdır (bkz. `crates/wfd/src/adapter.rs` — her `create` çağrısı `Uuid::new_v4()` üretir). `next_version = MAX(version) + 1` for `(orgtnt_id, name)`.

- "Bir WFD" = tenant içindeki bir **isim**.
- Bir **draft**, o ismin "sonraki versiyon" slotunu (`max+1`) sahiplenen, kendi `wfd_id`'li satırdır.

## Kararlar (brainstorm'da onaylandı)

1. **Draft store = engine**, `wf.wfd_meta` üzerinde `status` kolonu ile. JSON yine OpenDAL'da. Upload'taki v2.2 validator kapısı draft'ta **atlanır**.
2. **Tek açık draft = max+1.** Bir isim için aynı anda en fazla 1 draft; her zaman bir sonraki versiyon. Published'ı edit'e açmak bu draft'ı oluşturur.
3. **Metadata:** `name` (zorunlu), `description`, `tags`, `owner` (şimdilik default `'admin'`; JWT entegrasyonu ileride).
4. **Doğrulama:** draft save'de **hiç**; publish'te **tam** (v2.2 validator + graf/slug/expression). Publish başarısızsa hata listesi döner, draft kalır.
5. **Name create'te sabitlenir**; draft-edit'te sadece `description`/`tags` düzenlenebilir (rename YAGNI — grup anahtarı).

## Engine şema değişikliği (`migrations/wf` yeni migration)

`wf.wfd_meta`'ya eklenir:

```sql
ALTER TABLE wf.wfd_meta
  ADD COLUMN status      text        NOT NULL DEFAULT 'published'
      CHECK (status IN ('draft','published')),
  ADD COLUMN description text,
  ADD COLUMN tags        text[]      NOT NULL DEFAULT '{}',
  ADD COLUMN owner       text        NOT NULL DEFAULT 'admin',
  ADD COLUMN updated_at  timestamptz NOT NULL DEFAULT now();

-- isim başına en fazla tek açık draft
CREATE UNIQUE INDEX wfd_single_draft
  ON wf.wfd_meta (orgtnt_id, name)
  WHERE status = 'draft';
```

Mevcut satırların hepsi `status='published'` olarak geriye dönük doğru kalır (default).

**Değişmez güncellemesi:** immutability artık yalnızca `status='published'` satırlar için geçerlidir. Draft satırların JSON'u ve metadata'sı **mutable**'dır. `(wfd_id, version)` cache yalnızca published satırlar için güvenle kullanılır; draft fetch cache'i bypass eder (veya PUT'ta cache invalidate edilir).

## Engine endpoint'leri (`server` crate, `/wfd` router)

Mevcut kalır: `POST /wfd` (upload = publish yolu, validate'li), `GET /wfd` (list), `GET /wfd/:id/:version` (fetch).

Yeni:

| Endpoint | Davranış |
|---|---|
| `POST /wfd/draft` | Body: `{orgtnt_id, name, description?, tags?}`. İskelet v2.2 JSON üretir (symmetric-start iskeleti), `status='draft'`, `version=next_version`, `owner='admin'`. **Validasyon yok.** Döner `{wfd_id, version}`. Tek-draft ihlali → **409**. |
| `PUT /wfd/draft/:id/:version` | Draft JSON + metadata (`description`, `tags`) günceller, `updated_at=now()`. **Validasyon yok.** Yalnızca `status='draft'` satırda; published'a → **403**. |
| `POST /wfd/draft/:id/:version/publish` | **Tam v2.2 validator.** Geçerse `status='published'`, `updated_at`. Geçmezse **422** + hata listesi (`[code] path: message`), draft kalır. |
| `POST /wfd/:id/:version/new-draft` | Kaynak published JSON'u kopyalar → yeni draft (`version=next_version`), `description/tags/owner` devralır. Açık draft varsa → **409**. Döner `{wfd_id, version}`. |
| `DELETE /wfd/draft/:id/:version` | Draft iskarta eder (JSON + satır). Published → **403**. |
| `GET /wfd` | Yanıta `status`, `description`, `tags`, `owner`, `updated_at` alanları eklenir. |

### wfd crate (`WfdAdapter` / repo / ports) değişiklikleri

- `repo.rs`: `insert` imzasına `status/description/tags/owner`; yeni `update_draft`, `set_status`, `delete`, `get_meta` (SELECT genişletilir, draft'ı da döndürür).
- `adapter.rs`: `create_draft` (validasyonsuz iskelet), `save_draft` (validasyonsuz overwrite + cache invalidate), `publish` (validator + status flip), `new_draft_from` (kopya), `delete_draft`. Mevcut `create` (validate'li publish) korunur veya `publish` ile paylaşılır.
- İskelet JSON: geçerli minimum v2.2 (symmetric start + tek terminal) — yayınlanmadan çalıştırılamaz ama editörde açılır.

## Editör UX (`WFD-EDITOR`)

### Create akışı
- **"Yeni WFD"** butonu editörü açmaz. **Create ekranı/modal** açar: `name` (zorunlu), `description`, `tags`, `owner` (readonly `admin`).
- "Oluştur" → `POST /wfd/draft` → v1 draft → editör **draft-edit** modunda açılır.

### Editör üst barı (draft-edit modu)
- **Kaydet** → `PUT /wfd/draft/:id/:ver` (validasyonsuz). Sık kaydetme desteklenir.
- **Yayınla** → `POST .../publish`. 422'de validator hata listesi panel/toast ile gösterilir; başarılıysa read-only published görünüme geçer.

### Versiyonlar tab'ı
- Dashboard satırına tıklama artık "son versiyonu aç" yerine **Versiyonlar drawer/panel'i** açar.
- Her satır: `vN · [Draft|Published] rozeti · owner · updated_at · aksiyonlar`.
  - **Draft** → "Düzenlemeye devam et" → editör edit modu.
  - **Published** → "İncele (read-only)" → editör read-only; ayrıca "Yeni versiyon oluştur" → `POST /wfd/:id/:ver/new-draft` → editör edit modu.
- Rozet renkleri: Draft = uyarı/turuncu, Published = accent.

### Read-only editör
- Tüm mutasyonlar (node ekle/sil, kenar, form) devre dışı. Kaydet/Yayınla gizli.
- Üstte "salt-okunur vN (published)" bandı + "Yeni versiyon oluştur" butonu.

## Yaşam döngüsü / edge case'ler

- **Publish sonrası** draft satırı published olur (aynı `wfd_id`/`version`) → immutable. Sonraki edit `new-draft` ile `max+1` açar.
- **İkinci draft talebi** (create veya new-draft) → 409 → UI "zaten açık bir draft var (vN): devam et / sil" der.
- **Concurrency:** kısmi unique index iki eşzamanlı draft'ı DB seviyesinde engeller.
- **Rename:** create'te name sabit; draft-edit'te name değiştirilemez (YAGNI). İleride gerekirse ayrı iş.

## Test stratejisi

- **Engine:** `wfd` crate repo/adapter testleri (create_draft → save → publish happy path; publish invalid → 422 + draft kalır; tek-draft 409; new-draft kopya; delete). Server endpoint testleri.
- Mevcut golden fixture (`docs/spec/example-wfd_kredi-basvuru_v2_2.json`) DEĞİŞTİRİLMEZ.
- Zamana bağlı yok; standart `#[tokio::test]`.
- **Editör:** create-flow store testi, versiyon-tab render, read-only mod guard'ları, publish hata gösterimi.

## Açık noktalar (kabul edilmiş varsayımlar)

- `owner` şimdilik sabit `'admin'`; JWT gelince gerçek kullanıcı doldurulacak (ileride ayrı iş).
- Draft JSON şeması editörün ürettiği v2.2 formatıyla aynı; iskelet minimum geçerli v2.2.
