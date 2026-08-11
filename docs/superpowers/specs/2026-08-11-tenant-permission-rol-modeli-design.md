# Tenant Permission Havuzu + Rol = Permission Grubu (T‑A1, T‑A2)

**Tarih:** 2026-08-11
**Kapsam:** `agnoflow-engine` (org crate + server). Yönetim ekranları AYRI spec (`agnoflow-frontend`).
**Görevlendirme:** T‑A1 ("Profil = Rol"), T‑A2 (rol içi `except`).

## 1. Amaç ve duruş

agnoflow, tenant'ın **merkezi yetki dizini** oluyor. Tenant kendi **atomik** iş
yetkilerini (permission) bir havuzda tanımlar; rol bu yetkilerin **grubudur**;
kullanıcı rollerle yetki alır; kişi bazında istisna tanınabilir.

**agnoflow permission'ın ANLAMINI bilmez.** "1043" ya da `KREDI_ONAY` neyi açar —
bunu tenant'ın kendi uygulaması bilir. Motor bu veriyi yorumlamaz, sadece saklar,
dağıtır ve sorulunca cevaplar. Bu, `$env` secret'larındaki "kullanılabilir ama
yorumlanmaz" duruşunun aynısıdır.

Bunlar permission DEĞİLDİR ve bu spec'in dışındadır:

- **agnoflow'un kendi yetenekleri** (akış yayınla, tüm instance'ları gör, atama yap).
  Görevlendirmedeki T‑A4 / T‑A5 / T‑A6 bunlardır; tenant havuzu onları karşılamaz,
  ayrı spec ister.
- **WFD node kapıları.** `c_a` / `c_r` modeline dokunulmaz (§7).

**Okuyanlar:** (1) tenant'ın dış uygulamaları (API anahtarıyla), (2) portal
ekranları (kullanıcının kendi kümesi).

## 2. T‑A1 kararı: yeni katman YOK

"Profil" Rol'ün eş anlamlısıdır. `org.r` **tek katalogdur** ve iki işi birden yapar:

1. motorun `c_a.c_r` rol kanalı (bugünkü davranış, değişmiyor),
2. permission grubu (bu spec'in eklediği anlam).

Rol üstüne yeni bir "profil" katmanı eklenmiyor. Dokümanlarda kalan "profil"
terimi Rol'e çevrilir (T‑E1 temizliğine not).

## 3. Veri modeli

Üç tablo; hepsi mevcut `org.r` / `org.ur` desenini izler (UUID PK + `orgtnt_id` +
`is_active` + timeslice).

```sql
-- Havuz: tenant'ın atomik yetkileri. code numara da olabilir ("1043"), isim de.
CREATE TABLE org.p (
    p_id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id    uuid NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    code         text NOT NULL,
    display_name text NOT NULL,
    description  text,
    is_active    boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT p_code_format CHECK (code ~ '^[A-Za-z0-9._:-]{1,128}$')
);
CREATE UNIQUE INDEX p_code_unique ON org.p (orgtnt_id, lower(code));
CREATE INDEX p_orgtnt_idx ON org.p(orgtnt_id);

-- Rol = permission grubu.
CREATE TABLE org.rp (
    rp_id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id  uuid NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    r_id       uuid NOT NULL REFERENCES org.r(r_id),
    p_id       uuid NOT NULL REFERENCES org.p(p_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (r_id, p_id)
);
CREATE INDEX rp_r_idx ON org.rp(r_id);
CREATE INDEX rp_p_idx ON org.rp(p_id);

-- T‑A2: kişisel ıskarta. "Ahmet memur ama onda 1043 olmasın."
CREATE TABLE org.up (
    up_id       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id   uuid NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    u_id        uuid NOT NULL REFERENCES org.u(u_id),
    p_id        uuid NOT NULL REFERENCES org.p(p_id),
    up_type     text NOT NULL DEFAULT 'excluded' CHECK (up_type IN ('excluded')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (u_id, p_id, up_type)
);
CREATE INDEX up_u_idx ON org.up(u_id);
CREATE INDEX up_p_idx ON org.up(p_id);
```

### 3.1 Kararlar

**`code` yeniden adlandırılabilir, `p_id` değil.** Atamalar `p_id`'ye bağlıdır —
tenant "1043"ü `KREDI_ONAY`a çevirdiğinde kimse yetki kaybetmez. `org.r`'de `r_id`
ile aynı gerekçe (`repo::user_role::update_role` yorumu).

**`code` ASCII harf/rakam + `. _ : -` ile sınırlı; benzersizlik büyük/küçük harf
DUYARSIZ.** `code` bir MAKİNE kimliğidir, gösterim metni değil — o `display_name`'de
ve serbesttir. İki gerekçe:

- Boşluk yasak: dış uygulama permission listesini boşluk/virgülle ayırıp bölebilir,
  içinde boşluk olan kod sessizce ikiye ayrılırdı.
- Türkçe harf yasak: benzersizlik `lower(code)` üzerindedir ve PostgreSQL'in
  `lower()`'ı ile Rust'ın `to_lowercase()`'i **`İ` üzerinde ayrışır** (libc noktayı
  düşürür, Rust birleştirici nokta bırakır). Havuzda benzersiz sayılan iki kod,
  `check` karşılaştırmasında farklı görünürdü. Alfabe ASCII'ye kapatıldığı için
  `fold_code` = `to_ascii_lowercase` ile `lower()` aynı cevabı verir.

