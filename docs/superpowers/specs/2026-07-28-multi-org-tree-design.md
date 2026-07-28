# Tenant Başına Çoklu Org Ağacı — Design Specification

**Date:** 2026-07-28
**Status:** Approved
**Scope:** Bir tenant içinde birden fazla org ağacı (`org.orgt`) oluşturma, birini
varsayılan işaretleme, Organizations sayfasında ağaçlar arası geçiş. **Kapsam dışı:**
ağaç pasifleştirme/silme (sonraya bırakıldı), WFD'nin belirli bir ağaca bağlanması
(ayrı, takip eden bir spec — bkz. Madde 6).

---

## 1. Mevcut durum

- `org.orgt` şeması zaten çoklu ağaca izin veriyor: `orgtnt_id` düz bir FK, tenant
  başına tekil olmayı zorlayan bir UNIQUE constraint yok (yalnızca index var).
  Bugün her tenant'ın fiilen tek ağacı var ama bu bir şema kısıtı değil, veri durumu.
- `org.u_orgu` (üyelik) ve `org.ur` (rol atama, `orgu_id` ile zaten UNIT-SCOPED) hiçbir
  şekilde `orgt_id`'ye bağlı değil — bir kullanıcı farklı ağaçlardaki farklı birimlere
  zaten atanabiliyor, roller zaten "kullanıcının bağlı olduğu unit scope'unda" atanıyor.
  Bu iki gereksinim **zaten karşılanıyor**, bu spec'te değişiklik gerekmiyor.
- `/org/orgtnt/{id}/actors` (Kullanıcı sekmesinin/atamaların kaynağı) zaten tenant-geneli
  — `orgt_id` filtresi yok — yani bir kullanıcının atamaları hangi ağaçta olursa olsun
  birlikte görünüyor.
- Motorun (traversal/yetkilendirme) "hangi ağaçta olduğu" bilgisine hiçbir zaman ihtiyacı
  yok: `crates/org/src/traversal/executor.rs` her zaman **anchor node'un kendi**
  `orgt_id`'sini `get_orgt_id` ile çözüyor. Yani "varsayılan ağaç" motor için bir anlam
  taşımıyor — bu onaylandı (Madde 2).
- Eksik olan: `is_default` kavramı, `org.orgt` için create/update endpoint'leri (yalnız
  `list_orgt_by_tenant` var), ve frontend'in tamamen tek-ağaç varsayımıyla yazılmış olması
  (`org-data.store.ts` tek bir `orgtId` tutuyor; `App.tsx` bootstrap `trees[0]`'ı seçiyor;
  `OrgExplorer`/`LeftPanel` hiçbir yerde ağaç listesi çekmiyor).

---

## 2. Karar: "Varsayılan" yalnızca UI kolaylığıdır

`is_default`, Organizations sayfası açıldığında hangi ağacın otomatik seçili geleceğini
belirler. Motor/yetkilendirme davranışına **hiçbir etkisi yoktur** — traversal her zaman
anchor'ın gerçek ağacını kullanır. Bu, tasarımı önemli ölçüde basitleştirir: `is_default`
salt bir bootstrap/UX alanıdır, engine tarafında okunmaz.

---

## 3. Veri modeli

```sql
ALTER TABLE org.orgt ADD COLUMN is_default boolean NOT NULL DEFAULT false;

-- Geriye dönük uyumluluk: her tenant'ın BUGÜN var olan tek ağacı varsayılan olsun.
UPDATE org.orgt o
SET is_default = true
WHERE o.orgt_id = (
    SELECT o2.orgt_id FROM org.orgt o2
    WHERE o2.orgtnt_id = o.orgtnt_id AND o2.is_active = true
    ORDER BY o2.created_at ASC LIMIT 1
);

-- Tenant başına en fazla bir varsayılan — DB seviyesinde garanti.
CREATE UNIQUE INDEX orgt_one_default_per_tenant
    ON org.orgt (orgtnt_id) WHERE is_default = true;
```

