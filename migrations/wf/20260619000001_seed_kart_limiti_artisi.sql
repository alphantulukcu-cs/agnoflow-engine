-- ================================================================
-- WFD SEED: Bireysel Müşteri Kart Limiti Artışı
--
-- wfd_id  : c4f2a8e1-3b6d-4a9f-8c7e-1d5f0b2e6a3c
-- s3_key  : wfd/c4f2a8e1-3b6d-4a9f-8c7e-1d5f0b2e6a3c/1.json
--
-- orgtnt_id: QNB Finansbank (QNB_TR)
-- ================================================================

INSERT INTO wf.wfd_meta (wfd_id, orgtnt_id, name, version, s3_key, is_active)
VALUES (
    'c4f2a8e1-3b6d-4a9f-8c7e-1d5f0b2e6a3c',
    '3c1811a6-1e63-4261-a1ce-658da1fbfa6b',
    'Bireysel Müşteri Kart Limiti Artışı',
    1,
    'wfd/c4f2a8e1-3b6d-4a9f-8c7e-1d5f0b2e6a3c/1.json',
    true
)
ON CONFLICT (orgtnt_id, name, version) DO NOTHING;
