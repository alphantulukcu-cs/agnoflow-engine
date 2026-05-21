CREATE EXTENSION IF NOT EXISTS ltree;
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE SCHEMA IF NOT EXISTS org;

CREATE TABLE org.orgtnt (
    orgtnt_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    name       text        NOT NULL,
    code       text        NOT NULL UNIQUE,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE org.orgt (
    orgt_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    name        text        NOT NULL,
    description text,
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX orgt_orgtnt_idx ON org.orgt(orgtnt_id);

CREATE TABLE org.orgu (
    orgu_id    uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgu_type  jsonb       NOT NULL,
    name       text        NOT NULL,
    metadata   jsonb,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX orgu_type_gin     ON org.orgu USING gin(orgu_type);
CREATE INDEX orgu_active_btree ON org.orgu(is_active);
CREATE UNIQUE INDEX orgu_seed_code_unique
    ON org.orgu ((metadata->>'code'))
    WHERE metadata ? 'code';

CREATE TABLE org.orgt_orgu (
    orgt_id        uuid  NOT NULL REFERENCES org.orgt(orgt_id),
    orgu_id        uuid  NOT NULL REFERENCES org.orgu(orgu_id),
    orgtnt_id      uuid  NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    parent_orgu_id uuid  REFERENCES org.orgu(orgu_id),
    path           ltree NOT NULL,
    is_active      boolean     NOT NULL DEFAULT true,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (orgt_id, orgu_id),
    UNIQUE (orgt_id, path)
);
CREATE INDEX orgt_orgu_path_gist    ON org.orgt_orgu USING gist(path);
CREATE INDEX orgt_orgu_orgt_btree   ON org.orgt_orgu(orgt_id);
CREATE INDEX orgt_orgu_orgtnt_btree ON org.orgt_orgu(orgtnt_id);
CREATE INDEX orgt_orgu_parent_btree ON org.orgt_orgu(parent_orgu_id);
CREATE INDEX orgt_orgu_active_btree ON org.orgt_orgu(is_active);

CREATE TABLE org.u (
    u_id       uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    username   text        NOT NULL,
    full_name  text        NOT NULL,
    email      text,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, username)
);
CREATE INDEX u_orgtnt_idx ON org.u(orgtnt_id);

CREATE TABLE org.u_orgu (
    u_orgu_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    u_id       uuid        NOT NULL REFERENCES org.u(u_id),
    orgu_id    uuid        NOT NULL REFERENCES org.orgu(orgu_id),
    is_primary boolean     NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (u_id, orgu_id)
);
CREATE INDEX u_orgu_tenant_u_idx ON org.u_orgu(orgtnt_id, u_id);
CREATE INDEX u_orgu_u_idx        ON org.u_orgu(u_id);
CREATE INDEX u_orgu_orgu_idx     ON org.u_orgu(orgu_id);

CREATE TABLE org.r (
    r_id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    name         text        NOT NULL,
    display_name text        NOT NULL,
    is_active    boolean     NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX r_orgtnt_idx ON org.r(orgtnt_id);

CREATE TABLE org.ur (
    ur_id      uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    u_id       uuid        NOT NULL REFERENCES org.u(u_id),
    r_id       uuid        NOT NULL REFERENCES org.r(r_id),
    orgu_id    uuid        REFERENCES org.orgu(orgu_id),
    orgu_scope text,
    ur_type    text        NOT NULL DEFAULT 'granted'
               CHECK (ur_type IN ('inherited','granted','excluded')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ur_tenant_u_idx ON org.ur(orgtnt_id, u_id);
CREATE INDEX ur_u_idx        ON org.ur(u_id);
CREATE INDEX ur_r_idx        ON org.ur(r_id);
CREATE INDEX ur_orgu_idx     ON org.ur(orgu_id);
