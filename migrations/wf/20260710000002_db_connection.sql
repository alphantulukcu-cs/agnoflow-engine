-- DB bağlantı deposu (2026-07-10 tasarımı). secret_enc: AES-256-GCM (nonce||ciphertext).
CREATE TABLE wf.db_connection (
    id           uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL,
    name         text        NOT NULL,
    driver       text        NOT NULL CHECK (driver IN ('postgres','mysql','mssql')),
    mode         text        NOT NULL DEFAULT 'fields' CHECK (mode IN ('fields','uri')),
    host         text,
    port         integer,
    database     text,
    username     text,
    options      jsonb       NOT NULL DEFAULT '{}',
    secret_enc   bytea,
    is_active    boolean     NOT NULL DEFAULT true,
    last_test_at timestamptz,
    last_test_ok boolean,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX db_connection_orgtnt_idx ON wf.db_connection(orgtnt_id);
