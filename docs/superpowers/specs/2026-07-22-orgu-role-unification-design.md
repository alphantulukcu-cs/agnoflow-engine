# Tasarım: `special` → unit-inherited `role`

**Tarih:** 2026-07-22
**Kapsam:** `crates/org`, `crates/wfe-core` (matcher), `crates/server` (/org), migrations/org, seed data
**Durum:** Onaylandı (brainstorming), plan bekliyor

## Problem

Organizasyon modelinde üç kavram var ama ikisi çakışıyor:

- `type` — birimin hiyerarşik tipi (şube, bölge). `orgu.orgu_type` JSONB'de skaler string.
- `special` — birimin ek nitelikleri (döviz, kredi). **Aynı** JSONB'de string array.
- `role` — kişinin yetkisi. Relational: `org.r` katalog + `org.ur` (kullanıcı, birim, rol) atama.

`special` ve `role` semantik olarak örtüşüyor ("döviz" hem birim niteliği hem kişi rolü olabilir) ama iki
ayrı mekanizmada yaşıyorlar. `type`/`special` "hangi birim?" sorusunu (`c_orgu` ekseni, ORGTRVLANG SQL),
`role` "birimdeki hangi kişi?" sorusunu (`c_r` ekseni, `wfe-core` matcher) yanıtlıyor.

## Karar

`special` kaldırılır. `role` tek kavram olur ve **hem kullanıcıya hem orgu'ya** bağlanabilir; orgu'ya
bağlanınca o orgudaki tüm kullanıcılar rolü **devralır**. Depolama **tam relational** (org.r katalogu tek
kaynak). `type` olduğu gibi kalır.

Bu yön, şemanın zaten öngördüğü niyetle uyumlu: `org.ur` tablosunda `orgu_scope` ve
`ur_type IN ('inherited','granted','excluded')` alanları hâlihazırda mevcut.

**Compat notu (doğrulandı):** Hiçbir yayınlanmış WFD veya spec `c_orgu` ifadesinde `[special:...]`
kullanmıyor. `special` yalnızca `crates/org` ve 2 seed SQL dosyasında geçiyor. Dolayısıyla `special`
selector'ının kaldırılması hiçbir mevcut workflow tanımını kırmaz.

## Değişiklikler

### 1. Şema — yeni `org.orgu_r` tablosu

```sql
CREATE TABLE org.orgu_r (
    orgu_r_id   uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid NOT NULL,
    orgu_id     uuid NOT NULL REFERENCES org.orgu(orgu_id),
    r_id        uuid NOT NULL REFERENCES org.r(r_id),
    ur_type     text NOT NULL DEFAULT 'granted'
                CHECK (ur_type IN ('inherited','granted')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgu_id, r_id)
);
CREATE INDEX orgu_r_orgu_idx ON org.orgu_r(orgu_id);
CREATE INDEX orgu_r_r_idx    ON org.orgu_r(r_id);
```

- `org.ur` (kullanıcı-rol) **dokunulmaz**. Role hem user'a (org.ur) hem orgu'ya (org.orgu_r) aynı
  `org.r` katalogundan bağlanır — tek kavram, tek katalog.
- `excluded` orgu düzeyinde anlamlı değil (bir birime rol "verilir"); exclusion kullanıcı düzeyinde
  `org.ur` üzerinden yürür (§2).

### 2. `check_user_role` — devralma + exclusion

`org.ur` yolu genişler; üç kaynak, exclusion kazanır:

```
role_var(user, orgu, role) =
   (   (org.ur granted : u_id=user, r_id=role, timeslice OK)
    OR (org.orgu_r     : orgu_id=user.orgu, r_id=role, timeslice OK) )
   AND NOT (org.ur excluded : u_id=user, r_id=role)
```
(Parantez bağlayıcı: önce iki grant kaynağının OR'u, sonra tüm sonucun exclusion ile AND NOT'u.)

- Orgu "döviz" grant'ı → tüm üyeler devralır.
- Bir üye `org.ur ur_type='excluded'` satırıyla bireysel olarak dışlanabilir (exclusion her iki
  kaynağı da ezer).