`set_default` bu yüzden **transaction içinde** çalışmalı: önce tenant'ın mevcut
varsayılanının `is_default`'ını false yapıp sonra hedefi true yapmalı (aksi halde
partial unique index ihlali oluşur — iki satır aynı anda `is_default=true` olamaz).

---

## 4. Backend

**`crates/org/src/models.rs`** — `Orgt` struct'ına `is_default: bool` eklenir.

**`crates/org/src/repo/orgt.rs`** — yeni fonksiyonlar (mevcut `list_by_tenant`,
`get_orgtnt_id` deseniyle aynı stil):

```rust
pub async fn create(
    pool: &PgPool, orgtnt_id: Uuid, name: &str, description: Option<&str>,
) -> Result<Orgt, OrgError>
```
- `name` boşsa `BadRequest`.
- Tenant'ın **hiç aktif ağacı yoksa** yeni ağaç otomatik `is_default = true` olur
  (ilk ağaç her zaman varsayılandır — böylece "bir tanesi default olacak" hiçbir zaman
  ihlal edilmez). Aksi halde `is_default = false`.

```rust
pub async fn update(
    pool: &PgPool, orgt_id: Uuid, name: &str, description: Option<&str>,
) -> Result<Orgt, OrgError>
```
- Yeniden adlandırma; `is_active`/`is_default`'a dokunmaz.

