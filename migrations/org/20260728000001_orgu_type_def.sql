-- Tenant-scoped katalog: org birimi tipleri (org.r/rol tablosuyla aynı desen).
-- orgu.orgu_type JSONB'i ({"type": "<key>"}) bu katalogdan bağımsız bir kopyadır —
-- katalog kaydı deactivate edilse bile var olan orgu kayıtları etkilenmez.

CREATE TABLE org.orgu_type_def (
    type_id      uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    key          text        NOT NULL,
    display_name text        NOT NULL,
    is_active    boolean     NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, key)
);
CREATE INDEX orgu_type_def_orgtnt_idx ON org.orgu_type_def(orgtnt_id);

COMMENT ON TABLE org.orgu_type_def IS
    'Tenant-scoped org birimi tipi kataloğu; org.orgu.orgu_type->>''type'' değerleriyle eşleşir. org.r (rol) ile aynı yönetim deseni.';

-- Mevcut tenant'lar için bilinen 5 tipi seed et (src/store/org-data.store.ts::getOrguIcon
-- ile aynı vocabulary) — böylece bugün var olan birimlerin tipi dropdown'da geçersiz görünmez.
INSERT INTO org.orgu_type_def (orgtnt_id, key, display_name)
SELECT t.orgtnt_id, v.key, v.display_name
FROM org.orgtnt t
CROSS JOIN (VALUES
    ('root', 'Genel Müdürlük'),
    ('bolge', 'Bölge'),
    ('sehir', 'Şehir'),
    ('ilce', 'İlçe'),
    ('sube', 'Şube')
) AS v(key, display_name)
ON CONFLICT (orgtnt_id, key) DO NOTHING;
