-- ================================================================
-- special → unit-inherited role.
-- 'special' orgu niteliği kaldırılır; yerine org.orgu_r (birime rol grant'ı)
-- gelir. Bir orgu'ya verilen rol, o orgudaki tüm kullanıcılarca devralınır
-- (check_user_role). Kullanıcı-rol (org.ur) DOKUNULMAZ; tek katalog org.r.
--
-- Migration tek başına ve tekrar tekrar koşabilmeli (idempotent). gen_random_uuid()
-- (PG13+, pg_catalog) kullanılır — eklenti/search_path bağımlılığı yoktur.
-- ================================================================
CREATE SCHEMA IF NOT EXISTS org;

-- 1) Birim-rol tablosu (org.ur'ye dokunulmaz; ayrı tablo).
CREATE TABLE IF NOT EXISTS org.orgu_r (
    orgu_r_id   uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id   uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    orgu_id     uuid        NOT NULL REFERENCES org.orgu(orgu_id),
    r_id        uuid        NOT NULL REFERENCES org.r(r_id),
    ur_type     text        NOT NULL DEFAULT 'granted'
                CHECK (ur_type IN ('inherited','granted')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgu_id, r_id)
);
CREATE INDEX IF NOT EXISTS orgu_r_orgu_idx ON org.orgu_r(orgu_id);
CREATE INDEX IF NOT EXISTS orgu_r_r_idx    ON org.orgu_r(r_id);

-- 2) Mevcut orgu_type.special değerlerini role kataloğuna taşı (idempotent).
INSERT INTO org.r (orgtnt_id, name, display_name)
SELECT DISTINCT oo.orgtnt_id, s.val, s.val
FROM org.orgu o
JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
CROSS JOIN LATERAL jsonb_array_elements_text(o.orgu_type->'special') AS s(val)
WHERE o.orgu_type ? 'special'
ON CONFLICT (orgtnt_id, name) DO NOTHING;

-- 3) Her special içeren birim için org.orgu_r grant'ı (idempotent).
INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
SELECT DISTINCT oo.orgtnt_id, o.orgu_id, r.r_id, 'granted'
FROM org.orgu o
JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
CROSS JOIN LATERAL jsonb_array_elements_text(o.orgu_type->'special') AS s(val)
JOIN org.r r ON r.orgtnt_id = oo.orgtnt_id AND r.name = s.val
WHERE o.orgu_type ? 'special'
ON CONFLICT (orgu_id, r_id) DO NOTHING;

-- 4) orgu_type'tan 'special' anahtarını kaldır ('type' kalır).
UPDATE org.orgu
SET orgu_type = orgu_type - 'special'
WHERE orgu_type ? 'special';
