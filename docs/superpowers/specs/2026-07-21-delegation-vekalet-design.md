# Vekalet / Delegasyon (delegation) — Tasarım

**Tarih:** 2026-07-21
**Durum:** Kararlar onaylı; implementasyona hazır (audit = WFAH marker seçildi)
**Bağlam:** Patron toplantısı Madde 6 — kullanıcı izne çıkınca yetkisini (claim
eligibility'sini) geçici olarak başkasına/havuza devredebilmeli; yetki kontrolü
doğrudan rolden sonra aktif vekaletleri de dikkate almalı; her karar hangi yolla
(doğrudan/vekaleten) verildiğiyle loglanmalı.

## Çekirdek içgörü

Bu motor işi kişiye **itmez**; iş bir node'da `c_a` havuzuyla bekler, `c_a`'ya uyan
herkes **çeker**. Kritik olan: hem **claim yetkisi** (§3) hem de **görünürlük/inbox**
(§4 `can_view`, `listable`) AYNI `authorize(c_a, actor)` matcher'ından geçer
(`wfe-core/src/v22/visibility.rs::can_view` → `authorize`). Dolayısıyla vekaleti tek
noktaya — `authorize`'a — bağlarsak vekil işi hem **görür** hem **claim'ler**; iki ayrı
katman yamamaya gerek yok.

**Aksiyon almaya (apply) dokunulmaz:** apply sahiplik kontrol eder (`c_a`'yı değil).
Vekil işi claim edince sahibi *o* olur; normal akışla aksiyon alır. Vekalet yalnızca
**claim-öncesi uygunluk + görünürlük** katmanını genişletir.

## Model: "vekil, vekâlet verenin koltuğunu (şapkasını) giyer"

Vekil (Ayşe), vekâlet süresi boyunca, vekâlet verenin (Ahmet) **koltuğunun** eşleştiği
her `c_a`'da **da** eşleşir.

```
authorize_with_delegation(node.c_a, claimant, now):
    # 1) Doğrudan — bugünkü davranış
    if authorize(node.c_a, claimant): return Direct

    # 2) Vekaleten — claimant'a aktif vekalet veren her D için:
    for d in active_delegations_for(claimant, orgtnt, now):
        # claimant gerçekten bu vekaletin alıcısı mı? (kişi ya da havuz)
        if not authorize(d.grantee, claimant): continue
        # vekâlet verenin koltuğu bu node'un c_a'sına uyuyor mu?
        synthetic = Actor { orgu: d.seat.orgu_id, role: d.seat.role, user: d.delegator_user_id }
        if authorize(node.c_a, synthetic): return Delegated(d)
    return Denied
```

- İki `authorize` çağrısı da MEVCUT matcher'ı yeniden kullanır — yeni eşleşme mantığı yok.
- `c_a`'nın üç kanalını da tek sentetik aktör kapsar (orgu / rol / kişi-`c_u`).
- WFD şemasına **hiç** dokunmaz — vekalet org verisidir, akış tanımı değil.
- Role kanalı `authorize` içinde `org.check_user_role(synthetic)` çağırdığından, vekâlet
  verenin o koltuğu **gerçekten taşıdığı** auth anında da doğrulanır (koltuk sahipliği
  guard'ı bedava gelir).

### Kapsam (koltuk-bazlı, onaylı)

Bir vekalet TEK koltuğu delege eder: `seat = {orgu_id, role}`. "Tüm koltuklarım"
kısayolu grant anında delegator'ın mevcut üyeliklerine (`org.list_user_roles` +
`list_user_orgus`) genişler ve **her koltuk için ayrı satır** üretir (net audit + basit
auth yolu). Böylece Ahmet'in Beşiktaş-ilçeMüdürü koltuğunu delege etmesi, başka bir
kuruldaki üyeliğini etkilemez.

### Alıcı (grantee = CandidateActor, onaylı)

`grantee` bir CandidateActor'dır:
- **Kişi:** `{c_u: ["ayse"]}` → tek vekile devir.
- **Havuz:** `{c_orgu: "self", c_r: ["uzman"]}` → Ahmet'in şapkası o havuzdaki herkese
  geçer; iş "candidate havuza düşmüş" olur (patronun ikinci senaryosu).

Alıcı eşleşmesi de `authorize(d.grantee, claimant)` ile — yine aynı matcher.

### Zincir yok (tek seviye, onaylı)

Vekil, aldığı vekâleti başkasına devredemez. `active_delegations_for` yalnızca doğrudan
vekaletleri döndürür; sentetik aktör üzerinden ikinci bir vekalet turu KOŞULMAZ. Döngü/
derinlik sorunu olmaz, authorize'a en fazla tek ekstra seviye ekler.

## Veri modeli

```sql
CREATE TABLE org.delegation (
    delegation_id     uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id         uuid NOT NULL,            -- tenant izolasyonu
    delegator_user_id uuid NOT NULL,            -- şapka sahibi (c_u + audit)
    seat_orgu_id      uuid NOT NULL,            -- delege edilen koltuk: orgu
    seat_role         text NOT NULL,            -- delege edilen koltuk: rol
    grantee           jsonb NOT NULL,           -- CandidateActor (kişi VEYA havuz)
    valid_from        timestamptz NOT NULL DEFAULT now(),
    valid_to          timestamptz NOT NULL,     -- zaman penceresi (zorunlu)
    active            boolean NOT NULL DEFAULT true,  -- iptal edilebilir
    created_by        uuid NOT NULL,            -- self-service'te = delegator; amir atamasında = amir
    created_at        timestamptz NOT NULL DEFAULT now()
);
-- aktif vekalet sorgusu için indeks (grantee JSONB üzerinden değil; alıcı eşleşmesi
-- authorize ile yapılır, bu yüzden aday satırlar tenant + zaman + active ile daraltılır)
CREATE INDEX delegation_active_idx ON org.delegation (orgtnt_id, active, valid_from, valid_to);
```

> Not: `active_delegations_for(claimant)` önce tenant+zaman+active ile aday satırları
> çeker, sonra her aday için `authorize(grantee, claimant)` ile alıcı eşleşmesini
> doğrular. Grantee havuz olabildiği için alıcı filtresi SQL'de değil matcher'da yapılır.

## Governance (self + amir/admin, onaylı)

`POST /delegation` iki yolu da kabul eder:
- **Self-service:** `X-Actor` = delegator; `delegator_user_id = X-Actor.user_id`. Kişi
  yalnız **kendi** taşıdığı koltuğu delege edebilir (grant anında `check_user_role` ile
  doğrulanır).
- **Amir/admin adına:** `X-Actor` = amir veya `X-Admin-Key`; `delegator_user_id` gövdede
  gelir, `created_by = X-Actor`. (Amir-astı kontrolü Madde 7'deki reassign yetki
  modeline paralel ele alınır; v1'de admin-key + aynı-tenant yeterli, amir-astı
  sonraya bırakılabilir.)

CRUD: create / list (kendi verdiğim + bana verilen) / revoke (`active=false`). Portal UI
(work-pool-portal) grant/iptal ekranı.

## Dokunulan yerler

| Katman | Değişiklik |
|---|---|
| `migrations/org` | `org.delegation` tablosu + indeks. |
| `org` crate | `delegation` repo (create/list/revoke/active-for) + model. |
| `wfe-core/ports` | `OrgPort::active_delegations_for(claimant, orgtnt, now) -> Vec<DelegationGrant>` (default `Ok(vec![])` — geriye uyum). |
| `wfe-core/matcher` | `authorize`'ı saran `authorize_with_delegation`; `MatchEnv`'e istek-başı çekilen vekalet listesi. Doğrudan/vekaleten ayrımını döndürür (audit için). |
| `wfe-core/visibility` | `can_view` / `filter_dynctx` vekalet-farkında authorize'a geçer (tek satır değişiklik: `authorize` → `authorize_with_delegation`). |
| `wfe/org_adapter` | yeni port metodunun canlı implementasyonu (ltree/SQL). |
| `wfe/executor` + `pipeline` | `can_claim` vekalet-farkında; claim provenance (aşağı bkz.). |
| `server` | `/delegation` route'ları (JWT/portal + X-Admin-Key). |
| `docs/spec` | §3/§6'ya vekalet semantiği notu. |
| test | matcher birim testleri (doğrudan, vekaleten-kişi, vekaleten-havuz, süresi dolmuş/iptal, zincir-yok, koltuk-uyuşmaz), can_view vekalet testi, golden fixture değişmez. |

## Açık nokta — audit "acted via delegation"

Patron her kararın **hangi yolla** verildiğinin loglanmasını istiyor. Ama `claim()` şu an
WFAH yazmıyor (yalnız CAS ile `claimed_by` set ediyor; reassign/release marker yazıyor).
Vekaleten claim'i denetlenebilir kılmak için iki seçenek:

- **(A) WFAH marker (önerilen):** vekaleten claim başarılıysa `action = "claim:delegated"`,
  `actor = vekil`, `input = {delegator, seat, delegation_id}` markerı yazılır. Doğrudan
  claim eskisi gibi sessiz kalır. Append-only audit izine oturur, şema değişmez.
- **(B) Kolon:** `wf.wfe`'ye `claimed_via_delegation_id uuid NULL`. Sorgusu kolay ama
  paralel modda (`wfe_branch`) çift kolon gerekir ve node değişiminde temizlenmeli.

**KARAR: (A) — WFAH marker.** Vekaleten claim başarılıysa `action = "claim:delegated"`,
`actor = vekil`, `input = {delegator, seat, delegation_id}` markerı yazılır; doğrudan
claim eskisi gibi sessizdir.

## Kapsam dışı (YAGNI, v1)

- Zincirleme/transitif vekalet.
- Amir-astı grafiğinden otomatik "kim kimin amiri" türetme (admin-key + tenant yeterli).
- Vekalet çakışma/öncelik kuralları — birden çok aktif vekalet basitçe eligibility
  **birleşimi** üretir.
- Bildirim ("sana vekalet verildi") — sonraki iş.
