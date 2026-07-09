# v2.2 Migration — Alınan Kararlar (KARAR issue'ları)

Bu doküman v2.2 migration'ında alınan tasarım kararlarını kaydeder (main branch).
Linear referansları: WOR-24..WOR-30. İki repoda senkron tutulur
(kanonik kaynak: WFD-EDITOR/docs/spec/; kopya: workflow-engine/docs/spec/).

**Editör hassasiyetleri (2026-07-08, kullanıcı gereksinimi — kararları bağlar):**
Editör UI iyi seviyede, bozulmaz. (1) Aksiyonlar CaGroup altında ona bağlı kalır —
kullanıcıya gösterilmek istenen görsel budur. (2) Oklara (edge) anlam atfedilmez;
ok = bağlantı gösterimi + seçilince silme. (3) Görsel/UX yeniden tasarımı yasak.
Spec'in React Flow görsel eşlemesi (humanPool/when-edge) ile çelişince bu hassasiyet
kazanır; spec yalnız export edilen JSON formatını bağlar, editör görselini değil.

## WOR-24 — WFE DB şeması: `current_node`

**Karar:** `wf.wfe` tablosuna `current_node TEXT NULL` kolonu eklendi.

- `NULL` = WFE terminal (node'da beklemiyor). Aktif WFE'de her zaman dolu.
- `current_c_a` JSONB kolonu KALDIRILMADI ama artık türetilmiş veri: current_node'un
  c_a'sının resolve edilmiş hali okuma-yolu (pool listing) için denormalize edilir.
- `claimed_by` (mevcut kolon) assignment'tır: node değişiminde `NULL`'a resetlenir
  (M8 — yeni node'a UNASSIGNED giriş). Claim, current node c_a match'i ile yapılır (§7.1).

## WOR-25 — `WftRule::Parallel` kaldırıldı

**Karar:** v2.2 WFT formları yalnızca `{node}` / `{terminal}` / `{conditions, default?}`.
`Parallel` variant'ı ve editördeki `parallel_wft` tamamen silindi. WOR-8'deki
"join_when parse ediliyor ama kullanılmıyor" bug'ı böylece kökten kapanır
(kod yolu artık yok).

## WOR-27 — Kök `WFD-Specification.json`

**Karar:** Engine reposunda kanonik spec `docs/spec/` altına kopyalandı.
CI ve kabul testleri sibling repoya (WFD-EDITOR) değil, repo-içi kopyaya bağlıdır.
Spec güncellendiğinde iki repo senkronize edilir; kanonik kaynak WFD-EDITOR/docs/spec.

**Kök dosya:** `WFD-EDITOR/WFD-Specification.json` stale (deprecated `_step_*`,
`terminal:true`, c_a içi `from`, 2 kurallı `start.c_a`). Golden fixture
(`docs/spec/example-wfd_kredi-basvuru_v2_2.json`) zaten kanonik örnek olduğundan
ikinci referans drift üretir → dosya `WFD-EDITOR/docs/legacy/` altına arşivlendi
(silinmedi, tarihsel referans). 2 kurallı `start.c_a` dosyayla birlikte ölür;
ayrıca modellenmesi gerekmiyor.

## WOR-28 — Eski seeded WFD fixture'ları

**Karar:** Eski v2 formatındaki seed'ler (`seed_kart_limiti_artisi`, `seed_kredi_basvuru`)
v2.2 loader tarafından REDDEDİLİR (`wfd_version` yok). Yeni migration eski seed
satırlarını siler ve golden fixture'ı (kredi-basvuru-v2) v2.2 seed'i olarak ekler.
Kart-limiti akışının v2.2'ye çevirisi ayrı işe bırakıldı — çok elemanlı c_a içeriyorsa
İNSAN ONAYI gerekir (M10).

## WOR-26 — Editör canvas'ında autoexec temsili

**Karar (Seçenek B):** Autoexec canvas'ta **mevcut görsel node** olarak kalır
(`AutoexecStepNode`, inline config görünümü). Export'ta v2.2 root `autoexec` kataloğu
+ transition `trigger[]` listesine düzleşir; import'ta yeniden çizilir. Görsel↔veri
eşlemesi `useGraphNodes` (render) ve `useExport` (serialize) katmanlarında yapılır.

- Gerekçe: config'i edge property'sine taşıyan Seçenek A, "oklara anlam atfedilmez"
  hassasiyetine aykırı — reddedildi. Trigger/retry/catch/timeout düzenlemesi autoexec
  node'u seçilince PropertiesPanel'de yapılır (edge seçerek DEĞİL).
- In-progress node-tabanlı işler (WOR-20/21/22) bu kararla uyumlu; iptal gerekmez.

## WOR-29 — `caGroupDisplay` çoklu-kural gösterimi

**Karar (Seçenek A):** Çoklu-kural display yolu ve testi silinir; `formatCaHeader`
tek `CandidateActor` objesi alır. c_a tek kurala inince (`c_a: CandidateActor`) tip
sistemi bunu zaten zorlar. Tek kural + çoklu rol (`c_r: string[]`) GEÇERLİ kalır
(noktalı virgülle çoklu *kural* birleştirme kalkar, çoklu *rol* gösterimi kalır).
Çoklu bağımsız grant gerekirse v2.2 yolu: ayrı `listable[]` kayıtları (ayrı formatter).

## WOR-30 — Editör `x_editor` embed'i vs `additionalProperties:false`

**Karar (Seçenek A/C birleşimi — layout sidecar):** `x_editor` root-embed'i kalkar.
Engine'e giden export her zaman temiz, şema-valid v2.2'dir. Editör layout'u ayrı
**sidecar**'da saklanır: `slug → {x, y}` pozisyonları + node görünüm tipleri
(CaGroup / aksiyon / switch / autoexec). Node key'leri deterministik slug olduğundan
sidecar dokümandan bağımsız eşlenebilir.

- Import: sidecar varsa oku (görsel birebir geri gelir); yoksa ELK auto-layout.
- Round-trip (`import(export(x)) == x`) hem doküman hem görsel yerleşimi korur.
- Gerekçe: tek temiz format + kullanıcının elle düzenlediği görsel korunur.

## Simetrik Start (v2.2 — `from` + `action:"start"`)

**Karar:** v2.2 `start` `transitions` ile simetrik hale getirildi: `{ id, from, action:"start", wfes_effects?, trigger?, wft }`. `c_a` startRule'dan kaldırıldı; artık `start[].from` ile referans edilen bir `nodes` girdisinde durur. Start-node kimliği `start[].from` referansından TÜRETİLİR — node'a ayrı bir `kind` alanı eklenmedi (dual source-of-truth riski nedeniyle reddedildi). `wfd_version` yerinde kaldı (`"2.2"`); v2.3'e geçilmedi çünkü v2.2 henüz aktif geliştirmede, dış tüketici yok, tek örnek dosya var.

- Eski (bu değişiklikten önceki v2.2): `startRule = { id, c_a(inline), wfes_effects?, trigger?, wft }` — `from`/`action` yoktu, `c_a` node'a değil doğrudan startRule'a bağlıydı.
- Yeni: `c_a` node'da; `startRule.c_a` alanı SİLİNDİ.

## Ek kararlar (bağımsız issue'lar)

- **WOR-10:** /org admin API'si `X-Admin-Key` başlığı ile korunur (`ADMIN_API_KEY` env).
  Unset ise dev modu: koruma kapalı + startup uyarısı. Kalıcı çözüm (admin JWT rolleri)
  ayrı işe bırakıldı.
- **WOR-11:** `CorsLayer::permissive()` kaldırıldı; `CORS_ORIGINS` env'i, unset ise
  yalnızca localhost dev origin'leri.
- **WOR-12/13/14/19:** Eski engine kodu silindiğinden kökten kapandı — v2.2'de
  autoexec'ler transition'a bağlı sonlu trigger listesidir (sonsuz döngü yolu yok),
  `trigger` alanı artık gerçekten çalışır, `next_seq` dead-code'ları kaldırıldı.
- **WOR-16:** OrgAdapter'ın inline `orgtnt_for_orgu` SQL'i korundu (tek sorgu,
  repo'ya taşıma davranış değiştirmiyor) — istenirse ayrı refactor.

## Kapsam notları

- WOR-26 / WOR-29 / WOR-30 (editör kararları) yukarıda kayıt altına alındı; kod
  uygulaması ilgili [EDITOR] issue'larında (WOR-50/52/53/49/54/55).
- Autoexec `python` / `lambda` tipleri şemada tanımlı; engine'de `Unsupported` hatası
  döner (executor'ları sonraki iş).
- Eski `crates/wfe-core/src/types/wfd.rs` modeli ve ona bağlı tüm deprecated yollar
  bu branch'in sonunda silindi; `$ctx.status` konvansiyonu kalktı (M1).
