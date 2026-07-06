-- ================================================================
-- WFD SEED: Kredi Başvuru Akışı
--
-- wfd_id  : 0ba295fa-5c40-4254-a013-0577aa83a863
-- s3_key  : wfd/0ba295fa-5c40-4254-a013-0577aa83a863/1.json
--
-- orgtnt_id: QNB Finansbank (QNB_TR)
-- ================================================================

INSERT INTO wf.wfd_meta (wfd_id, orgtnt_id, name, version, s3_key, is_active)
VALUES (
    '0ba295fa-5c40-4254-a013-0577aa83a863',
    '3c1811a6-1e63-4261-a1ce-658da1fbfa6b',
    'Kredi Başvuru Akışı',
    1,
    'wfd/0ba295fa-5c40-4254-a013-0577aa83a863/1.json',
    true
)
ON CONFLICT (orgtnt_id, name, version) DO NOTHING;
