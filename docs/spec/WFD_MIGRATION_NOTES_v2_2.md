# WFD Migration Notes — v2 / v2.1 → v2.2

Bu doküman, mevcut engine/editor kodunun v2.2 modeline taşınması için delta'yı listeler ve önceki tüm migration notlarının yerini alır. Kanonik referanslar: `wfd_schema_v2_2.json`, `Terminology_v2_2.MD`, `wfd-custom-validator-runtime-semantics_v2_2.md`, golden fixture `example-wfd_kredi-basvuru_v2_2.json`, referans Rust modeli `wfd_types_v2_2.rs`.

Not: v2.1 hiç deploy edilmediyse bu doküman tek adımda v2 → v2.2 geçişi olarak okunur; M1–M9 v2.1'den taşınan maddelerdir, M10–M14 v2.2'nin ekleridir.

---

## M1. Root `nodes` Kataloğu

State = isimli bekleme havuzu. `$ctx.status` konvansiyonu ve onu set eden tüm effects kodu silinir. Engine `current_node` tutar, expression'lara `$node` olarak sunar.

## M2. Transition: `from` zorunlu, `when` opsiyonel guard

```json
{ "id": "t_x", "from": "self__creditAnalyst", "action": "approve", ... }
```

`from` string veya array. Seçim: aynı (node, action) için array sırasında `when`'i true olan İLK transition.

## M3. WFT formları: `{node}` / `{terminal}` / `{conditions, default?}`

Inline `wft.c_a` YOK. `default` yoksa match'sizlik = `WFD.NoConditionMatched`.

## M4. Trigger `retry[]` + `catch`

ASL semantiği. Catch effects uygular, handled sayar, devam eder; routing yapmaz.

## M5. Timeout'lar

`autoexec.*.timeout_seconds` (default 60) → `WFD.Timeout`; root `timeout` (ISO 8601).

## M6. Escalation (Node SLA)

`nodes.*.escalation[]`; assigned WFE'de de çalışır, taşımada assignment temizlenir.

## M7. Tek exec namespace: `$exec.result.*`

`$exec.response.*` her yerde hataya çevrilir.

## M8. Atomik pipeline

Diff'ler ancak WFT çözülünce commit; başarılı transition sonrası yeni node'a UNASSIGNED giriş.

## M9. Küçükler (v2.1'den)

`actions.*.name` → `label`; `terminal.wfes_effects` opsiyonel; `wfahAnchor.occurrence` (default last); hata taksonomisi `WFD.*`.

---

## M10. C_A TEK KURAL (v2.2 — EN KRİTİK)

Array formu ve kurallar-arası OR semantiği KALDIRILDI (orijinal karara dönüş).

Eski (v2/v2.1):

```json
"c_a": [
  { "c_orgu": "self", "c_r": ["creditAnalyst"] },
  { "c_orgu": "parent", "c_r": ["branchManager"] }
]
```

Yeni (v2.2):

```json
"c_a": { "c_orgu": "self", "c_r": ["creditAnalyst"] }
```

- Tek elemanlı array → objeye indirgenir (mekanik dönüşüm).
- ÇOK elemanlı array → tasarım kararı gerektirir: iki ayrı node'a bölünür veya ikincisi `listable` kaydına iner. Otomatik dönüştürülmez; migration aracı bu durumda İNSAN ONAYI istemelidir.
- Uygulandığı yerler: `nodes.*.c_a`, `start[].c_a`, `transitions[].c_a` (ek kısıt), `listable[].c_a`.
- Engine matcher: `resolved(c_orgu) AND (rol_match OR user_match)`; yok = false; c_u rol-agnostik. `any()` döngüsü silinir.

## M11. Node Kimliği = slug(c_a), İsim = `label` (v2.2)

- Node key elle yazılmaz; runtime-semantics §2a algoritmasıyla türetilir (`self__creditAnalyst`, `parent__creditDeptManager`, `wfah_submit_parent__branchManager`, c_u-only: `self__u_user_ayse`).
- `nodeDef.label` eklendi (UI ismi).
- Validator: key == slug kontrolü + canonical c_a uniqueness.
- Editör: c_a düzenlenince slug yeniden üretilir, tüm referanslar otomatik yeniden bağlanır; label korunur.

## M12. Aynı c_a = tek node kuralı (v2.2)

"Duplicate c_a node meşrudur" yaklaşımı TERS DÖNDÜ: aynı canonical c_a ikinci node'da = HATA. UI'da bir havuz bir kez görünür; aksiyonlar node içinde slot'tur.

## M13. Visibility matcher OR + ayrı fonksiyon (v2.2)

`x-visibility` kriterleri bağımsız, aralarında OR; `c_r`/`c_u` scope'suz; `c_a` (tek kural) scope'lu grant. Authorization matcher'ı ile TEK fonksiyonda birleştirilmez. Eski `c_user` → `c_u`.

## M14. Root metadata

`wfd_version: "2.2"` zorunlu; tanınmayan versiyon = yükleme reddi. `expression_language` default `"zen@1"`.

---

## Editor (React Flow) Eşlemesi

```text
nodes katalogu     -> humanPool node (basligi = label, alt yazi = slug)
terminals[]        -> terminal node
start[]            -> start node
transitions[]      -> edge (conditional wft = when-label'li coklu edge)
escalation         -> kesikli edge ("after" label)
```

Sanal node üretme katmanı silinir; export ajv (draft 2020-12) ile `wfd_schema_v2_2.json`'a valide edilir; `ui_*` alanları export'ta yoktur. c_a editörü tek-kural formudur: orgu seçici + rol çoklu-seçim + kişi çoklu-seçim.

## Doğrulama Sırası (CI kapıları)

1. JSON Schema validation (golden fixture geçmeli).
2. Custom validator: cross-ref + slug/uniqueness + context path + ZEN parse.
3. Graf analizi: BFS reachability (escalation kenarları DAHİL) + çıkışsız node.
4. Rust kabul testi: `wfd_types_v2_2.rs` fixture'ı parse eder, slug'ları doğrular.
