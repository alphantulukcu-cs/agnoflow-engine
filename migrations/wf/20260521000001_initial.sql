-- WOR-15: uuid_generate_v4() bağımlılığı — org migrationdan bağımsız çalışabilmeli
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE SCHEMA IF NOT EXISTS wf;

CREATE TABLE wf.wfd_meta (
    wfd_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL,
    name       text        NOT NULL,
    version    integer     NOT NULL DEFAULT 1,
    s3_key     text        NOT NULL,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name, version)
);
CREATE INDEX wfd_meta_orgtnt_idx ON wf.wfd_meta(orgtnt_id);

CREATE TABLE wf.wfe (
    wfe_id       uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL,
    wfd_id       uuid        NOT NULL,
    wfd_version  integer     NOT NULL,
    status       text        NOT NULL CHECK (status IN ('active','terminal','error')),
    current_c_a  jsonb       NOT NULL DEFAULT '[]',
    end_response jsonb,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wfe_orgtnt_idx ON wf.wfe(orgtnt_id);
CREATE INDEX wfe_status_idx ON wf.wfe(status);
CREATE INDEX wfe_wfd_idx    ON wf.wfe(wfd_id);

CREATE TABLE wf.wfe_dynctx (
    dynctx_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    ctx        jsonb       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);
CREATE INDEX wfe_dynctx_wfe_idx ON wf.wfe_dynctx(wfe_id);

CREATE TABLE wf.wfah (
    wfah_id    uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    action     text        NOT NULL,
    actor      jsonb       NOT NULL,
    input      jsonb,
    applied_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);
CREATE INDEX wfah_wfe_idx ON wf.wfah(wfe_id);
