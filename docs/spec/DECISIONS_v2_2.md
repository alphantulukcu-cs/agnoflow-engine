# v2.2 Migration — Alınan Kararlar (KARAR issue'ları)

Bu doküman `feat/wfd-v2.2-migration` branch'inde alınan tasarım kararlarını kaydeder.
Linear referansları: WOR-24..WOR-30.

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

**Karar:** Engine reposunda kanonik spec `docs/spec/` altına kopyalandı (bu commit).
CI ve kabul testleri sibling repoya (WFD-EDITOR) değil, repo-içi kopyaya bağlıdır.
Spec güncellendiğinde iki repo senkronize edilir; kanonik kaynak WFD-EDITOR/docs/spec.

## WOR-28 — Eski seeded WFD fixture'ları

**Karar:** Eski v2 formatındaki seed'ler (`seed_kart_limiti_artisi`, `seed_kredi_basvuru`)
v2.2 loader tarafından REDDEDİLİR (`wfd_version` yok). Yeni migration eski seed
satırlarını siler ve golden fixture'ı (kredi-basvuru-v2) v2.2 seed'i olarak ekler.
Kart-limiti akışının v2.2'ye çevirisi ayrı işe bırakıldı — çok elemanlı c_a içeriyorsa
İNSAN ONAYI gerekir (M10).

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

- WOR-26 / WOR-29 / WOR-30 (editör kararları) bu branch'te ele alınmadı — editör tarafı.
- Autoexec `python` / `lambda` tipleri şemada tanımlı; engine'de `Unsupported` hatası
  döner (executor'ları sonraki iş).
- Eski `crates/wfe-core/src/types/wfd.rs` modeli ve ona bağlı tüm deprecated yollar
  bu branch'in sonunda silindi; `$ctx.status` konvansiyonu kalktı (M1).
