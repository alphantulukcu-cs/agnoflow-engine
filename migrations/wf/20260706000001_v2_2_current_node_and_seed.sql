-- ================================================================
-- WFD v2.2 Migration (WOR-24 / WOR-28)
--
-- 1. wf.wfe.current_node — Named Nodes modeli: aktif WFE'nin beklediği
--    node slug'ı. NULL = terminal. Assignment (claimed_by) node
--    değişiminde engine tarafından temizlenir (M8).
-- 2. Eski v2 formatındaki seed WFD'ler deaktive edilir — v2.2 yükleme
--    kapısı (wfd_version zorunlu) bunları zaten reddeder (M14).
--    Onlara bağlı aktif WFE'ler artık çalıştırılamaz; dev ortamında
--    status='error' ile işaretlenir.
-- 3. Golden fixture (kredi-basvuru-v2, v2.2) yeni seed olarak eklenir.
--    JSON dosyası: storage/wfd/7a2e4c90-11d4-4b7e-9f3a-52c8e01b6f2d/1.json
-- ================================================================

ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS current_node TEXT;

COMMENT ON COLUMN wf.wfe.current_node IS
    'v2.2 Named Nodes: aktif WFE''nin beklediği node slug''ı; terminal''de NULL';

-- Eski format WFD'ler v2.2 altında parse edilemez. UI listesi sadece
-- fetch edilebilir v2.2 kayıtları göstermeli; bu yüzden mevcut tüm aktif
-- kayıtları kapatıp aşağıdaki v2.2 seed'i aktif bırakıyoruz.
UPDATE wf.wfd_meta
SET is_active = false
WHERE wfd_id <> '7a2e4c90-11d4-4b7e-9f3a-52c8e01b6f2d';

-- Eski format WFD'lere bağlı aktif WFE'ler v2.2 altında yürütülemez
UPDATE wf.wfe
SET status = 'error', updated_at = now()
WHERE status = 'active'
  AND wfd_id <> '7a2e4c90-11d4-4b7e-9f3a-52c8e01b6f2d';

-- v2.2 golden fixture seed'i (QNB_TR tenant'ı)
INSERT INTO wf.wfd_meta (wfd_id, orgtnt_id, name, version, s3_key, is_active)
VALUES (
    '7a2e4c90-11d4-4b7e-9f3a-52c8e01b6f2d',
    '3c1811a6-1e63-4261-a1ce-658da1fbfa6b',
    'Kredi Başvurusu',
    1,
    'wfd/7a2e4c90-11d4-4b7e-9f3a-52c8e01b6f2d/1.json',
    true
)
ON CONFLICT (orgtnt_id, name, version) DO NOTHING;
