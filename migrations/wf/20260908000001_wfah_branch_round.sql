-- WFAH satırının TURU — aynı fork'a yeniden giriş
-- (2026-09-08, WFD v2.3 — karar: E14/S3; iş: WOR-77).
--
-- Sorun: aynı fork'a İKİNCİ kez girilebiliyor (join sonrası bir döngüyle ya da geri
-- gönderme sonrası) ve ikinci tur aynı `entry_node`'u yeniden kullanıyor. Yani
-- birinci turun satırları ile ikinci turunkiler AYNI `branch_entry`'yi taşıyor;
-- `#.branch_entry == "hukuk"` yazan tasarımcının ifadesi iki turu TOPLUYOR.
--
-- Okuma tarafındaki eleme (eleme kuralı 5, `wfe_core::v22::valid`) `$valid`i düzeltir
-- ama HAM `$wfah` üzerinde "geçen turda ne oldu" sorusu alansız YAZILAMIYOR: çapa
-- `_fork`un `input.branches` listesini gerektiriyor, dizi fonksiyonları iki argümanlı
-- olduğu için kapanış içinde o listeyi gezmek mümkün değil ve `max([])` boş dizide
-- patlıyor. Alanla aynı soru tek satır:
--
--   some($wfah, #.branch_entry == "hukuk" and #.branch_round == $branch_round - 1)
--
-- Kolon NULLABLE ve DEFAULT'suzdur. `branch_entry` ile BİRLİKTE null ya da birlikte
-- dolu olması bir DEĞİŞMEZDİR: `branch_entry` NULL ⇔ `branch_round` NULL ("bu satır
-- bir kolda değil"). Motorda ikisi TEK fonksiyondan çıkar
-- (`wfe_core::v22::valid::round_of_opt`) ki bir üretici birini doldurup diğerini
-- atlayamasın.
--
-- Değeri motorun DEFTERDEN türetimidir: kolun giriş node'unu `input.branches`inde
-- taşıyan `_fork` satırlarının sayısı — 1'den başlar, FORK BAŞINA sayar (global sayaç
-- DEĞİL). `wf.wfe_branch`'e tur kolonu EKLENMEZ: o tablo yalnız YAŞAYAN turu taşır
-- (E14/S2) ve `$valid` saflık sözleşmesi tek girdi olarak defteri tanır (§3.1).
--
-- BACKFILL YOK (R01/S1, Değişmez #9): eski satırların turu geri türetilemez ve v2.3
-- inmeden önce koşan/koşmuş WFE verisi TAMAMEN silinir (R01/S2) — eski şekilli satır
-- kalmaz.
ALTER TABLE wf.wfah ADD COLUMN branch_round integer;

COMMENT ON COLUMN wf.wfah.branch_round IS
    'E14: satırın yazıldığı tur (fork başına, 1''den; defterdeki `_fork` sayımından türetilir); NULL = bu satır bir kolda değil — branch_entry ile BİRLİKTE null/dolu';

-- E14/S2 — `wf.wfe_branch` yalnız YAŞAYAN turun tablosudur; paralel modu bitiren HER
-- yol (AND join, quorum join, collapse, terminal/failed/terminated) satırları artık
-- SİLİYOR. Bunun iki sonucu YAZILMAYAN İKİ MIGRATION'dır:
--
--   1. Ç4-EK'in kısmi indeks migration'ı (`UNIQUE (wfe_id, branch_node)
--      WHERE status <> 'cancelled'`) YAZILMAZ. Eski tur satırı kalmadığı için
--      çakışacak bir şey yoktur ve tam kısıt, Ç4-EK'in bilinçle feda ettiği kalkanı
--      ("aynı kolu iki kez açan MOTOR HATASINI yakala") koruyarak yerinde kalır.
--   2. E14'ün öngördüğü "kapanmış turların artık kol satırlarını temizle" DELETE'i de
--      YAZILMAZ — R01/S5 açıkça düşürdü: tam sıfırlama `wf.wfe_branch`'i zaten
--      boşaltıyor.
--
-- `status` CHECK'inde `cancelled` KALIR ve kol `c_a` güncellemesinin `status =
-- 'active'` filtresi de kalır: canlı tur içinde bir kol anlık olarak `cancelled`
-- olabilir (silen transaction commit edene kadar).