```rust
pub async fn set_default(pool: &PgPool, orgtnt_id: Uuid, orgt_id: Uuid) -> Result<Orgt, OrgError>
```
- Transaction: `UPDATE org.orgt SET is_default=false WHERE orgtnt_id=$1 AND is_default=true`,
  sonra `UPDATE org.orgt SET is_default=true WHERE orgt_id=$2 AND orgtnt_id=$1` (0 satır
  etkilenirse `NotFound` — orgt bu tenant'a ait değil demektir), sonra `get`.

**`crates/server/src/routes/org.rs`** — yeni route'lar:

```rust
#[derive(Deserialize, ToSchema)]
struct CreateOrgtBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(post, path = "/orgtnt/{id}/orgt", tag = "org", ...)]
async fn create_orgt(...) -> Result<Json<Orgt>, AppError>

#[utoipa::path(patch, path = "/orgt/{id}", tag = "org", ...)]
async fn update_orgt(...) -> Result<Json<Orgt>, AppError>  // body: CreateOrgtBody

#[utoipa::path(post, path = "/orgt/{id}/set-default", tag = "org", ...)]
async fn set_default_orgt(...) -> Result<Json<Orgt>, AppError>
```

`set_default_orgt`, path'teki `orgt_id`'den `orgtnt_id`'yi **mevcut** `orgt::get_orgtnt_id`
fonksiyonuyla çözer (orgu-crud işinde zaten eklenmişti — yeni bir fonksiyon gerekmiyor),
sonra `set_default(pool, orgtnt_id, orgt_id)` çağırır.

Router'da mevcut `.routes(routes!(list_orgt_by_tenant))` → `.routes(routes!(list_orgt_by_tenant, create_orgt))`
(aynı path, axum route-gruplama kuralı — bkz. orgu-crud spec'indeki aynı gotcha).
`update_orgt`/`set_default_orgt` farklı path'ler (`/orgt/{id}`, `/orgt/{id}/set-default`)
olduğundan ayrı `.routes()` çağrıları olabilir.

---

## 5. Frontend

**`src/api/engineApi.ts`**:
- `OrgTree` interface `is_default: boolean` alanı kazanır.
- Yeni fonksiyonlar: `createOrgTree`, `updateOrgTree`, `setDefaultOrgTree` (mevcut
  `createRole`/`updateRole` deseniyle aynı — `adminHeaders`, aynı imza şekli).

**`src/App.tsx` bootstrap** — `trees.find(t => t.orgt_id === stored.orgtId) ?? trees.find(t => t.is_default) ?? trees[0]`
(önce localStorage'daki son seçim, yoksa varsayılan, o da yoksa ilk ağaç).

**`OrgExplorer.tsx`**:
- Yeni state: `trees: OrgTree[]`, `listTrees(baseUrl, tenantId)` ile `loadOrgData`'ya
  paralel yüklenir (aynı `useEffect`'e eklenir).
- Yeni handler: `handleSwitchTree(orgtId)` → `useOrgDataStore`'un `setConfig` + `refresh`'ini
  çağırır (mevcut store zaten bunun için tasarlı — `setConfig` farklı `orgtId` verilince
  `loadedKey`'i sıfırlıyor).
- Yeni state: `treeManagerOpen: boolean` ("Ağaçlar" modalını açar).

**`LeftPanel.tsx` toolbar** — mevcut "Birim Tipleri" / "+ Yeni Ekle" butonlarının yanına:
- Bir `<select>` (ağaç switcher) — `trees` prop olarak gelir, `value=orgtId`,
  `onChange` → `onSwitchTree(orgtId)`. Varsayılan ağaç option metninde `" (varsayılan)"`
  soneki ile işaretlenir.
- Bir "Ağaçlar" butonu → `onOpenTreeManager()` (OrgExplorer'daki modalı açar).

**Yeni component `OrgTreeManagerModal.tsx`** — `OrguTypeManagerModal.tsx` ile **birebir aynı
iskelet** (ModalShell, liste + inline rename + "+ Yeni Ağaç" formu), farkı: her satırda
varsayılan olmayan ağaçlar için bir "Varsayılan yap" butonu (varsayılan olan satırda bunun
yerine sabit bir "Varsayılan" rozeti gösterilir, buton yok).

---

## 6. Kapsam dışı (bu spec'te YOK, ayrı takip eden spec'ler)

- Ağaç pasifleştirme/silme — kullanıcı "şimdilik gerek yok, sonra düşünürüz" dedi.
- WFD'nin ayarlarında hangi org ağacını kullandığının seçilmesi (+ buna bağlı validator
  ve "bu ağacı kullanan publish edilmiş WFD varsa silinemez" guard'ı) — ayrı, takip eden
  bir spec. Bu spec'in ürettiği `org.orgt`/`is_default` altyapısı o işin önkoşuludur.

---

## 7. Test planı

- Backend: `crates/org` repo testleri — `create` (ilk ağaç otomatik varsayılan olur,
  ikinci ağaç varsayılan olmaz), `set_default` (eski varsayılan false'a düşer, unique
  index ihlali oluşmaz), `update` (rename, is_default/is_active değişmez). `cargo test --workspace`.
- Backend manuel smoke test: gerçek dev DB'ye karşı curl ile create/rename/set-default
  (orgu-crud spec'indeki gibi — `psql`/docker yok, gerekirse aynı sqlx-example-binary
  yöntemi kullanılabilir).
- Frontend: `tsc --noEmit` + mevcut vitest suite; yeni saf mantık yok (switcher/modal
  state'i basit), bu yüzden yeni bir util testi gerekmiyor — canlı smoke test (headless
  chromium + gerçek backend) yeterli doğrulama.

---

## 8. Başarı kriterleri

- [ ] Migration uygulandı: `is_default` kolonu var, mevcut tenant'ların tek ağacı
  otomatik varsayılan işaretlendi, unique index kuruldu.
- [ ] Backend: create/update/set-default endpoint'leri çalışıyor, ilk ağaç otomatik
  varsayılan oluyor, set-default transaction'ı unique index'i asla ihlal etmiyor.
- [ ] Frontend: Organizations sayfasında ağaç switcher'ı çalışıyor (seçim değişince
  doğru ağacın birimleri yükleniyor), "Ağaçlar" modalından yeni ağaç oluşturulabiliyor,
  yeniden adlandırılabiliyor, varsayılan değiştirilebiliyor.
- [ ] `App.tsx` bootstrap artık tenant'ın varsayılan ağacını seçiyor (ilk ağaç yerine).
- [ ] `cargo test --workspace` ve frontend `vitest` suite yeşil.
