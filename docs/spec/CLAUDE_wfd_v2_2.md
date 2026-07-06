# CLAUDE.md — WFD v2.2 Migration Guide (wfd-editor + workflow-engine)

Bu repo WFD v2.2 (Named Nodes, Single-Rule C_A) spec'ine taşınmaktadır. Spec dokümanları kanoniktir; kod ile spec çelişirse SPEC kazanır ve kod düzeltilir.

## Kanonik Spec Dosyaları (docs/spec/ altında)

- `wfd_schema_v2_2.json` — yapısal doğrulama (JSON Schema 2020-12)
- `Terminology_v2_2.MD` — domain kavramları
- `wfd-custom-validator-runtime-semantics_v2_2.md` — davranış: matcher'lar, slug algoritması, pipeline, graf
- `WFD_MIGRATION_NOTES_v2_2.md` — delta (M1–M14)
- `example-wfd_kredi-basvuru_v2_2.json` — GOLDEN FIXTURE: her parse/validate/execute testi önce bununla geçmelidir
- `wfd_types_v2_2.rs` — doğrulanmış referans serde modeli + slug + matcher (fixture'ı parse eder)

Detay gerektiğinde bu dosyaları oku; içeriklerini buraya kopyalama.

## Model Özeti (her oturumda geçerli)

- state = `nodes` kataloğundaki bekleme havuzu; `$ctx.status` konvansiyonu YOK
- **C_A TEK KURALDIR** (object, array DEĞİL): `{ c_orgu, c_r?, c_u? }`
- matcher: `resolved(c_orgu) AND (rol_match OR user_match)`; verilmeyen alan = false (wildcard değil); c_u rol-agnostik
- node key = slug(c_a) (runtime-semantics §2a), `label` = UI ismi; aynı canonical c_a ikinci node'da OLAMAZ
- claim/assignment node'u değiştirmeyen runtime metadata
- transition: `from` (slug/array) + `action`; `when` sadece ek veri guard'ı; aynı (node,action) = ilk-match
- wft: `{node}` / `{terminal}` / `{conditions[], default?}`; default yoksa `WFD.NoConditionMatched`
- trigger: `use` + `when?`/`required?`(true)/`retry[]?`/`catch?`; catch routing yapmaz
- tek exec namespace `$exec.result.*`; pipeline atomik; hata isimleri `WFD.*`
- **visibility matcher AYRI fonksiyondur ve kriterler arası OR'dur** — authorization matcher'ı ile birleştirme
- expression namespace'leri: `$ctx $wfah $node $actor $timestamp $wfe_id $action.input.* $exec.result.*`

## Çalışma Kuralları

- Her fazdan sonra `cargo test` (engine) / `npm test` + `tsc --noEmit` (editor); kırmızıyken sonraki faza geçme.
- Golden fixture'ı değiştirme; kod fixture'a uyar.
- İstenmeyen refactor yok; değişiklikler migration maddeleriyle (M1–M14) sınırlı; commit mesajına madde no yaz.
- Çok elemanlı eski `c_a` array'i ile karşılaşırsan OTOMATİK dönüştürme — dur ve sor (M10).
- Emin olamadığın tasarım kararında iki seçeneği açıkla ve sor; spec'i kendi başına genişletme.

## Engine (Rust) Hedefleri

- `docs/spec/wfd_types_v2_2.rs` → `src/wfd/` entegrasyonu; fixture parse + slug doğrulama kabul testi
- İki ayrı matcher: `authorize(c_a, actor, wfe)` ve `visible(x_visibility, actor, wfe)` (§3 ve §4)
- validator: cross-ref + slug/uniqueness + graf (BFS, escalation kenarları DAHİL) + çıkışsız node
- runtime: current_node, ilk-match seçim, atomik commit, retry/catch/timeout, escalation scheduler, `$node`

## Editor (React Flow) Hedefleri

- nodes→humanPool node (başlık=label, altyazı=slug), transitions→edge, escalation→kesikli edge
- c_a editörü tek-kural formu (orgu + roller + kişiler); slug otomatik üretilir, c_a değişince referanslar yeniden bağlanır
- export ajv ile v2.2 şemasına valide; `ui_*` alanları export'ta yok; import(export(x)) == x round-trip testi
