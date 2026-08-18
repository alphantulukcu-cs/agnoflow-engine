-- ================================================================
-- Terminal-seviyesi görünürlük (2026-08-17) — `terminals[].listable[]`in
-- projeksiyonu. `20260813000004_node_listable.sql`ın KARDEŞİDİR ve aynı
-- düzeni sürdürür: karar commit anında hesaplanır, SQL'de containment
-- ile sorulur.
--
-- Cevaplanan soru: "bu akış bittikten sonra kimler görebilir?" Bugün
-- bunun tek cevabı kök `listable`/`wf_admin`tir ve SONUÇTAN BAĞIMSIZDIR
-- — onaylanan da reddedilen de aynı kümeye görünür. Oysa terminal'ler
-- zaten sonucu ayırıyor (her terminal ayrı kayıt); grant'ı oraya koymak
-- ayrımı `when` guard'ı yazmadan, yapıdan alıyor.
--
-- Neden mevcut kolonların hiçbiri YETMEZ:
--   * `wfe.view_c_a` (listable ∪ wf_admin) kalıcıdır ama SONUCU BİLMEZ.
--     Aynı kuralı iki farklı terminal için ayırmanın yolu yok.
--   * `wfe.current_view_c_a` (node listable) terminal'de BOŞALTILIR —
--     tanımı gereği: "WFE bu node'da İKEN". Terminal grant'ı oraya
--     yazılsa, boşaltıldığı için hiç okunmazdı; boşaltma kaldırılsaydı
--     node listable kalıcı olurdu ve 20260813000004'ün önlediği hata
--     geri gelirdi.
--   * `wfe.current_c_a` / `wfe_branch.c_a` ACT ADAYLARIDIR — oraya
--     yazmak bitmiş işe claim/ACT yetkisi verirdi (WOR-44'ün hatası).
--
-- Kural (`crates/wfe/src/visibility.rs::sql`, kriter (g)):
--     görünür(WFE, viewer) :=
--          view_c_a     @> viewer      -- KALICI, sonuçtan bağımsız
--       OR end_view_c_a @> viewer      -- KALICI, SONUCA BAĞLI       [YENİ]
--       OR (status='active' AND ( ... mevcut aktif kol ... ))
--
-- `end_view_c_a` `status='active'` kolunun DIŞINDADIR: WFE vardığı
-- terminal'den bir daha çıkmaz, dolayısıyla grant da geri alınmaz. Kolon
-- yalnız BAŞARILI `Terminal` commit'inde yazılır — `Failed` (error) ve
-- `Terminated` (SLA) yollarında varılmış bir terminal YOKTUR ve o
-- satırlar boş kalır (eski davranış: yalnız kök listable/wf_admin).
-- ================================================================

-- Varılan terminal'in çözülmüş grant listesi (`view_c_a` / `current_view_c_a`
-- ile AYNI biçim: CandidateActor[], `when` guard'ı UYGULANMIŞ, ORGTRVLANG
-- çapası `wfe.origin_orgu_id`).
ALTER TABLE wf.wfe
ADD COLUMN IF NOT EXISTS end_view_c_a jsonb NOT NULL DEFAULT '[]'::jsonb;

COMMENT ON COLUMN wf.wfe.end_view_c_a IS 'Varilan terminal''in listable[] grant''lari (cozulmus CandidateActor[], when uygulanmis). KALICIDIR (terminal''den cikis yok) ama view_c_a''dan farkli olarak SONUCA BAGLIDIR; yalniz basarili Terminal commit''inde yazilir, ACT VERMEZ';

-- WFE'nin BİTTİĞİ terminal id'si — `current_node`'un aynadaki karşılığı (biri
-- aktifken dolu, diğeri bittiğinde). İKİ tüketicisi var ve ikisi de zorunlu:
--   1. `can_view` (g) referans okuması: hangi terminal'in `listable[]`ına
--      bakılacağı yalnız bu satırdan bilinir (WFD'de birden çok terminal var,
--      mesele de bu).
--   2. `reproject`: org ağacı değişince bitmiş WFE'ler de yeniden projelendirilir;
--      terminal id satırda durmasaydı `end_view_c_a` bir daha ÜRETİLEMEZDİ.
-- `end_response` bu işi TEK BAŞINA göremez: yanıt gövdesi tasarımcınındır, hangi
-- terminal'den geldiğini taşıması garanti değildir. Ama BİR KANIT'tır — kolondan önce
-- bitmiş satırlarda `visibility_backfill`in ön geçişi onu WFAH'ın son aksiyonu ve
-- değişmez belgeyle birlikte kullanarak kolonu GERİYE DÖNÜK doldurur
-- (`wfe_core::v22::end_terminal`). Kanıtlar tek bir terminal'e indirgenmezse kolon
-- NULL bırakılır — kurtarma asla tahmin etmez.
ALTER TABLE wf.wfe
ADD COLUMN IF NOT EXISTS end_terminal text;

COMMENT ON COLUMN wf.wfe.end_terminal IS 'WFE''nin bittigi terminal id''si (terminals[].id); yalniz status=terminal satirlarda dolu. NULL = henuz bitmedi, ya da Failed/Terminated ile bitti, ya da bu kolondan onceki bir satir';

-- Containment (@>) index'i — komşu görünürlük kolonlarıyla AYNI gerekçe:
-- jsonb_path_ops yalnız @> destekler, bu kolonda sorulan tek soru da
-- containment'tır (bkz. 20260813000001).
CREATE INDEX IF NOT EXISTS wfe_end_view_c_a_gin ON wf.wfe USING gin (end_view_c_a jsonb_path_ops);
