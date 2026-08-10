-- WFAH'a akış izi: from_node / to_node (2026-08-10, WFE not tasarımı Faz 0, K7).
--
-- Sorun: `wf.wfah` bugün SADECE aksiyon adını tutuyor — hangi node'dan hangi
-- node'a gidildiği kayıtlı değil. "Bu onaya nereden gelindi" sorusu bugün
-- ancak akışı baştan yeniden oynatarak cevaplanabiliyor. Bu ikincil problem,
-- ad-hoc not defterini (Faz 1) motorun defterine (`wfah_seq` ile) çapalarken
-- ortaya çıktı: "5. onayla hangi adımdaydı, oraya nereden gelindi" notun
-- context'i olmadan cevapsız kalırdı.
--
-- Karar (K7): motor tipine (`WfahEntry`) alan EKLENMEZ — o tip `project_entry`
-- ile `$wfah`'a akıyor ve golden fixture'da serileşiyor; alan eklemek spec
-- yüzeyini ve fixture'ı değiştirirdi. Bilgi `WfeAdapter` seviyesinde türetilir:
-- `CommitOutcome` zaten hedefi (to_node), commit tx'i içindeki `wfe.current_node`
-- (paralelde outcome varyantının `from_node`'u) zaten kaynağı (from_node) biliyor.
-- Bu ekleme yalnız KAYIT VE EKRAN içindir — `$wfah` izdüşümü ve yayınlanmış
-- akışların koşul ifadeleri aynı kalır.
--
-- İkisi de NULLABLE: eski satırlar NULL kalır (backfill yok — geçmişi motor
-- olmadan yeniden türetmek mümkün değil), start satırında from_node NULL
-- (öncesi yok), ForkTo gibi çok-hedefli geçişlerde to_node NULL (hedefler
-- `wf.wfe_branch`'te zaten satır satır var).
ALTER TABLE wf.wfah
    ADD COLUMN from_node text,
    ADD COLUMN to_node   text;
