-- ================================================================
-- WFD v2.3 sürüm kapısı: 2.2 belgeleri ARŞİVLENİR + yeni golden seed
-- (WOR-104 / R07-S2 + R07-S4)
--
-- `SUPPORTED_WFD_VERSION` TEK DEĞERLİ bir eşitliktir ve `"2.3"`e çevrildi
-- (WOR-71, `crates/wfe-core/src/types/wfd_v22.rs:49`). Kapı her fetch'te koşar
-- (`crates/wfd/src/adapter.rs`), dolayısıyla katalogda duran 2.2 belgeleri motor
-- tarafından artık okunamaz. Geriye uyum okuyucusu, okuma anında çeviri katmanı ve
-- belge migration'ı R07'de REDDEDİLDİ (Değişmez #9: ürün production'da değil).
--
-- Bu göç iki iş yapar:
--   S2  arşivleme  — `wfd_meta` + `wfd_template`in BÜTÜN satırları `is_active=false`
--   S4  yeni seed  — eski golden satırı arşivde kalır, v2.3 golden'ı aktif girer
--
-- SİLME YOK. Kaynak metin (storage blob'ları) DOKUNULMAZ — arşivin anlamı bu:
-- kullanıcı tarifleri yeniden kurarken onlara bakarak dönüşümü ELLE yapar (R07/S3,
-- çeviri aracı yazılmaz).
--
-- Emsal AYNI depoda: `20260706000001_v2_2_current_node_and_seed.sql:20-31` v2.1 → v2.2
-- geçişinde birebir bunu yaptı. O göçün ikinci adımı (eski belgeye bağlı aktif WFE'leri
-- `status='error'` işaretlemek) BURADA GEREKMEZ: `R01`/`WOR-107` tam sıfırlama bütün
-- WFE verisini zaten sildi (208 satır + 27 blob, 13 tablo boş).
--
-- ⚠️ SIRA İHLALİ KAYDA GEÇER: bağlayıcı sıra (R06/Adım 2) sıfırlama → arşivleme →
-- seed → kapı idi. Fiilen kapı BİRİNCİ koştu (WOR-71/0. adım, main 35d605b), sıfırlama
-- ikinci. Arada 2.2 belgeleri ve onlara pinli WFE'ler `UnsupportedWfdVersion("2.3")`
-- ile reddedildi; sıfırlama o WFE'leri sildiği için veri tarafındaki hasar kapandı.
-- Bu göç geriye kalan tek kalemi — arşivlenmemiş BELGELERİ — kapatır.
--
-- ⚠️ GERİ ALINABİLİR AMA YETKİLENDİRİLMEMİŞ: `is_active = true` yazmak belgeyi
-- yeniden koşum yoluna sokar ve kapı onu yine reddeder. `is_active` bir TUZAK
-- DÜĞMESİDİR (R07/S2).
-- ================================================================

-- ----------------------------------------------------------------
-- S2 — ARŞİVLEME. Hangi satırın 2.2 olduğu ARANMAZ.
--
-- `wf.wfd_meta`'da wire sürümünü taşıyan kolon YOKTUR (`version` belge
-- revizyonudur, `20260813000003_wfe_wfd_fk.sql:14-17`); 2.2 satırlarını ayırt etmek
-- her blob'u okumayı gerektirirdi. Bu göç koştuğu anda var olan HER belge 2.2'dir —
-- 2.3 belgesi henüz yayınlanmamıştır — dolayısıyla koşulsuz arşivleme birebir
-- doğrudur (R07/S2).
--
-- Kapsam `published` ile SINIRLI DEĞİL: taslaklar ve `pending_approval` da arşivlenir
-- (kullanıcı hükmü, R07/S2 tablo satır 2).
-- ----------------------------------------------------------------

UPDATE wf.wfd_meta
SET is_active = false, updated_at = now()
WHERE is_active;

UPDATE wf.wfd_template
SET is_active = false, updated_at = now()
WHERE is_active;

-- DOKUNULMAYANLAR (R07/S2 tablo satır 4-6), bilerek:
--   * storage blob'ları (`wfd_meta.s3_key`) — arşivin anlamı kaynak metnin durması
--   * `wf.wfd_env_var`            — `wfd_name` ile anahtarlı, sürüme bağlı değil;
--                                   aynı ADLA yeniden kurulan tarif değişkenlerini bulur
--   * `wf.wfd_template_project` / `wf.wfd_template_user` — arşivlenen şablonun yan verisi

-- ----------------------------------------------------------------
-- S4 — v2.3 golden fixture'ı YENİ seed olarak girer.
--
-- Eski golden satırı (`7a2e4c90-…`, v2.2 belgesine işaret ediyordu) yukarıdaki
-- arşivlemede kapandı. Aynı `wfd_id` yeniden kullanılamaz (PK) ve aynı
-- (project_id, name, version) üçlüsü de kullanılamaz (unique) → yeni satır YENİ
-- `wfd_id` ile ve `version = 2` olarak girer. Belgenin KENDİ adı (`name`) korunur:
-- `wfd_env_var` `wfd_name` ile anahtarlı (R07/S5).
--
-- `doc_id`/`doc_version` WFC çözümü içindir (`repo::resolve_doc`); değerler
-- dokümanın kendi alanlarından gelir:
--   docs/spec/examples/kredi-basvuru.golden.json → id="kredi-basvuru-v2", version="2.1.0"
--
-- ⚠️ BLOB ELLE KONUR. Bu göç yalnız meta satırını yazar; `s3_key`in gösterdiği JSON'ı
-- SQL yazamaz. Emsal göç de bunu bir yorumla belgeliyordu (`:11-12`). Depo yolu:
--
--   local  ($STORAGE_PATH altına):
--     cp docs/spec/examples/kredi-basvuru.golden.json \
--        "$STORAGE_PATH/3c1811a6-1e63-4261-a1ce-658da1fbfa6b/wfd/9fe82344-b477-4af9-bc73-0afed2c56c4f/2.json"
--   s3:
--     aynı anahtarı bucket'a yükle (`wf_wfd::storage::s3_key` biçimi:
--     `{orgtnt_id}/wfd/{wfd_id}/{version}.json`)
--
-- Blob konmadan satır aktiftir ama fetch "bulunamadı" verir; sıra ÖNEMSİZ, ikisi de
-- yapılmalıdır.
-- ----------------------------------------------------------------

INSERT INTO wf.wfd_meta (
    wfd_id, orgtnt_id, project_id, name, version, s3_key, status, is_active,
    description, owner, doc_id, doc_version
)
SELECT
    '9fe82344-b477-4af9-bc73-0afed2c56c4f',
    p.orgtnt_id,
    p.project_id,
    'Kredi Başvurusu',
    2,
    p.orgtnt_id::text || '/wfd/9fe82344-b477-4af9-bc73-0afed2c56c4f/2.json',
    'published',
    true,
    'v2.3 golden fixture (Ç5 düz aksiyon gövdesi + A05 escalation grant)',
    'admin',
    'kredi-basvuru-v2',
    '2.1.0'
FROM wf.project p
WHERE p.orgtnt_id = '3c1811a6-1e63-4261-a1ce-658da1fbfa6b'
  AND p.name = 'Test Project'
ON CONFLICT (project_id, name, version) DO NOTHING;
