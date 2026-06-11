-- ================================================================
-- WF ENGINE — Organizasyon Hiyerarşisi + ORGTRVLANG
-- PostgreSQL 15+  |  ltree extension gerekli
--
-- İÇERİK:
--   1. Extensions
--   2. Tablolar (orgtnt → orgt → orgu → orgt_orgu → u/r/ur)
--   3. Traversal notları
--   4. Bulk organizasyon seed loader (psql ile)
-- ================================================================


-- ================================================================
-- 1. EXTENSIONS
-- ================================================================

CREATE EXTENSION IF NOT EXISTS ltree;
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";


-- ================================================================
-- 2. TABLOLAR
-- ================================================================

-- ----------------------------------------------------------------
-- ORGTNT — En üst yapı; her organizasyonun tenant kökü
-- ----------------------------------------------------------------
CREATE TABLE orgtnt (
    orgtnt_id   uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    name        text        NOT NULL,
    code        text        NOT NULL,
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (code)
);

-- ----------------------------------------------------------------
-- ORGT — Bir ORGTNT altında birden fazla ağaç
-- Örnek: QNB → [Şubeler Ağacı, Merkezi Departmanlar]
-- ----------------------------------------------------------------
CREATE TABLE orgt (
    orgt_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL REFERENCES orgtnt(orgtnt_id),
    name        text        NOT NULL,
    description text,
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX orgt_orgtnt_idx ON orgt(orgtnt_id);

-- ----------------------------------------------------------------
-- ORGU — Ağaçtan bağımsız kimlik düğümü
-- orgu_type : jsonb {"type":"sube"} + opsiyonel özel tipler / wildcard
-- ----------------------------------------------------------------
CREATE TABLE orgu (
    orgu_id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgu_type       jsonb       NOT NULL,
    name            text        NOT NULL,
    metadata        jsonb,
    is_active       boolean     NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX orgu_type_gin      ON orgu USING gin(orgu_type);
CREATE INDEX orgu_active_btree  ON orgu(is_active);
-- Seed/code lookup: seed loader parent çözümlemesini tekilleştirir
CREATE UNIQUE INDEX orgu_seed_code_unique
    ON orgu ((metadata->>'code'))
    WHERE metadata ? 'code';

-- ----------------------------------------------------------------
-- ORGT_ORGU — ORGU'nun belirli bir ağaçtaki konumu
-- path ve parent_orgu_id burada tutulur; aynı ORGU birden fazla ağaçta olabilir
-- ----------------------------------------------------------------
CREATE TABLE orgt_orgu (
    orgt_id         uuid        NOT NULL REFERENCES orgt(orgt_id),
    orgu_id         uuid        NOT NULL REFERENCES orgu(orgu_id),
    orgtnt_id       uuid        NOT NULL REFERENCES orgtnt(orgtnt_id),
    parent_orgu_id  uuid        REFERENCES orgu(orgu_id),
    path            ltree       NOT NULL,
    is_active       boolean     NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (orgt_id, orgu_id),
    UNIQUE (orgt_id, path)
);

CREATE INDEX orgt_orgu_path_gist    ON orgt_orgu USING gist(path);
CREATE INDEX orgt_orgu_orgt_btree   ON orgt_orgu(orgt_id);
CREATE INDEX orgt_orgu_orgtnt_btree ON orgt_orgu(orgtnt_id);
CREATE INDEX orgt_orgu_parent_btree ON orgt_orgu(parent_orgu_id);
CREATE INDEX orgt_orgu_active_btree ON orgt_orgu(is_active);

-- ----------------------------------------------------------------
-- U — Kullanıcı
-- ----------------------------------------------------------------
CREATE TABLE u (
    u_id        uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL REFERENCES orgtnt(orgtnt_id),
    username    text        NOT NULL,
    full_name   text        NOT NULL,
    email       text,
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, username)
);

CREATE INDEX u_orgtnt_idx ON u(orgtnt_id);

-- ----------------------------------------------------------------
-- U_ORGU — U <-> ORGU üyelik (many-to-many)
-- orgtnt_id denormalize: "kullanıcının tüm unitleri" JOIN yok
-- ----------------------------------------------------------------
CREATE TABLE u_orgu (
    u_orgu_id   uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL REFERENCES orgtnt(orgtnt_id),
    u_id        uuid        NOT NULL REFERENCES u(u_id),
    orgu_id     uuid        NOT NULL REFERENCES orgu(orgu_id),
    is_primary  boolean     NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (u_id, orgu_id)
);

CREATE INDEX u_orgu_tenant_u_idx ON u_orgu(orgtnt_id, u_id);
CREATE INDEX u_orgu_u_idx        ON u_orgu(u_id);
CREATE INDEX u_orgu_orgu_idx     ON u_orgu(orgu_id);

-- ----------------------------------------------------------------
-- R — Rol
-- ----------------------------------------------------------------
CREATE TABLE r (
    r_id        uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL REFERENCES orgtnt(orgtnt_id),
    name        text        NOT NULL,
    display_name text       NOT NULL,
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);

CREATE INDEX r_orgtnt_idx ON r(orgtnt_id);

-- ----------------------------------------------------------------
-- UR — User Role; tam atama tuple
-- ur_type: inherited | granted | excluded
-- ----------------------------------------------------------------
CREATE TABLE ur (
    ur_id           uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id       uuid        NOT NULL REFERENCES orgtnt(orgtnt_id),
    u_id            uuid        NOT NULL REFERENCES u(u_id),
    r_id            uuid        NOT NULL REFERENCES r(r_id),
    orgu_id         uuid        REFERENCES orgu(orgu_id),
    orgu_scope      text,
    ur_type         text        NOT NULL DEFAULT 'granted'
                    CHECK (ur_type IN ('inherited','granted','excluded')),
    valid_from      timestamptz,
    valid_until     timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX ur_tenant_u_idx     ON ur(orgtnt_id, u_id);
CREATE INDEX ur_u_idx            ON ur(u_id);
CREATE INDEX ur_r_idx            ON ur(r_id);
CREATE INDEX ur_orgu_idx         ON ur(orgu_id);

-- ================================================================
-- 3. TRAVERSAL NOTLARI
--
-- Traversal artık veritabanı SQL fonksiyonları yerine uygulama katmanında
-- `org-api/src/traversal/executor.rs` içinden, `orgu + orgt_orgu` flatten
-- görünümü üzerinden çalıştırılıyor.
-- ================================================================


-- ================================================================
-- 4. BULK ORGANİZASYON SEED LOADER
--
-- Bu blok `psql -f init.sql` akışında büyük organizasyon seedlerini yükler.
-- Uygulama içinden migration çalıştırılıyorsa ve `\i` desteklenmiyorsa,
-- data altındaki seed dosyalarını aynı sırayla ayrıca çalıştırın.
-- ================================================================
\echo ''
\echo '--- Loading bulk QNB organization seeds ---'
\i data/seed_qnb_regions.sql
\i data/seed_qnb_ankara_units.sql
\i data/seed_qnb_istanbul_units.sql
\i data/seed_qnb_other_cities_units.sql
