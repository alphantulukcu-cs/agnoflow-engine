# WFE Claim Devri (amir reassign) — Tasarım

**Tarih:** 2026-07-20
**Durum:** Onaylandı, implementasyona hazır
**Bağlam:** Patron toplantısı Madde 7 — "WFE instance değiştirme yetkisi senaryoları.
Örn: claim'e sahip kişiden amir sahipliği geri alabilir."

## Problem

Bugün bir WFE adımını bir kullanıcı `POST /wfe/:id/claim` ile üzerine alabiliyor
(CAS, `claimed_by IS NULL`). Ama:

- Claim'i **başkasına devretmek** için API yok.
- Claim'i **zorla geri almak / havuza döndürmek** için API yok.
- Var olan `WfeStore::release_claim` yalnızca sistemin SLA-1 `claim_timeout` otomatik
  temizliği için kullanılıyor — dışarıya açık, yetkilendirilmiş bir devir yolu değil.

İhtiyaç: yetkili bir "amir"in, bir claim'i sahibinden alıp (a) belirli birine
devredebilmesi veya (b) havuza (unassigned) geri bırakabilmesi; her devrin denetim
için WFAH geçmişine yazılması.

## Kararlar (onaylı)

| Karar | Seçim | Gerekçe |
|---|---|---|
| Yetki modeli | Node'da opsiyonel `reassign` C_A kuralı | "C_A tek kuraldır" değişmezini korur; mevcut `authorize()` matcher'ı yeniden kullanılır. Opsiyonel alan → golden fixture değişmez. |
| Operasyonlar | Hem belirli kişiye devir hem havuza bırakma | Patronun hem "devret" hem "geri al" senaryosunu karşılar. |
| Hedef kontrolü | Hedef, node c_a'sına uygun olmalı | Yetkisiz sahibe iş atanmasını engeller; claim ile aynı uygunluk kuralı. |
| Hedef tanımı | Tam aktör üçlüsü `{orgu_id, user_id, role}` | Node c_a'nın c_r (rol+orgu) kanalını doğrulayabilmek için gerekli; sadece user_id yetmez. |
| `reassign` kuralı yoksa | Devir tamamen kapalı (403) | Güvenli varsayılan; devir yalnızca WFD'nin açıkça izin verdiği node'larda. |

## Yetki modeli

Node'a **opsiyonel `reassign` C_A kuralı** eklenir; node'un `c_a`'sıyla birebir aynı
şekildedir: `{c_orgu, c_r?, c_u?}`. Devir yetkisi mevcut `authorize()` matcher'ından
geçer.

```jsonc
{
  "c_a": { "c_orgu": "self", "c_r": ["clerk"] },
  "reassign": { "c_orgu": "self.parent", "c_r": ["amir"] }  // opsiyonel
}
```

Alan opsiyonel olduğundan golden fixture (`docs/spec/example-wfd_kredi-basvuru_v2_2.json`)
**değişmeden** geçerli kalır (serde `default`).

## API

Tek endpoint: `POST /wfe/:id/reassign`

```jsonc
// Belirli kişiye devir (hedef = tam aktör üçlüsü):
{ "to": { "orgu_id": "<uuid>", "user_id": "<uuid>", "role": "clerk" },
  "node": "opsiyonel-kol-node-slug" }

// Havuza bırakma (force-unclaim):
{ "to": null }
```

