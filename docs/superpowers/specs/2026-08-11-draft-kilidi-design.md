# WFD taslak kilidi — pessimistic (T‑B4)

**Tarih:** 2026-08-11
**Kapsam:** `agnoflow-engine` (wfd crate + server) + `agnoflow-frontend` (editör).
**Görevlendirme:** T‑B4 ("Draft kilidi — aynı draft iki kişide açık olmasın").

## 1. Sorun

Taslak = `wf.wfd_meta` satırı (`status='draft'`); `(project_id, name)` başına tek açık
taslak vardır. Ama `save_draft`'ta **hiçbir eşzamanlılık denetimi yok** — son yazan
kazanır. İki tasarımcı aynı taslağı açıp yarım saat çalışırsa ikinci kaydeden birincinin
emeğini sessizce siler.

Çözüm **pessimistic** kilit: iş başlamadan önce sahiplik alınır, çakışma kaydetme anında
değil açılışta görünür.

## 2. Veri modeli

Kilit `wf.wfd_meta`'ya üç kolon olarak gelir — ayrı tablo DEĞİL, çünkü kilit taslakla
1:1 ve taslak zaten o satırdır (join yok, yazma yolu tek).

```sql
ALTER TABLE wf.wfd_meta
  ADD COLUMN lock_user_id     uuid,
  ADD COLUMN lock_acquired_at timestamptz,
  ADD COLUMN lock_expires_at  timestamptz;
```

### 2.1 Alma ve tazeleme AYNI ifadedir

Tek `UPDATE`; `WHERE` cümlesi CAS görevi yapar, ayrı bir "önce oku sonra yaz" adımı yok:

```sql
UPDATE wf.wfd_meta
SET lock_user_id = $4,
    lock_acquired_at = COALESCE(lock_acquired_at, now()),
    lock_expires_at = now() + interval '5 minutes'
WHERE wfd_id = $1 AND version = $2 AND orgtnt_id = $3
  AND status = 'draft'
  AND (lock_user_id IS NULL OR lock_user_id = $4 OR lock_expires_at <= now())
RETURNING lock_user_id, lock_acquired_at, lock_expires_at
```

Sıfır satır → başkasında CANLI kilit var → `409 draft.locked` + sahibi + `lock_expires_at`.
Kullanıcı ne zaman serbest kalacağını görür.

### 2.2 Kararlar

**Süresi geçmiş kilit yok sayılır, SİLİNMEZ.** `lock_expires_at <= now()` koşulu onu
zaten geçirir; ayrı süpürücü yazılmaz (kolonlar `wf.wfe_reservation`'ın aksine yer
tutmuyor, satır zaten var). Kolonlar son sahibin izini taşımaya devam eder — "5 dakika
önce kimdeydi" sorusu destek için değerli.

**`lock_acquired_at` tazelemede DEĞİŞMEZ** (`COALESCE`): "bu kişi bu taslağı ne zamandır
tutuyor" ancak böyle cevaplanır.

**Yalnız `status='draft'` kilitlenir.** `pending_approval` düzenlenemez (status kapıları
korur), `published` immutable — ikisinde kilit anlamsız olurdu.

**TTL 5 dakika.** Kısa tutulur çünkü yenileme insan eylemine bağlıdır (§4); uzun TTL
gözetimsiz sekmenin rehin süresini uzatırdı.

## 3. Uçlar ve kapılar

| Uç | İş |
|---|---|
| `POST /wfd/draft/{id}/{ver}/lock` | Al **veya** tazele → `{lock_expires_at, lock_acquired_at, holder}` |
| `DELETE /wfd/draft/{id}/{ver}/lock` | Bırak — **yalnız sahibi**; başkası çağırırsa `409 draft.locked` (kilit sessizce düşmesin) |
| `GET /wfd/draft/{id}/{ver}` | Yanıta kilit durumu eklenir — editör salt-okunur açabilsin |

Kapılar:

