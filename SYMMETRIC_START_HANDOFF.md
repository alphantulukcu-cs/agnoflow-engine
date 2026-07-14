# Engine Handoff — WFD v2.2 "Symmetric Start"

> **GÜNCEL NOT (M16, 2026-07):** Bu brief'teki "action rezerve sabit \"start\"" kuralı
> spec'ten KALDIRILDI (bkz. docs/spec/WFD_MIGRATION_NOTES_v2_2.md §M16). `start[].action`
> artık start aksiyonunun gerçek adıdır ve `actions{}` içinde normal bir ACT olarak tanımlanır;
> validator V4 buna göre güncellendi, eski V5 (reserved_action) kaldırıldı. Aşağısı tarihsel brief.

> Bu doküman, workflow-engine'i WFD v2.2'nin **simetrik start** değişikliğine uyarlamak
> için hazırlanmış bir görev brief'idir. Editor tarafı (WFD-EDITOR reposu) bitti ve merge
> edildi; engine bu değişikliği tüketecek şekilde güncellenmezse **yeni WFD JSON'ları parse
> edemez / çalıştıramaz**. Aşağıdaki her adımı uygula, sonda `cargo test --workspace` yeşil olmalı.

---

## Prompt (coding agent'a bunu ver)

> WFD v2.2'de `start` bloğunun şekli değişti — artık `transitions` ile simetrik. Bu repoyu
> (workflow-engine) yeni şekle uyarla. Kanonik tasarım kararı: spec dosyaları `docs/spec/`
> altında; "spec ile kod çelişirse spec kazanır". Aşağıdaki "Ne değişti", "Engine'de yapılacaklar"
> ve "Doğrulama" bölümlerini uygula. Golden fixture bu spec değişikliğiyle GÜNCELLENİR (kod
> fixture'a uyar kuralı, şeklin kendisi değişmediğinde geçerlidir; burada şekil spec gereği değişti).
> Eski çok-kurallı c_a array'i ile karşılaşmazsın — bu farklı bir migrasyon; sadece start'ın
> yeni tek-node şekline geç. Bittiğinde `cargo test --workspace` yeşil olmalı.

---

## Ne değişti (spec)

**Eski start:** `startRule = { id, c_a(inline), wfes_effects?, trigger?, wft }` — kim başlatabilir
bilgisi (`c_a`) start kuralının içindeydi; başlangıç havuzu bir node DEĞİLDİ.

**Yeni start:** `startRule = { id, from, action:"start", wfes_effects?, trigger?, wft }`
- `from` — `nodes{}` katalogundaki bir node id'si (**tekil**, array değil). Initiator yetkisi
  (`c_a`) artık bu node'da yaşıyor.
- `action` — rezerve sabit `"start"`. `actions{}` içinde tanımlanamaz.
- `c_a` start kuralından **kaldırıldı**.

**Tasarım ilkesi:** Start-node kimliği "bir `start[].from` tarafından refere edilmek"ten türer.
Node üzerinde `kind` alanı YOK. Node saf state kalır (`{ label?, description?, c_a, escalation? }`),
asla `wft`/`wfes_effects` taşımaz.

**Örnek (yeni şekil):**
```json
"nodes": {
  "type_branch__branchClerk": {
    "label": "Şube Memuru",
    "c_a": { "c_orgu": "*:[type:branch]", "c_r": ["branchClerk"] }
  },
  "self__creditAnalyst": { "c_a": { "c_orgu": "self", "c_r": ["creditAnalyst"] } }
},
"start": [
  {
    "id": "start__type_branch__branchClerk",
    "from": "type_branch__branchClerk",
    "action": "start",
    "wfes_effects": { "set": { "initiated_by": "$actor" } },
    "wft": { "node": "self__creditAnalyst" }
  }
]
```
Not: start node key'i `type_branch__branchClerk` — bu §2a slug(c_a) kuralının çıktısı
(`orguSlug("*:[type:branch]") = "type_branch"`, `+ "__" + "branchClerk"`). Elle "start__" öneki
verilmez; key her node gibi c_a'dan türer.

---

## Runtime çözüm semantiği

Actor rezerve `start` aksiyonunu çağırır → aktör her candidate start node'un `c_a`'sıyla
eşleştirilir → eşleşen node = etkin `from` → o start kuralının `wfes_effects`/`trigger`'ı çalışır
→ WFE `wft`'ye iner. `transitions` seçim modeliyle birebir (`from` + `action`).

Lifecycle notu: transition'da `node.c_a` = WFE'yi o an tutan owner; start node'da `c_a` = kim
BAŞLATABİLİR (henüz WFE yok). Aynı matcher mekaniği, farklı yaşam-döngüsü anlamı.

---

## Engine'de yapılacaklar

### 1. Tip — `crates/wfe-core/src/types/wfd_v22.rs:342`
`StartRule` struct'ı simetrik hale getir:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRule {
    pub id: String,
    /// v2.2 simetrik start: giriş node id'si (nodes katalogunda; initiator c_a'sını taşır). Tekil.
    pub from: String,
    /// Rezerve sabit "start".
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wfes_effects: Option<WfesEffects>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger: Vec<TriggerInvocation>,
    pub wft: Wft,
}
```
`c_a` alanını sil. `deny_unknown_fields` kalır (eski `c_a` içeren doküman böylece reddedilir).
(Opsiyonel sağlamlık: `action`'ı `#[serde(deserialize_with=...)]` ile "start"e zorlayabilirsin,
ama V4 validator + JSON Schema zaten const kontrolü yapıyor.)

