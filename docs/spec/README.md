# WFD Spesifikasyonu — KANONİK KAYNAK

Bu dizin **WFD v2.2** (Named Nodes, Single-Rule C_A) modelinin tek gerçek kaynağıdır.
Kod ile spec çelişirse **SPEC KAZANIR** ve kod düzeltilir.

Aynı dizin `agnoflow-backend/docs/spec/` ve `agnoflow-frontend/docs/spec/` altında
**birebir aynı** tutulur. Değişiklik yaparken iki kopyayı da güncelle.

## Ne nerede

| Dosya | İçerik | Ne zaman bakılır |
|---|---|---|
| [`terminology.md`](terminology.md) | Domain sözlüğü — ORGT/ORGU, ORGTRVLANG, C_A, WFE/WFES/WFAH, node/transition/wft, visibility | "Bu kavram ne demek?" |
| [`schema.json`](schema.json) | JSON Schema 2020-12 — yapısal doğrulama | "Bu alan geçerli mi, tipi ne?" |
| [`runtime-semantics.md`](runtime-semantics.md) | Davranış — matcher'lar, slug algoritması, pipeline, graf kuralları | "Çalışma anında ne olur?" |
| [`decisions.md`](decisions.md) | WOR-* karar kaydı — neden böyle, alternatifler neden elendi | "Bu neden böyle yapılmış?" |
| [`migration-notes.md`](migration-notes.md) | v2 / v2.1 → v2.2 delta (M1–M14) | "Eski model neydi, ne değişti?" |
| [`reference-types.rs`](reference-types.rs) | Doğrulanmış referans serde modeli + slug + matcher | "Rust tarafı nasıl modellenir?" |
| [`examples/`](examples/) | Örnek WFD'ler (aşağı bak) | "Gerçek bir WFD neye benzer?" |

### examples/

| Dosya | Kapsam |
|---|---|
| [`examples/kredi-basvuru.golden.json`](examples/kredi-basvuru.golden.json) | **GOLDEN FIXTURE — DEĞİŞTİRİLMEZ.** Her parse/validate/execute testi önce bununla geçmelidir; kod fixture'a uyar, fixture koda değil. |
| [`examples/paralel-onay.json`](examples/paralel-onay.json) | Fork/join, kol-bazlı SLA ve escalation |
| [`examples/belge-onay.json`](examples/belge-onay.json) | Ek-belge (attachments) katalogu ve node referansları |

**Sürüm dosya adlarında DEĞİL veridedir** — `schema.json` içindeki `$id` ve WFD'lerdeki
`wfd_version: "2.2"` alanı. Yeni sürüm gelirse bu dizin YERİNDE güncellenir; yan yana
ikinci bir sürüm dizini açılmaz (çoklu-sürüm drift'i bilinçli olarak engellenmiştir).

Detay gerektiğinde bu dosyaları oku; içeriklerini başka yere kopyalama.

## Model Özeti (her oturumda geçerli)

- state = `nodes` kataloğundaki bekleme havuzu; `$ctx.status` konvansiyonu YOK
- **C_A TEK KURALDIR** (object, array DEĞİL): `{ c_orgu, c_r?, c_u? }`
- matcher: `resolved(c_orgu) AND (rol_match OR user_match)`; verilmeyen alan = false (wildcard değil); c_u rol-agnostik
- node key = slug(c_a) (runtime-semantics §2a), `label` = UI ismi; aynı canonical c_a ikinci node'da OLAMAZ
- claim/assignment node'u değiştirmeyen runtime metadata
- transition: `from` (slug/array) + `action`; `when` sadece ek veri guard'ı; aynı (node,action) = ilk-match
- **start artık transition ile simetrik** (amended v2.2 in place): `{ id, from, action:"start", wfes_effects?, trigger?, wft }`; `c_a` startRule'da DEĞİL, `start[].from` ile referans edilen node'da; start-node kimliği referanstan türetilir (node'da `kind` alanı yok)
- wft: `{node}` / `{terminal}` / `{conditions[], default?}`; default yoksa `WFD.NoConditionMatched`
- trigger: `use` + `when?`/`required?`(true)/`retry[]?`/`catch?`; catch routing yapmaz
- tek exec namespace `$exec.result.*`; pipeline atomik; hata isimleri `WFD.*`
- **visibility matcher AYRI fonksiyondur ve kriterler arası OR'dur** — authorization matcher'ı ile birleştirme
- expression namespace'leri: `$ctx $wfah $prev $first $node $actor $timestamp $wfe_id $action.input.* $exec.result.*`
- `$wfah` girdisi `{seq, action, actor, input, at}`; `$prev`/`$first` uç girdi kısayollarıdır ve boş geçmişte null döner (patlamaz). `$wfah`'ı DOĞRUDAN indeksleme — dizi fonksiyonları İKİ argümanlıdır (`count($wfah, #.action == "x")`), `every` yok karşılığı `all`. Detay: TERMINOLOGY "EXPRESSION NAMESPACE"
- **attachments (ek-belge)** opsiyonel: root `attachments` katalogu (grup→`items[]`) + `nodes.<key>.attachments` (grup key referansları). Engine core dosya I/O YAPMAZ — katalog+referansı metadata tutar; varlık kontrolü + gate portal edge'inde (`422 attachment.missing`). Detay DECISIONS Madde 8 + runtime-semantics §6c.

## Çalışma Kuralları

- Her fazdan sonra `cargo test` (engine) / `npm test` + `tsc --noEmit` (editor); kırmızıyken sonraki faza geçme.
- Golden fixture'ı değiştirme; kod fixture'a uyar.
- İstenmeyen refactor yok; değişiklikler migration maddeleriyle (M1–M14) sınırlı; commit mesajına madde no yaz.
- Çok elemanlı eski `c_a` array'i ile karşılaşırsan OTOMATİK dönüştürme — dur ve sor (M10).
- Emin olamadığın tasarım kararında iki seçeneği açıkla ve sor; spec'i kendi başına genişletme.

## Engine (Rust) Hedefleri

- `docs/spec/reference-types.rs` → `src/wfd/` entegrasyonu; fixture parse + slug doğrulama kabul testi
- İki ayrı matcher: `authorize(c_a, actor, wfe)` ve `visible(x_visibility, actor, wfe)` (§3 ve §4)
- validator: cross-ref + slug/uniqueness + graf (BFS, escalation kenarları DAHİL) + çıkışsız node
- runtime: current_node, ilk-match seçim, atomik commit, retry/catch/timeout, escalation scheduler, `$node`

## Editor (React Flow) Hedefleri

- nodes→humanPool node (başlık=label, altyazı=slug), transitions→edge, escalation→kesikli edge
- c_a editörü tek-kural formu (orgu + roller + kişiler); slug otomatik üretilir, c_a değişince referanslar yeniden bağlanır
- export ajv ile v2.2 şemasına valide; `ui_*` alanları export'ta yok; import(export(x)) == x round-trip testi
