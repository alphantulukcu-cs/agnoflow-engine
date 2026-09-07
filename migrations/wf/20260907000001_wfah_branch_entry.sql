-- WFAH satırının KOLU + kol kimliğinin NOT NULL'a çekilmesi
-- (2026-09-07, WFD v2.3 — kararlar: Ç4, Ç2, Ç3; iş: WOR-75).
--
-- Sorun: `$valid` (elenmiş geçmiş) satır satır hesaplanacak ve eleme kuralı 1
-- "iptal/geçersizleşmiş KOLUN satırları elenir" diyor. Satır bugün hangi kolda
-- yazıldığını taşımıyor; kol kimliği yalnız marker payload'larından tahmin
-- edilebiliyordu (ve orada da YANLIŞ anahtar yazılıyordu — bkz. Ç3).
--
-- Kolonun değeri kolun `entry_node`'udur: kolun DEĞİŞMEZ kimliği (fork'taki giriş
-- node'u). Kol nereye giderse gitsin (kol içi hareket, fork öncesine geri gönderme)
-- satırın etiketi DEĞİŞMEZ.
--
-- NULLABLE ve SENTINEL YOK (`_main` gibi bir sihirli değer KULLANILMAZ — tasarımcı
-- o adı bir node'a verebilir ve proje bu deseni defalarca temizledi). `NULL` TEK
-- anlam taşır: "bu satır bir kolda değil" — fork öncesi satırlar, join sonrası
-- satırlar, `_fork`/`_collapse`/`_join` marker'ları ve paralel olmayan WFE'lerin
-- tüm satırları. "Kol bilinmiyor" diye bir hâl YOKTUR: satırı üreten kod kolu
-- bilir ve `WfahEntry.branch_entry` alanı derleyici tarafından zorunlu kılınır.
--
-- BACKFILL YOK (R01/S1): eski satırların hangi kola ait olduğu tek adımlı kollar
-- dışında geri türetilemez; v2.3 inmeden önce koşan/koşmuş WFE verisi TAMAMEN
-- silinir (R01/S2), dolayısıyla eski şekilli satır kalmaz.
ALTER TABLE wf.wfah ADD COLUMN branch_entry text;

COMMENT ON COLUMN wf.wfah.branch_entry IS
    'Ç4: satırı yazan kolun kimliği = wfe_branch.entry_node (kol hareket etse de değişmez); NULL = bu satır bir kolda değil (sentinel YOK)';

-- Ç2 DÜZELTMESİ — `20260810000001_wfah_path.sql` yorumundaki gerekçe cümlesi
-- YANLIŞTI: "`WfahEntry` `project_entry` ile `$wfah`'a akıyor ve golden fixture'da
-- serileşiyor" iddiasının ikinci yarısı diskte doğrulanamadı. `kredi-basvuru.golden.json`
-- bir WFD BELGESİDİR, WFAH satırı taşımaz ("wfah" anahtarı sıfır isabet) ve
-- `reference-types.rs`te `WfahEntry` YOK. Bu yüzden Ç2 çekirdek tipe `from_node`/
-- `to_node` alanlarını EKLEDİ (Ç4 de `branch_entry`'yi): K7'nin
-- `#.first_by_actor_at_node` anahtarı `(from_node, actor.user_id)` olduğu için alan
-- her hâlükârda satıra inmek zorundaydı. `$wfah` izdüşümü (`project_entry`)
-- DEĞİŞMEDİ — ZEN'e açılması E05'in işidir.

-- Ç4/S2 — kol kimliği mirası. `wf.wfe_branch`'e tek INSERT yolu var
-- (`wfe_adapter.rs`, `VALUES ($1, $2, $2, …)`) ve kolonu her zaman dolduruyor;
-- 2026-07-31 migration'ı mevcut satırları backfill etmişti. Yani NULL kalmış satır
-- YOK — sorun NULL değil, o backfill'in bazı satırlara kolun O ANKİ node'unu kimlik
-- diye yazması. R01/S2'nin tam sıfırlaması `wfe_branch`'i boşalttığı için düzeltme
-- UPDATE'i YAZILMAZ (R01/S5); geriye yalnız kısıt kalır. Kısıt, motordaki
-- `entry_or_current()` fallback'inin ve `Option<String>` sarmalının silinmesinin
-- ön koşuludur (Değişmez #9: pre-production'da geriye uyum kodu saf borçtur).
ALTER TABLE wf.wfe_branch ALTER COLUMN entry_node SET NOT NULL;

COMMENT ON COLUMN wf.wfe_branch.entry_node IS
    'WOR-73: kolun değişmez kimliği = fork''taki giriş node''u ($branches.<entry_node>). Ç4 (2026-09-07): NOT NULL — okuyan her yol kolonu çeker, fallback YOK';