### 2. Runtime start çözümü — `crates/wfe-core/src/v22/pipeline.rs:70-79`
Şu an inline `r.c_a` ile authorize ediyor:
```rust
for r in &wfd.start {
    let env = MatchEnv { ctx: &empty_ctx, wfah: &empty_wfah, orgtnt_id };
    if authorize(&r.c_a, actor, env, self.org).await? {   // ← r.c_a artık YOK
        rule = Some(r); break;
    }
}
```
Yeni: `from` node'un c_a'sıyla authorize et:
```rust
for r in &wfd.start {
    let node = wfd.nodes.get(&r.from)
        .ok_or_else(|| EngineError::/* uygun hata: start.from bilinmeyen node */)?;
    let env = MatchEnv { ctx: &empty_ctx, wfah: &empty_wfah, orgtnt_id };
    if authorize(&node.c_a, actor, env, self.org).await? {
        rule = Some(r); break;
    }
}
```
(Validator V1 zaten `from`'un var olduğunu garantiler; runtime yine de defensive `.get()` kullan.)

WFAH kaydı (`pipeline.rs:107`) şu an `format!("start:{}", rule.id)` yazıyor — istersen
`rule.action` ("start") + `rule.from`'u da audit'e ekle; zorunlu değil.

`server` katmanındaki start endpoint'i "start" aksiyonunu nasıl alıyor kontrol et — eğer
başlatma çağrısı bir `action` alanı taşıyorsa, `"start"` beklendiğini doğrula.

### 3. Validator — `crates/wfe-core/src/validator.rs`
Mevcut start döngüleri (satır ~73, 171, 264, 322, 448, 515, 584) çoğunlukla korunur
(unique id, trigger cross-ref, wft ref, reachability, when/effects/catch kontrolleri — hepsi
`from`/`action` ile de geçerli). EKLE (spec V1–V6):

| # | Kural | Uygulama |
|---|-------|----------|
| V1 | `start[].from` var olan bir node'a işaret etmeli | `wfd.nodes.contains_key(&s.from)` değilse `cross_ref` error |
| V2 | start.from node'u HİÇBİR `wft.node` hedefi olamaz (giriş-only) | tüm `wft` node hedeflerini topla (transitions + start + node escalation), `s.from` içindeyse error |
| V3 | start node'u `escalation` taşıyamaz | `wfd.nodes[&s.from].escalation` boş değilse error |
| V4 | `start[].action == "start"` | değilse error (JSON Schema const de yakalar; engine de kontrol etsin) |
| V5 | `"start"` aksiyonu `actions{}` içinde OLAMAZ | `wfd.actions.contains_key("start")` ise error |
| V6 | en az 1 start | zaten var (schema minItems / mevcut kontrol) |

Not: §2a slug kuralı start node için de geçerli — `from` bir node key olduğundan node key
üretimi/uniqueness kontrolleri onu da kapsar; ayrıca bir şey gerekmez.

Reachability (satır ~322): start artık `from` node üzerinden başlıyor — `from` node'un da
"reachable/kullanılıyor" sayıldığından emin ol (V2 zaten onu wft hedefi olmaktan men ediyor,
ama reachability graph'ında start node bir kaynak (source), dead-node uyarısı vermemeli).

### 4. matcher.rs yorumu — `crates/wfe-core/src/v22/matcher.rs:10`
"node c_a, start[].c_a, ..." diyor. `start[].c_a` artık yok; yorumu "start `from` node'unun
c_a'sı" olacak şekilde güncelle. Matcher mantığı değişmez (generic `authorize`).

### 5. Spec senkron + golden fixture
- `docs/spec/wfd_schema_v2_2.json`, `docs/spec/wfd-custom-validator-runtime-semantics_v2_2.md`,
  `Terminology_v2_2.MD`, `DECISIONS_v2_2.md`, `WFD_MIGRATION_NOTES_v2_2.md`, `CLAUDE_wfd_v2_2.md`
  → WFD-EDITOR reposunun `docs/spec/`'inden senkronla (editor tarafında güncellendi).
- Golden fixture (iki kopya): `docs/spec/example-wfd_kredi-basvuru_v2_2.json` VE
  `crates/wfe-core/tests/fixtures/example-wfd_kredi-basvuru_v2_2.json` → editor'ün migrate
  edilmiş sürümüyle değiştir (yeni start + `type_branch__branchClerk` node). İkisi birebir aynı olmalı.
- `crates/wfe-core/src/types/wfd_v22.rs` içindeki Rust'ın kanonik referansı editor'deki
  `docs/spec/wfd_types_v2_2.rs` — oradaki `StartRule` ile aynı olmalı.

### 6. Testler
`crates/wfe-core/tests/`: `golden_fixture.rs`, `pipeline.rs`, `visibility_view.rs` fixture'ı
kullanıyor. Fixture yeni şekle geçince başlangıç authorize akışını test eden case'ler
`start[].c_a`'ya değil `from` node c_a'sına dayanmalı. Start-happy-path testi: `branchClerk`
aktörü `start` ile WFE başlatabilmeli; yetkisiz aktör `StartNotEligible` almalı. V1–V5 için
validator birim testleri ekle (her kural: bir geçerli + bir ihlal case).

---

## Referanslar (WFD-EDITOR reposu — tamamlanmış editor tarafı, örnek olarak oku)
- Tasarım: `WFD-EDITOR/docs/superpowers/specs/2026-07-09-symmetric-start-design.md`
- Plan (adım adım editor implementasyonu): `WFD-EDITOR/docs/superpowers/plans/2026-07-09-symmetric-start.md`
- Editor validator (V1/V2/V3/V5 TS referans implementasyonu):
  `WFD-EDITOR/WFD/wfd-editor/src/utils/validateStartRules.ts`
- caSlug/node-key algoritması (engine slug ile birebir olmalı):
  `WFD-EDITOR/WFD/wfd-editor/src/utils/v22.ts`

## Doğrulama (bitiş kriteri)
```bash
cargo test --workspace          # tümü yeşil
```
- Yeni WFD JSON (from/action) hatasız deserialize + validate olmalı.
- Eski WFD JSON (inline c_a) `deny_unknown_fields` / eksik alan ile reddedilmeli.
- Start happy-path + V1–V5 ihlal testleri geçmeli.

## Dikkat / tuzaklar
- `deny_unknown_fields`: eski `c_a` + yeni `from`/`action` bir arada gelirse hata verir — istenen bu.
- `wfd_version` "2.2" olarak KALIR (yerinde amend, bump yok).
- `"start"` action'ı `actions{}` map'ine hiç girmemeli (V5) — resolver/matcher onu gerçek ACT sanmasın.
- Start node key elle "start__" değil; c_a'dan slug ile türer (`type_branch__branchClerk` gibi).
