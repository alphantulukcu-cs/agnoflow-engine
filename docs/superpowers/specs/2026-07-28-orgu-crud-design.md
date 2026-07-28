# Orgu (Org Birimi) CRUD — Design Specification

**Date:** 2026-07-28
**Status:** Approved
**Scope:** Organizations sayfasında (`OrgExplorer` → "Org Ağacı" sekmesi) orgu (org birimi)
create/edit/delete + yönetilebilir birim-tipi kataloğu. Tenant/kullanıcı/rol yönetimi bu
işin dışında (bkz. `2026-05-20-org-ui-design.md` / frontend'deki
`2026-06-12-org-management-design.md` — daha geniş, uygulanmamış kapsam).

---

## 1. Mevcut durum

- `crates/org/src/repo/orgu.rs`: yalnız `get`, `list_by_tree`, `get_orgt_id`, `get_orgtnt_id` var.
  Create/update/delete YOK.
- `crates/server/src/routes/org.rs`: `/orgu/{id}` (GET), `/orgt/{id}/orgu` (GET),
  `/orgu/{id}/traverse` (GET) — hepsi salt-okunur. Rol (`org.r`) ve kullanıcı (`org.u`)
  için create/update zaten var (`create_role`, `update_role`, `create_user`) — bu spec
  aynı deseni orgu'ya taşıyor.
- `org.orgu_type` JSONB serbest bir alan; traversal filtreleri `orgu_type->>'key' = 'val'`
  şeklinde okuyor (bkz. `crates/org/src/traversal/executor.rs::filter_sql`). Bilinen
  anahtar hep `"type"` (örn. `{"type": "sube"}`) — `data/seed_qnb_regions.sql` bunu
  doğruluyor (`seed_orgu_type` fonksiyonu).
- Frontend "Organizations" sayfası = `OrgExplorer.tsx` → `tree` sekmesi → `LeftPanel.tsx`
  (graph/list toggle) + `TreeList.tsx` / `OrgGraphNode.tsx`. Roller zaten aynı dosyada
  `RoleManagementPanel` ile yönetiliyor (liste + yeni-ekle formu) — yeni "Birim Tipleri"
  paneli bunu birebir taklit eder.
- Path (ltree) bugün yalnız seed script'lerinde elle üretiliyor
  (`seed_orgu_segment`: `metadata->>'code'` slugify edilir). UI'dan oluşturulan birimler
  için bu koda ihtiyaç yok — global unique index (`orgu_seed_code_unique` on
  `metadata->>'code'`) çakışma riski taşır, o yüzden UI-oluşturma bu alanı kullanmaz.

---

## 2. Birim-tipi kataloğu (yeni, tenant-scoped)

Roller (`org.r`) ile birebir aynı desen:

```sql
CREATE TABLE org.orgu_type_def (
    type_id      uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    key          text        NOT NULL,   -- orgu_type->>'type' değeriyle eşleşir
    display_name text        NOT NULL,
    is_active    boolean     NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, key)
);
CREATE INDEX orgu_type_def_orgtnt_idx ON org.orgu_type_def(orgtnt_id);
```

Migration ayrıca **mevcut tenant'lar için 5 bilinen tipi seed eder**
(`root`, `bolge`, `sehir`, `ilce`, `sube` — `src/store/org-data.store.ts::getOrguIcon`'dan
alınan mevcut vocabulary), böylece bugün var olan birimlerin tipi dropdown'da geçersiz
görünmez.

**Repo (`crates/org/src/repo/orgu_type.rs`, yeni dosya):**
- `list(pool, orgtnt_id) -> Vec<OrguTypeDef>`
- `create(pool, orgtnt_id, key, display_name) -> OrguTypeDef` — `ON CONFLICT (orgtnt_id, key)
  DO UPDATE SET display_name=EXCLUDED.display_name, is_active=true` (create_role ile aynı
  reaktivasyon deseni).