Kod yazıldığı gibi saklanır; tüm karşılaştırmalar katlanmış biçim üzerinden yapılır
(unique index de, `check` ucu da).

**`up_type` CHECK'i şimdilik yalnız `'excluded'`.** Tablo şekli `org.ur`'nin
aynısıdır (ileride kişiye doğrudan grant istenirse yer var) ama tasarlanmamış
semantiği DB kabul etmez. Timeslice var: istisnalar tipik olarak geçicidir ("bu ay
onaylamasın") ve etkin küme hesabı zaten timeslice uygular — sonradan eklemek
API + UI + hesap üçlüsünü birden değiştirmek olurdu.

**Kişisel ıskarta bir permission'ı kullanıcıdan TAMAMEN kaldırır**, hangi rolden
geldiğine bakmaz. Rol bazlı ıskarta ("memur'dan geleni düş, şef'ten geleni bırak")
tasarlanmadı: kullanıcı görünümünde yetkinin kaynağı değil varlığı önemlidir, ve
iki rolden gelen aynı yetkiyi kısmen kaldırmak ekrandaki sonucu açıklanamaz kılar.

## 4. Etkin küme

```
birimler(u)   = org.u_orgu satırları
etkin_rol(u)  = { r : ∃ b ∈ birimler(u) → check_user_role(u, b, r) }
etkin_p(u)    = ⋃ rp(etkin_rol(u))  −  up_excluded(u)
```

Mevcut `check_user_role` semantiği **birim başına aynen korunur**, sonra
kullanıcının tüm birimleri üzerinde birleştirilir: bir rol, kullanıcının **en az
bir** biriminde etkinse permission'larını verir. Yani `org.ur` doğrudan grant'ı VEYA
`org.orgu_r` birim devralması; o birimdeki `ur_type='excluded'` satırı ikisini de
ezer; `r.is_active` ve `p.is_active` süzer.

Türetilmiş kurallar:

- Birim A'daki `excluded`, birim B'deki grant'ı **ezmez** (kapsam birimdir).
- **`org.ur`'daki rol ıskartasına timeslice UYGULANMAZ**: süresi geçmiş bir
  `excluded` satırı rolü yine kapatır. `check_user_role`'ün son `NOT EXISTS`'i de
  öyle davranıyor ([user_role.rs:186](crates/org/src/repo/user_role.rs#L186)) —
  motorla aynı cevabı vermek, "portal yetki veriyor ama node açılmıyor"
  çelişkisinden daha önemli. `org.up` kişisel ıskartası bundan AYRIDIR: orada
  timeslice geçerlidir (§3.1'de bilinçli tasarlandı).
- `org.ur.orgu_id IS NULL` satırı yetki **ÜRETMEZ**. `check_user_role` birim
  eşitliği ister; bu satırlar bugün motorda hiçbir kapı açmıyor. Burada "tenant
  geneli grant" saymak, kimsenin niyet etmediği yetkileri sessizce dağıtırdı.
- `org.ur.orgu_scope` **okunmaz** — `check_user_role` da okumuyor.
- (Bu iki kolon şemada duran ama kimsenin okumadığı alanlardır; T‑E1 temizlik notu.)

### 4.1 Hesap SQL'de değil, saf Rust'ta

SQL yalnız **ham satırları** çeker; birleşim/ıskarta kuralını saf bir fonksiyon
uygular:

```rust
// crates/org/src/repo/permission.rs
pub struct PermissionRows {
    pub ur: Vec<UrRow>,          // u_id'nin ur satırları (orgu_id, r_id, ur_type, timeslice, role_is_active)
    pub orgu_r: Vec<OrguRRow>,   // u_id'nin ÜYE olduğu birimlerin orgu_r satırları (+ role_is_active)
    pub rp: Vec<RpRow>,          // ilgili rollerin permission'ları
    pub up: Vec<UpRow>,          // u_id'nin ıskartaları (timeslice dahil)
    pub perms: Vec<Permission>,  // p katalog satırları — is_active DAHİL, süzülmemiş
}

pub fn effective_permissions(
    rows: &PermissionRows,
    now: DateTime<Utc>,
) -> Vec<EffectivePermission>;
```

Gerekçe: bu repoda yerel psql yok, DB'li test koşulmuyor. Kural SQL içinde üç
`NOT EXISTS` olarak yaşarsa modelin en kolay yanlış yazılacak iki davranışı ("en az
bir birimde etkinse sayılır", "A'daki `excluded` B'yi ezmez") **hiç test edilemez**.
Saf fonksiyon `cargo test --workspace` içinde tablo-güdümlü test alır. Maliyet:
kullanıcının rol sayısıyla sınırlı (onlarca) fazladan satır — tenant büyüklüğüyle
değil.

**SQL süzmez, saf fonksiyon süzer.** `is_active` ve timeslice kontrolleri SQL
`WHERE`'ine kaçarsa o kurallar yine test dışına düşer — yani bölmenin amacı boşa
gider. SQL'in tek işi `u_id` (ve onun birimleri) kapsamındaki satırları getirmektir;
her karar fonksiyonda verilir.

`now` parametre olarak geçer (`Utc::now()` içeride çağrılmaz) ki timeslice testleri
sabit zamanla koşsun.

### 4.2 Provenance baştan var

`EffectivePermission { code, display_name, via_roles: Vec<String> }`.

"Ahmet neden 1043'e sahip?" yönetim ekranının ilk sorusudur ve ıskarta koymadan
önce cevabı gerekir. `AuthDecision::Delegated`'ın vekâlet provenance'ı taşımasıyla
aynı gerekçe: yetki kararı denetlenebilir olmalı.

## 5. API yüzeyi

Rota adlandırması mevcut geleneği izler: tablo kısa (`org.r` → `/roles`), yol açık
(`org.p` → `/permissions`).

### 5.1 Yönetim — `/org` ağacı, X‑Admin-Key

| Uç | İş |
|---|---|
| `GET /org/orgtnt/{id}/permissions` | Havuzu listele (arama + sayfalama) |
| `POST /org/orgtnt/{id}/permissions` | Yeni permission |
| `PATCH /org/orgtnt/{id}/permissions/{pid}` | `code` / `display_name` / `description` / `is_active` |
| `DELETE /org/orgtnt/{id}/permissions/{pid}` | Kullanımdaysa 409, değilse sil |
| `GET /org/orgtnt/{id}/roles/{rid}/permissions` | Rolün permission kümesi |
| `PUT /org/orgtnt/{id}/roles/{rid}/permissions` | Rolün kümesini topluca ayarla |
| `GET /org/orgtnt/{id}/permissions/{pid}/roles` | Ters sorgu: hangi rollerde |
| `GET /org/users/{id}/permissions` | Etkin küme + `via_roles` + ıskartalar |
| `GET /org/users/{id}/permission-exceptions` | Kişisel ıskartalar |
| `PUT /org/users/{id}/permission-exceptions` | Iskarta kümesini ayarla |

`PATCH` semantiği `PATCH /org/orgtnt/{id}` ile aynıdır: **alan gönderilmezse
değişmez, boş string temizler** (`description` için NULL). `code` ve
`display_name` boş gönderilirse 400.

### 5.2 Dış uygulama — `/ext` ağacı, tenant API anahtarı, salt okuma

| Uç | İş |
|---|---|
| `POST /ext/permissions/check` | `{u_id \| username, codes:[…]}` → `{granted:[…], denied:[…], unknown:[…]}` |
| `GET /ext/permissions/user/{u_id}` | Etkin küme (kod + display_name + `via_roles`) |

**Bu uçlar `/org` altında OLAMAZ.** `main.rs` tüm `/org` (ve `/db`) ağacını tek bir
X‑Admin-Key middleware'inin arkasına koyuyor; oraya eklenen bir yol X‑Api-Key ile
erişilemez. Bu yüzden ayrı bir üst ağaç (`/ext`) ve ayrı router: kapı `X-Api-Key`,
yetki salt okuma — yapı gereği, bayrakla değil.

`check` **toplu**dur: dış uygulama tek ekranda onlarca yetki sorar, tek tek uç N+1
üretirdi. Bilinmeyen `code` `denied`a düşer (hata değil) — tenant henüz tanımlamamış
olabilir; ama `unknown` alanı hangi kodların havuzda hiç olmadığını bildirir, böylece
yazım hatası sessiz kalmaz. Bir kod hem `denied` hem `unknown` içinde görünür:
`denied` yetki cevabıdır, `unknown` teşhis.

`GET /ext/permissions/user/{u_id}` ile `GET /org/users/{id}/permissions` aynı etkin
kümeyi döner; fark kapı ve içeriktir — `/org` sürümü ıskartaları da listeler (yönetim
ekranı için), `/ext` sürümü yalnız sonucu (dış uygulamaya iç yönetim detayı sızmaz).

Anahtar TEK tenant'a bağlıdır; `u_id`/`username` o tenant dışındaysa `404`
(varlığı sızmaz).

### 5.3 Portal — JWT

| Uç | İş |
|---|---|
| `GET /portal/me/permissions` | Yalnız **kendi** etkin kümesi |

Başkasının kümesi portal ağacından **okunamaz**.

### 5.4 Kararlar

**Küme uçları `PUT`, tek tek `POST`/`DELETE` değil.** Yönetim ekranı "kutucukları
işaretle → kaydet" akışıdır; tek tek çağrı iki yöneticinin aynı rolü düzenlemesinde
yarış üretir ve yarım uygulanmış küme bırakır. `PUT` diff'i tek transaction'da
uygular — pipeline'ın "tüm diff'ler staged, tek commit" duruşuyla aynı.

**Silme yerine `is_active`.** Kullanımdaki bir permission'ı silmek, dış uygulamanın
bir gün `granted:true` alıp ertesi gün sessizce `false` almasıdır. Kullanımdaysa
409 (`permission.in_use`); kullanımda değilse gerçek silme, havuz temiz kalsın.

**Permission JWT'ye GÖMÜLMEZ.** Portal her sorduğunda DB'den okunur. JWT TTL
saatlercedir — token bugün alınan yetkiyi yarına kadar taşırdı ve ıskarta koymanın
hiçbir etkisi olmazdı.

## 6. Tenant API anahtarı

```sql
CREATE TABLE org.orgtnt_api_key (
    key_id       uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id    uuid NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    name         text NOT NULL,          -- "SET entegrasyonu"
    prefix       text NOT NULL UNIQUE,   -- lookup anahtarı
    key_hash     text NOT NULL,          -- SHA-256 hex
    is_active    boolean NOT NULL DEFAULT true,
    expires_at   timestamptz,
    last_used_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX orgtnt_api_key_orgtnt_idx ON org.orgtnt_api_key(orgtnt_id);
```

- Biçim: `agp_<prefix:8>_<secret:32>`; `X-Api-Key` başlığıyla gönderilir.
- Düz metin **yalnız yaratılışta bir kez** döner; DB'de yalnız hash durur.
- Aynı tenant'ta birden çok aktif satır olabilir → **rotasyon** yeni anahtarı
  ekleyip eskisini `is_active=false` yapmakla olur (`DB_CONN_SECRET`'ın virgüllü
  liste yaklaşımıyla aynı mantık).
- Yönetim: `GET/POST /org/orgtnt/{id}/api-keys`, `DELETE …/api-keys/{key_id}`
  (X‑Admin-Key arkasında).
- Extractor `crates/server/src/api_key.rs`: `TenantApiKey { orgtnt_id, key_id }`.
  `prefix` ile satır çekilir, hash sabit-zamanlı karşılaştırılır, `expires_at` ve
  `is_active` süzülür. Geçersiz → `401 api_key.invalid`.

**Hash SHA-256, bcrypt değil.** Anahtar 256-bit rastgeledir; sözlük/brute-force
yüzeyi yok. `check` ucu her dış ekranda çağrılacak ve bcrypt (cost 12) her isteğe
~100 ms eklerdi. bcrypt kullanıcı **şifresinde** kalır — düşük entropi, farklı
tehdit modeli.

**Neden küresel `ADMIN_API_KEY` değil:** o anahtar TÜM tenant'larda tam org YAZMA
yetkisi verir, ve staging'de tanımsız olduğu için uç tamamen açık olurdu.
`X-Api-Key` tek tenant + salt okuma ile sınırlıdır.

`last_used_at` best-effort güncellenir (her istekte yazma değil; gün bazında
tazelenir) — amaç kullanılmayan anahtarı fark etmek, kesin denetim izi değil.

## 7. Motor bu katmandan habersizdir

`wfe-core`'a tek satır girmez:

- Permission `$ctx`'e girmez, `$wfah`'a girmez, `$p` diye bir ZEN namespace'i
  YOKTUR.
- `c_a`'da permission kanalı yoktur; `c_orgu` / `c_r` / `c_u` tek kural modeli
  değişmez.
- `docs/spec/schema.json` değişmez; golden fixture (`kredi-basvuru.golden.json`)
  etkilenmez.
- Yayınlanmış akışların davranışı **değişmez**.

WFE not defterindeki K1 duruşunun aynısıdır: akışın kararını etkileyen her şey hâlâ
WFD `actions[].input` → `wfes_effects` → `$ctx` yolundan gider.

## 8. Kod yerleşimi

| Dosya | İş |
|---|---|
| `migrations/org/20260811000001_permission.sql` | `org.p` / `org.rp` / `org.up` / `org.orgtnt_api_key` (idempotent, psql ile manuel) |
| `crates/org/src/permission.rs` | **SAF** çekirdek: satır tipleri, `effective_permissions`, `check_codes` + testler |
| `crates/org/src/repo/permission.rs` | I/O: satır çekme, küme `PUT` transaction'ları, API anahtarı sorguları |
| `crates/org/src/models.rs` | `Permission`, `PermissionRoleUsage`, `PermissionException`, `TenantApiKey` |
| `crates/org/src/error.rs` | `OrgError::Conflict(String)` — 409 sınıfı (metin makine kodudur) |
| `crates/server/src/routes/permissions.rs` | Yönetim uçları; `/org` ağacına merge (`org_branding.rs` deseni) |
| `crates/server/src/routes/ext_permissions.rs` | `/ext` ağacı — X‑Api-Key kapısı, salt okuma |
| `crates/server/src/routes/portal/permissions.rs` | İnce kabuk, ortak mantık paylaşılır (`notes.rs` / `portal/notes.rs` ikilisi gibi) |
| `crates/server/src/api_key.rs` | `TenantApiKey` extractor |
| `crates/server/src/main.rs` | `/ext` router'ını mount et (X‑Admin-Key middleware'inin DIŞINDA) |

Üç kabuk (`/org`, `/ext`, `/portal`) aynı `repo::permission` fonksiyonlarını çağırır;
etkin küme mantığı tek yerde durur.

`user_role.rs` (450+ satır) şişmesin diye permission ayrı modüldedir. Saf mantık
`repo/`'nun DIŞINDA yaşar: `repo/` I/O'nun yeri, `wfe-core`'un "I/O YOK" ayrımının
org katmanındaki karşılığı.

`/org/users/{id}/...` yolları tenant taşımaz (mevcut `/org/users/{id}/roles` ile
aynı biçim); kapsam `repo::permission::tenant_of_user` ile kullanıcı satırından
çözülür, böylece alt sorgular yine `orgtnt_id` ile bağlanır.

## 9. Hata kodları

| Kod | HTTP | Durum |
|---|---|---|
| `permission.code_format` | 400 | Alfabe dışı karakter / boş / 128+ karakter kod |
| `permission.code_conflict` | 409 | Aynı tenant'ta (harf duyarsız) aynı kod |
| `permission.not_found` | 404 | Kapsam dışı veya olmayan `p_id` |
| `permission.in_use` | 409 | Rol/ıskarta referansı olan permission silinmek istendi |
| `api_key.invalid` | 401 | Bilinmeyen / süresi geçmiş / kapalı anahtar |

DB kısıt ihlalleri kısıt **ADINDAN** çevrilir (mevcut `error.rs` deseni; SQL metni
sızmaz).

## 10. Test stratejisi

**Saf birim testleri** (`crates/org`, DB'siz — asıl güvence buradadır):

1. Rol tek birimde etkin → permission'ları etkin kümede.
2. Birim A'da `excluded`, birim B'de grant → permission **VAR** (kapsam birimdir).
3. Tek birimde grant + aynı birimde `excluded` → permission YOK.
4. `orgu_r` birim devralması → permission etkin.
5. `org.ur.orgu_id IS NULL` satırı → permission ÜRETMEZ.
6. `orgu_scope` dolu ama `orgu_id` NULL → yine üretmez.
7. Süresi geçmiş `ur` / `orgu_r` → sayılmaz.
8. `p.is_active=false` → etkin kümede yok.
9. `r.is_active=false` → o rolün permission'ları yok.
10. Kişisel ıskarta iki rolden gelen aynı permission'ı **tamamen** kaldırır.
11. Süresi geçmiş ıskarta → permission geri gelir.
12. İki rol aynı permission'ı verir → `via_roles` İKİSİNİ de listeler, kod tek satır.

Ek olarak `check_codes` (harf duyarsızlık, tekrar, sıra korunumu, `unknown` ayrımı)
ve `api_key` (üret→ayrıştır→doğrula turu, yabancı şema, bozuk/yanlış uzunluk, özet
determinizmi) saf testleri.

**DB gerektiren yollar** (rota kabukları, transaction'lar, kapsam kapıları) birim
testiyle kapatılamaz — bu repoda DB'li test koşulmuyor. Onlar canlı duman testiyle
doğrulandı (§10.1).

`cargo test --workspace` her değişiklikten sonra koşar.

### 10.1 Canlı doğrulama (2026-08-11, dev DB)

Uygulama sonrası gerçek sunucu + gerçek Postgres ile koşulan ve GEÇEN senaryolar:

| Senaryo | Sonuç |
|---|---|
| Havuza yetki ekleme (`1043`, `MUSTERI.GORUNTULE`) | 200 |
| Boşluklu kod / Türkçe `İ` içeren kod | 400 `permission.code_format` |
| Aynı kodun farklı harf biçimi | 409 `permission.code_conflict` |
| Rolün kümesini `PUT` ile ayarlama | 200, küme birebir |
| Kullanıcının etkin kümesi + `via_roles` | 2 yetki, `via_roles: ["mudur"]` |
| Ters sorgu: yetki hangi rollerde | `mudur`, `user_count: 6` |
| Kişisel ıskarta (T‑A2) sonrası etkin küme | ıskartalı yetki DÜŞTÜ |
| Kullanımdaki yetkiyi silme | 409 `permission.in_use` |
| `/ext/permissions/check` anahtarsız | 401 `api_key.invalid` |
| `/ext/permissions/check` anahtarlı | `granted:[musteri.goruntule]`, `denied:[1043,1O43]`, `unknown:[1O43]` |
| Başka tenant'ın kullanıcısı (`u_id` ve `username` ile) | 404, iki yoldan da |
| Kapatılmış (`is_active=false`) anahtar | 401 `api_key.invalid` |
| `GET /portal/me/permissions` token'sız | 401 (mount doğrulaması) |

Duman testinde yaratılan veri (yetkiler, atamalar, ıskarta, anahtarlar, geçici
tenant) sonrasında geri alındı.

## 11. Reddedilen alternatifler

**`org.r` üzerinde `permissions jsonb` dizisi.** Bir kolon, iki migration yerine
bir tanesi. Reddedildi: havuz kavramı kaybolur — tenant "1043" mü "1034" mü yazdı
belli olmaz (FK yok), silinen permission rollerde hayalet kalır, "bu yetki kimlerde
var" ters sorgusu GIN taramasına döner. Kullanıcının açıkça istediği **havuz**
fikrini karşılamıyor.

**Kapsamlı tam RBAC** (permission'a `orgu_scope` + rol kalıtımı). Reddedildi:
etkin küme tenant genelinde birleşim olarak seçildi, kalıtım yerine kişisel istisna
seçildi. İkisini şimdi yapmak kullanılmayan iki eksen bakımı demek. Kapı kapanmadı —
kapsam ileride `rp` / `up` satırına kolon olarak eklenebilir.

**Rolde `deny` + birleşimde deny'in kazanması.** Reddedildi: rol EKLEMEK yetki
kaybettirebilirdi; yöneticinin ekranda gördüğü sonuç açıklanamaz hale gelir.
Yasaklama ihtiyacı kişisel ıskarta ile karşılanıyor.

**Rol kalıtımından `except`** (`sef = memur + {onay} except {silme}`). Reddedildi:
rol kompozisyonu gerektirir ve iki mekanizma (rol içi çıkarma + kişisel istisna)
aynı ihtiyaca hizmet eder. Kişisel istisna seçildi çünkü gerçek dünyadaki istisna
kişiye özeldir.

**Permission'ı agnoflow yeteneklerine eşlemek** (yetenek→permission map). Kapsam
dışı: bu havuz tenant'ın İŞ yetkileridir, agnoflow'un kendi yetkileri
(T‑A4/A5/A6) ayrı bir eksendir ve ayrı spec ister.

**Küresel `ADMIN_API_KEY` ile dış erişim.** Reddedildi: §6.

## 12. Bu spec'in dışı

- **Yönetim ekranları** (`agnoflow-frontend`): Ayarlar > Yetkiler havuzu, rol
  detayında permission kutucukları, kullanıcı detayında etkin küme + ıskarta.
  Ayrı spec; öncelik engine tarafı.
- **T‑A4 / T‑A5 / T‑A6** — agnoflow'un kendi yetkileri (WFD‑Observer,
  WF Admin, doğrudan claim ettirme). Ayrı spec.
- **T‑E1 notları:** "profil" teriminin temizliği; `org.ur.orgu_id IS NULL` ve
  `org.ur.orgu_scope` ölü kolonları.
