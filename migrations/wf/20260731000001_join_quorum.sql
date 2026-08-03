-- Active: 1781250192278@@192.168.3.148@5433@workflow_engine_test
-- ================================================================
-- WOR-72: OR-join (K-of-N quorum).
--
-- `wft.parallel` artık `join_mode: and | or` + `join_threshold` taşıyor. Runtime
-- bu ikiliyi TEK sayıya indirger (`ParallelSpec::quorum`) ve fork anında WFE
-- satırına yazar:
--   join_threshold IS NULL  → AND-join (WOR-31 davranışı; tüm kollar beklenir)
--   join_threshold = k      → k kol varır varmaz paralel mod OTORİTER kapanır,
--                             kalan aktif kollar `cancelled` olur.
--
-- Neden WFD'den her seferinde okunmuyor: aynı join hedefine giden iki ayrı fork
-- mümkündür, yani "hangi fork'un içindeyiz" bilgisi WFD'den tek başına
-- çıkmaz. Eşik, kol durumlarıyla aynı yerde (WFE satırı) yaşar.
--
-- Geriye dönük: mevcut satırlar NULL kalır = AND. Veri dönüşümü YOKTUR.
-- ================================================================

SET search_path = wf, org, public;

ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS join_threshold integer;

ALTER TABLE wf.wfe
DROP CONSTRAINT IF EXISTS wfe_join_threshold_positive;

ALTER TABLE wf.wfe
ADD CONSTRAINT wfe_join_threshold_positive CHECK (
    join_threshold IS NULL
    OR join_threshold >= 1
);

COMMENT ON COLUMN wf.wfe.join_threshold IS 'WOR-72: fork''ta persist edilen quorum eşiği; NULL = AND-join (tüm kollar), k = k varış yeterli';