- `update(pool, orgtnt_id, type_id, key, display_name) -> OrguTypeDef`
- `deactivate(pool, orgtnt_id, type_id) -> bool` — var olan orgu kayıtlarını ETKİLEMEZ
  (orgu_type bir JSONB kopyası, foreign key değil).

**Routes (`org.rs`):**
- `GET /org/orgtnt/{id}/orgu-types`
- `POST /org/orgtnt/{id}/orgu-types` — body `{ key, display_name }`
- `PATCH /org/orgtnt/{id}/orgu-types/{type_id}` — body `{ key, display_name }`
- `DELETE /org/orgtnt/{id}/orgu-types/{type_id}`

Hepsi mevcut `X-Admin-Key` kapısı altında.

---

## 3. Backend: Orgu CRUD

**`crates/org/src/repo/orgu.rs`'e eklenecek:**

```rust
pub async fn create(
    pool: &PgPool, orgtnt_id: Uuid, orgt_id: Uuid,
    parent_orgu_id: Option<Uuid>, name: &str, type_key: &str,
) -> Result<Orgu, OrgError>
```
- Yeni `orgu_id` üretilir (`uuid_generate_v4()`), path segmenti **kod DEĞİL** —
  `format!("u_{}", orgu_id.simple())` (tire olmadan uuid; ltree label kurallarına uygun,
  global unique code index'i tetiklemez).
- `parent_orgu_id = None` → path = segment (kök birim o ağaçta).
- `parent_orgu_id = Some(p)` → `org.orgt_orgu`'dan ebeveynin path'i okunur, segment
  eklenir. Ebeveyn bulunamazsa/başka ağaçtaysa `OrgError::NotFound`.
- `orgu_type` = `{"type": type_key}`.
- Tek transaction: `org.orgu` INSERT + `org.orgt_orgu` INSERT.

```rust
pub async fn update(
    pool: &PgPool, orgu_id: Uuid, name: &str, type_key: &str,
) -> Result<Orgu, OrgError>
```
- Yalnız `name` ve `orgu_type` günceller. **Path/parent değişmez** (approved eski spec'teki
  kısıtla aynı — "path is derived, immutable").

```rust
pub async fn delete_cascade(pool: &PgPool, orgu_id: Uuid) -> Result<i64, OrgError>
```
- Hedef birimin `path`'ini okur; `org.orgt_orgu.path <@ target_path` olan TÜM satırların
  (hedef dahil) `is_active=false` yapar (`org.orgt_orgu`), karşılık gelen `org.orgu.is_active`
  de false'a çekilir (yalnız o birimlerin başka aktif `orgt_orgu` konumu yoksa — orgu bir
  ağaçta tekil konumlanıyor pratikte, ama şema çoklu-tree'ye izin veriyor, bu yüzden
  `org.orgu`'yu yalnızca ilgili `orgt_orgu` satırları da pasifse pasifle). Etkilenen satır
  sayısını döndürür (UI "N birim pasifleştirildi" gösterir).

**Routes (`org.rs`):**
- `POST /org/orgt/{orgt_id}/orgu` — body `{ name, type_key, parent_orgu_id: Option<Uuid> }`
- `PATCH /org/orgu/{id}` — body `{ name, type_key }`
- `DELETE /org/orgu/{id}` — response `{ deactivated_count: i64 }`

---

## 4. Frontend

**`src/api/engineApi.ts`** — yeni fonksiyonlar (mevcut `createRole`/`updateRole` deseniyle
aynı, `adminHeaders(adminKey)` kullanır):
`listOrguTypes`, `createOrguType`, `updateOrguType`, `deleteOrguType`,
`createOrgu`, `updateOrgu`, `deleteOrgu`.

**`src/store/org-data.store.ts`** — `loadData()` bugün `loadedKey` eşleşirse no-op yapıyor;
mutasyon sonrası yenileme için `refresh()` action'ı eklenir (loadedKey'i sıfırlayıp
`loadData()`'yı tekrar tetikler).

**UI:**
- **Birim Tipleri paneli** — `RoleManagementPanel`'in birebir kopyası (liste + yeni-ekle
  formu + rename), `OrgExplorer.tsx` içinde tree sekmesinin araç çubuğunda açılır/kapanır
  bir bölüm olarak.
