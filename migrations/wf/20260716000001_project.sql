-- Project katmanı: bir proje birden fazla WFD barındırır; tenant (orgtnt) scoped.
-- Mevcut tüm WFD'ler tenant başına otomatik açılan "Test Project"e bağlanır.

-- uuid-ossp bu DB'de org şemasına kurulu; uuid_generate_v4() oradan çözülür.
SET search_path = wf, org, public;

CREATE TABLE wf.project (
    project_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL,
    name        text        NOT NULL,
    description text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX project_orgtnt_idx ON wf.project(orgtnt_id);

-- Gelecek kullanıcı katmanı: project admin / project user üyelikleri.
-- Genel admin membership'siz global roldür, bu tabloya girmez.
CREATE TABLE wf.project_member (
    project_id uuid        NOT NULL REFERENCES wf.project(project_id) ON DELETE CASCADE,
    user_id    uuid        NOT NULL,
    role       text        NOT NULL CHECK (role IN ('admin','user')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX project_member_user_idx ON wf.project_member(user_id);

ALTER TABLE wf.wfd_meta ADD COLUMN project_id uuid REFERENCES wf.project(project_id);

INSERT INTO wf.project (orgtnt_id, name, description)
SELECT DISTINCT orgtnt_id, 'Test Project', 'Mevcut WFD''lerin otomatik bağlandığı proje'
FROM wf.wfd_meta;

UPDATE wf.wfd_meta m
SET project_id = p.project_id
FROM wf.project p
WHERE p.orgtnt_id = m.orgtnt_id AND p.name = 'Test Project';

ALTER TABLE wf.wfd_meta ALTER COLUMN project_id SET NOT NULL;
CREATE INDEX wfd_meta_project_idx ON wf.wfd_meta(project_id);

-- İsim benzersizliği artık proje kapsamında: farklı projelerde aynı isimli WFD serbest.
ALTER TABLE wf.wfd_meta DROP CONSTRAINT wfd_meta_orgtnt_id_name_version_key;
ALTER TABLE wf.wfd_meta ADD CONSTRAINT wfd_meta_project_name_version_key
    UNIQUE (project_id, name, version);

-- Tek açık draft kuralı da proje kapsamına iner.
DROP INDEX wf.wfd_single_draft;
CREATE UNIQUE INDEX wfd_single_draft
    ON wf.wfd_meta (project_id, name)
    WHERE status = 'draft';