- Reassigner = mevcut `X-Actor-*` header'ları (`extract_actor`).
- Hedef body'de tam üçlü olarak gelir ki `authorize(node.c_a, hedef)` doğrudan çalışsın.
- `node` opsiyoneldir; WOR-31 paralel modda hangi kolun claim'inin devredileceğini seçer
  (mevcut `claim` endpoint'iyle aynı desen).

Yanıt: `ReassignOutcome { success: bool, reason: Option<String> }` (claim ile simetrik).

## Akış — `Executor::reassign`

1. `load(wfe)` + `fetch(wfd)`; WFE terminal veya deadline-expired ise reddet.
2. Aktif node'u çöz (paralel modda `node` kolu); `node.reassign` kuralını al.
   Kural **yoksa** → `Unauthorized` (403, devir bu node'da kapalı).
3. `authorize(node.reassign, reassigner)` → false ise `Unauthorized` (403).
4. `to` verilmişse `authorize(node.c_a, hedef)` → false ise `TargetNotEligible` (400).
   `to: null` ise bu adım atlanır.
5. `WfeStore::reassign(...)` — TEK transaction:
   - `to = Some`: `claimed_by = hedef.user_id`, `claimed_at = now()`.
   - `to = None`: `claimed_by = NULL`, `claimed_at = NULL` (havuz).
   - WFAH marker append (aşağıya bkz.).
   - Paralel modda `wf.wfe_branch` satırında; kol için `FOR UPDATE` ile serialize.
6. `nudge_timers()` — SLA-1 claim saati sıfırlandı/başladı.

Engine katmanı (`Engine::reassign`) saftır (I/O yok): iki `authorize` çağrısını koşar,
sonucu ve yazılacak WFAH entry'sini döner; asıl yazımı adapter yapar.

## Audit (WFAH)

Her devir append-only bir `WfahEntry` yazar. `WfahEntry` şeması (`{seq, action, actor,
input, applied_at}`) **değişmez**:

- `action`: belirli kişiye devirde `"reassign"`, havuza bırakmada `"unclaim"`.
- `actor`: devri yapan reassigner (amir).
- `input`: `{ "from": <önceki_owner_uuid|null>, "to": <yeni_owner_uuid|null> }`.

## Concurrency

- Devir zaten claim'li (ya da havuzdaki) bir satırı override eder; `claim`'in
  `claimed_by IS NULL` CAS'ı burada geçersizdir. Bunun yerine yazım `wfe_id` (+ paralel
  modda kol) üzerinde `status = 'active'` koşuluyla ve paralel modda `FOR UPDATE`
  serialize ile yapılır — lost-update önlenir.
- Idempotentlik: `to: null` zaten havuzdaki bir WFE'de no-op benzeri (success döner,
  WFAH marker yine de yazılır çünkü denetim kaydı istenir). *(Karar: her başarılı çağrı
  bir WFAH kaydı üretir.)*

## Dokunulan yerler

| Katman | Değişiklik |
|---|---|
| `wfe-core/src/types/wfd_v22` | `Node`'a `reassign: Option<CandidateActor>` (serde `default`). |
| `wfe-core/src/validator` | `reassign` kuralına da c_a ile aynı cross-ref / slug / c_orgu / expression kontrolleri. |
| `wfe-core/src/error` | `TargetNotEligible` varyantı (→ 400). |
| `wfe-core/src/v22/ports` | `WfeStore::reassign(wfe_id, orgtnt_id, target, wfah_entry, branch)`. |
| `wfe-core/src/v22/pipeline` | `Engine::reassign` — iki authorize + WFAH entry üretimi (saf). |
| `wfe/src/wfe_adapter` | `reassign` impl (wfe + branch; CAS/`FOR UPDATE`). |
| `wfe/src/executor` | `reassign` orkestrasyonu + `nudge_timers`. |
| `server/src/routes/wfe` | `POST /:id/reassign` route + `ReassignBody`. |
| `docs/spec/` | reassign kuralının §3 (authorize) / §7 (pipeline) tanımına eklenmesi (SPEC kaynaklı; kod SPEC'e uyar). |

## Test planı

`#[tokio::test]` (zamana bağlıysa `start_paused = true`):

1. Yetkili amir belirli kişiye devreder → `claimed_by` değişir + `"reassign"` WFAH entry.
2. Yetkisiz aktör (reassign c_a'ya uymayan) → 403 `Unauthorized`.
3. Node'da `reassign` kuralı yok → 403 `Unauthorized`.
4. Hedef node c_a'ya uygun değil → 400 `TargetNotEligible`.
5. `to: null` → havuza bırakır (`claimed_by = NULL`); ardından uygun başka aktör
   `claim` edebilir.
6. Paralel modda (`node` verilir) kol-bazlı devir → yalnız o kolun `claimed_by`'ı değişir.
7. Terminal / expired WFE'de devir reddedilir.
8. Golden fixture (`example-wfd_kredi-basvuru_v2_2.json`) hâlâ validator'dan geçer
   (reassign alanı olmadan).

## Kapsam dışı (YAGNI)

- Org-birim hiyerarşisi tabanlı otomatik amir tespiti (parent/ancestors) — bu tasarımda
  yetki node'un `reassign` kuralıyla açıkça verilir, org-graph'tan türetilmez.
- Delegasyon/vekalet (Madde 6) — ayrı iş; bu tasarım yalnızca claim sahipliği devri.
- Toplu (bulk) reassign / bir kullanıcının tüm claim'lerini devretme — sonraki iş.