- **`TreeList.tsx`** — her satıra `TypeBadge`'in yanına düzenle (kalem) ve sil (çöp kutusu)
  ikonları eklenir; hover'da "+" (bu birimin altına çocuk ekle) belirir. `UsersPage.tsx`'in
  satır aksiyonlarıyla aynı görsel dil.
- **`OrgGraphNode.tsx`** — kart üzerinde hover'da aynı üç ikon (düzenle/sil/+).
- **Create/Edit modal** — `OrgCaModal.tsx` ile aynı görsel stil; alanlar: `name` (text),
  `type` (dropdown, birim-tipi kataloğundan). Create modunda ebeveyn salt-okunur gösterilir
  ("X birimi altında oluşturulacak" / seçili birim yoksa "Kök birim olarak oluşturulacak").
- **Delete** — `window.confirm` (UsersPage.removeUser deseni); hedefin çocuğu varsa uyarı
  metni "Bu birim ve N alt birim pasifleştirilecek" gösterir (silmeden önce çocuk sayısı
  `orgus` state'inden path-prefix ile hesaplanır, ayrı bir API çağrısı gerekmez).
- **Araç çubuğu** — "+ Yeni birim" butonu: seçili birim varsa onun altına, yoksa kök
  seviyede oluşturur.

---

## 5. Hata durumları

- Ebeveyn bulunamadı / farklı ağaçta → `404`, form hata mesajı gösterir.
- `type_key` kataloğunda yok → backend `400` döner (whitelist kontrolü create/update'te
  yapılır: `orgu_type_def` içinde `orgtnt_id + key` aktif olmalı).
- Cascade delete sırasında hata → transaction geri alınır, hiçbir satır değişmez.
- Tip kataloğunda `key` boşsa veya tekrar edilmiş aktif kayıt varsa → `400`
  (`create_role` ile aynı davranış, ama burada ON CONFLICT reaktivasyon yapar).

---

## 6. Test planı

- Backend: `crates/org` içinde yeni repo testleri — create (kök + child), update
  (name/type değişir, path değişmez), cascade delete (3 seviyeli ağaçta orta düğüm
  silinince tüm alt ağacın pasifleştiği doğrulanır), orgu_type_def CRUD + reaktivasyon.
  `cargo test --workspace` ile birlikte koşar.
- Frontend: `TreeList`/`OrgGraphNode` için yeni aksiyon butonlarının render/click testleri;
  cascade-delete confirm metninin çocuk sayısını doğru hesapladığına dair birim testi;
  tip dropdown'unun registry endpoint'inden beslendiğine dair test.

---

## 7. Kapsam dışı

- Tenant/kullanıcı/rol CRUD (mevcut ayrı sayfalar/panel'ler zaten var).
- Orgu'nun ağaç/parent değiştirmesi (taşıma) — path immutable kalıyor, bu ayrı bir iş.
- Çoklu-tip (multi-select) — kullanıcı tek-tip seçti, JSONB'de tek `"type"` anahtarı kalır.
- Migration'ın otomatik uygulanması — repo kuralı gereği `psql` ile elle uygulanır.

---

## 8. Başarı kriterleri

- [ ] `org.orgu_type_def` migration'ı yazıldı, mevcut 5 tip seed edildi.
- [ ] Backend: orgu create/update/cascade-delete + tip kataloğu CRUD endpoint'leri çalışıyor.
- [ ] Frontend: Org Ağacı sekmesinde hem List hem Graph görünümünde düzenle/sil/ekle
  ikonları var; Birim Tipleri paneli roller panelinin aynısı.
- [ ] Cascade delete alt birimleri doğru pasifleştiriyor, UI etkilenen sayıyı gösteriyor.
- [ ] `cargo test --workspace` yeşil; yeni frontend testleri geçiyor.