- C_A matcher'ın §3 `(c_r role OR c_u user)` şekli **değişmez** — yalnızca `check_user_role`
  gövdesi genişler (`crates/org/src/repo/user_role.rs`).

### 3. ORGTRVLANG — `[special:X]` çıkar, `[role:X]` girer

- Bare token / `[type:X]` → bugünkü JSONB predicate (`orgu_type`), **değişmez**.
- `[role:X]` → **relational**: `org.orgu_r` + `org.r` join'i olan birimleri seçer.
  `*:[role:doviz]` = tenant-geneli döviz birimleri.
- **Çoklu rol (array): `[role:doviz,kredi]`** → virgülle listelenen rollerden **herhangi birine**
  (OR) sahip birimleri seçer. `&&` (filtreler arası AND) grameriyle çakışmaz; virgül tek bir
  `role:` değerinin içinde çalışır. Örn. `[role:doviz,kredi && type:sube]` =
  "(döviz VEYA kredi) VE tip=şube".
- Semantik: `[role:X]` birimi **orgu-level grant'ı (org.orgu_r) varsa** seçer — eski `special`
  anlamının birebir karşılığı. Bir üyesinde rol olması birimi seçtirmez (o yetki eksenidir).
- Grammar: `crates/org/src/traversal/parser.rs` — `role` key'i tanınır; çoklu değer virgülle
  parse edilir. Executor: `crates/org/src/traversal/executor.rs` — `key == "role"` dalı JSONB
  predicate yerine `org.orgu_r` alt-sorgusu üretir. Global selektör `*:[role:X]` twin'i
  `crates/org/src/repo/user_role.rs` içinde.

### 4. Migration (veri + seed)

`migrations/org` altında yeni migration (psql ile manuel uygulanır — CLAUDE.md kuralı):

1. `org.orgu_r` tablosunu oluştur.
2. Her tenant için `orgu_type->'special'` içindeki distinct değerler → `org.r` kaydı (idempotent).
3. Her `special` içeren birim → ilgili `org.orgu_r` satır(lar)ı.
4. `UPDATE org.orgu SET orgu_type = orgu_type - 'special'` (JSONB'den `special` anahtarı silinir;
   `type` kalır).
5. Migration idempotent olmalı (tekrar çalıştırmaya güvenli).

Seed dosyaları yeni modele güncellenir: `data/seed_qnb_regions.sql`, `data/seed_qnb_users.sql`.

### 5. API (/org, X-Admin-Key)

Mevcut `assignments` paternine paralel (`crates/server/src/routes/org.rs`):

- `POST /org/orgtnt/:id/orgu-roles` — body `{orgu_id, role}` → orgu-level grant (idempotent).
- `DELETE /org/orgtnt/:id/orgu-roles/:orgu_id/:r_id` — revoke.

Repo: `crates/org/src/repo/` — `grant_orgu_role` / `revoke_orgu_role`.

### 6. Test

Zamana bağlı değil → `start_paused` gerekmez. Golden fixture (WFD) değişmez, `special` içermiyor.

- ORGTRVLANG: `[role:doviz]` yalnızca `org.orgu_r` grant'ı olan birimleri seçer.
- ORGTRVLANG: `[role:doviz,kredi]` OR semantiği (herhangi biri).
- ORGTRVLANG: `[role:doviz,kredi && type:sube]` bileşimi.
- `check_user_role`: orgu-inherited yol true döner.
- `check_user_role`: `org.ur excluded` satırı devralınan rolü ezer.
- matcher: `c_r` yalnızca orgu-inherited rolle karşılanabilir (bireysel org.ur olmadan).
- migration idempotent (iki kez çalıştır → aynı sonuç).
- `cargo test --workspace`.

## Kapsam dışı (YAGNI)

- `[role:X]`'in "üyesinde rol olan birim" semantiği (pahalı, istenmedi).
- `org.orgu_r` için delegation (org.ur delegation'ı yeterli).
- `special` selector için geriye-dönük alias (hiçbir WFD kullanmıyor — gereksiz).