```
PUT    /wfd/draft/{id}/{ver}        → kilit ZORUNLU
POST   /wfd/draft/{id}/{ver}/publish → kilit ZORUNLU
POST   /wfd/draft/{id}/{ver}/submit  → kilit ZORUNLU
DELETE /wfd/draft/{id}/{ver}        → kilit ZORUNLU
POST   .../approve | .../reject      → kilit YOK
```

**Neden tüm mutasyonlar, yalnız kaydetme değil:** A kilidi tutup düzenlerken B
yayınlarsa A'nın YARIM işi yayına çıkar. Kaydetmeyi korumak tek başına yetmez.
Yayınlamak isteyen kilidi alır; başkasındaysa `409` → devir açık bir el değiştirme olur.

**Neden onay/ret kilit istemez:** `pending_approval` satır düzenlenemez, dolayısıyla
onaycının korunacak bir şeyi yoktur. Kilit istemek onaycıyı tasarımcının kilidine bağımlı
kılardı.

### 3.1 Kapı, kontrol-sonra-yaz DEĞİL

Kilit koşulu ayrı bir `SELECT` ile değil, mutasyonun kendi `WHERE` cümlesine eklenir.
Repoda bu desen zaten var: `update_draft` `status='draft'` kapısını `WHERE`'de tutuyor ve
storage yazımı ondan SONRA geliyor ([adapter.rs:277](crates/wfd/src/adapter.rs#L277)) —
"eşzamanlı bir publish araya girerse immutable JSON'a dokunmayız". Kilit aynı hattan
gider: `UPDATE ... WHERE ... AND lock_user_id = $user AND lock_expires_at > now()`; sıfır
satır → 409 ve storage'a HİÇ dokunulmaz.

Aynı `UPDATE` `lock_expires_at`'i de ileri atar — **kaydetmek tazelemektir.** Çalışan
tasarımcının ayrıca "devam et" demesi gerekmez; kaydeden kişi zaten oradadır.

## 4. İstemci protokolü

```
açılış:  POST .../lock → 200 ise düzenleme, 409 ise SALT OKUNUR ("Ahmet'te, 14:32'ye kadar")
T-60s    popup → [Devam et] [Kaydet] [Kaydetmeden çık]      (5 dk TTL'de 4:00)
         · Devam et        → POST .../lock (tazeler)
         · Kaydet          → PUT save (kaydetme de tazeler)
         · Kaydetmeden çık → DELETE .../lock + editörden çık
T-30s    cevap YOK → önce PUT save, SONRA DELETE .../lock → salt okunur   (4:30)
T-0s     kilit zaten serbest; sunucu tarafı da süresini geçmiş sayar      (5:00)
```

**Popup `T-60s`'de çıkar, `T-30s`'de karar verilir — `T-0`'da DEĞİL.** Bu 30 saniyelik
pay zorunludur: otomatik kaydetme tam bitiş anında koşarsa kilit o an düşmüş olabilir ve
`PUT` `409` alır — yani emeği kurtarmak için eklenen mekanizma tam kurtaracağı anda
başarısız olur. Ağ gecikmesi de bu paya sığar.

**Zamanlayıcı sunucunun `lock_expires_at`'inden türetilir**, istemcinin kendi 5
dakikasından değil: saat kayması ya da yavaş ağ yüzünden popup çıkmadan kilidin düşmesi
böyle engellenir. Sunucu istemciye ASLA güvenmez; TTL SQL'de zorlanır, popup yalnız
kullanıcıyı sürprize karşı korur.

**Yenileme yalnız İNSAN eylemiyle olur** — kör zamanlayıcı YOKTUR. Bu, gözetimsiz
sekmenin taslağı her koşulda bırakmasını garanti eder; klasik "açık ama idle sekme
taslağı rehin alır" deliği böyle kapanır.

**Zaman aşımında ÖNCE kaydedilir, sonra bırakılır.** Taslak tanımı gereği yarım iştir
(doğrulanmamış, yayınlanmamış) — yarım kaydetmenin zararı yok, yarım saatlik emeği çöpe
atmanın zararı büyük. Kullanıcı döndüğünde işini yerinde bulur, kilit serbesttir.

## 5. Sözleşme kırılması (bilinçli)

"Kilit ZORUNLU", **kimse kilidi tutmuyorsa da kaydetmenin reddedilmesi** demektir. Aksi
hâlde kilit almayan iki istemci birbirini yine ezer ve mekanizma dekoratif kalır.

Bedeli: `PUT /wfd/draft/...`'ı kilit almadan çağıran her istemci `409` almaya başlar.
Editör bu işle birlikte güncelleniyor; varsa başka bir entegrasyon etkilenir.

İki ayrı kod, istemci ikisini farklı ele alsın:

| Kod | HTTP | Anlam |
|---|---|---|
| `draft.locked` | 409 | Başkasında; gövdede sahibi + `lock_expires_at` → kullanıcıya gösterilir |
| `draft.lock_required` | 409 | Kimsede değil ya da sende değil → istemci kilidi alıp KENDİLİĞİNDEN tekrar dener |

## 6. Kapsam dışı

**Zorla açma (`?force=true`).** Eklenmiyor: bu tasarımda gözetimsiz kilit 5 dakikada
kendiliğinden düşer, yani klasik "çöken sekme" vakası yok. Kalan senaryo "aktif çalışan
kişi devretmiyor" ki teknik değil insani bir sorundur. Seam hazır — `DELETE .../lock`'a
admin dalı (~20 satır).

**Çoklu eşzamanlı düzenleme (CRDT/OT).** Bambaşka bir problem; pessimistic kilit tam
olarak onu YAPMAMA kararıdır.

**Kilit devri talebi ("bana ver" bildirimi).** Bildirim altyapısı gerektirir.

## 7. Yerleşim ve testler

| Dosya | İş |
|---|---|
| `migrations/wf/20260811000004_wfd_draft_lock.sql` | Üç kolon (idempotent) |
| `crates/wfd/src/repo.rs` | `acquire_or_renew_lock`, `release_lock`; kilit koşulu mutasyon `WHERE`'lerine |
| `crates/wfd/src/models.rs` | Meta'ya kilit alanları |
| `crates/wfd/src/adapter.rs` | İnce sarmalayıcılar |
| `crates/server/src/routes/wfd.rs` | İki uç + kapı bağlama + `GET draft`'a kilit durumu |
| `crates/server/src/error.rs` | `draft.locked` / `draft.lock_required` eşlemesi |
| `agnoflow-frontend` | Kilit durum makinesi + popup + salt-okunur mod |

### 7.1 Testler

Kilit mantığı SQL'in `WHERE` cümlesinde yaşıyor (CAS orada) — bu repoda DB'li test
koşulmadığı için birim testiyle kapatılamaz. Güvence iki yerden gelir:

**Canlı duman testi** (gerçek sunucu + Postgres):

1. A kilitler → 200, `lock_expires_at ≈ now+5dk`
2. A tazeler → `expires_at` ileri gider, `acquired_at` DEĞİŞMEZ
3. B kilitlemeye çalışır → `409 draft.locked` + A'nın kimliği
4. B kaydetmeye çalışır → `409`
5. B yayınlamaya çalışır → `409`
6. A kaydeder → 200
7. A bırakır → 200; B kilitler → 200
8. Süresi geçmiş kilit (elle geriye çekilir) → B devralır, süpürücü GEREKMEZ
9. `published` / `pending_approval` satır → kilit reddedilir
10. Kilitsiz kaydetme → `409 draft.lock_required`
11. Kilit A'dayken B `DELETE .../lock` çağırır → `409`, kilit DÜŞMEZ
12. A kaydeder → `lock_expires_at` ileri gider (kaydetmek tazeler)

**Saf birim testi** (frontend, TDD): `lockUiState(lockExpiresAt, now)` →
`editing | warning | expired`. Popup anını sunucu damgasından türetmek bu fonksiyonun
işi; test sabit `now` ile zaman-bağımsız koşar.
