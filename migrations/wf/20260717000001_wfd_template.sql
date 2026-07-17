-- Predefined schema (WFD şablonları): sık kullanılan akış kalıpları versiyonlu
-- şablon olarak saklanır; yeni WFD açarken galeriden seçilir.
-- scope='global'  → tenant geneli, yalnız tenant admin yönetir.
-- scope='project' → tek proje, tenant admin ya da o projenin admin'i yönetir.
-- Her (scope, proje, ad, versiyon) satırı IMMUTABLE snapshot'tır; "düzenleme"
-- yeni versiyon eklemektir. Şablondan açılan taslak ise tamamen serbesttir.

SET search_path = wf, org, public;

CREATE TABLE wf.wfd_template (
    template_id uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL,
    scope       text        NOT NULL CHECK (scope IN ('global','project')),
    project_id  uuid        REFERENCES wf.project(project_id) ON DELETE CASCADE,
    name        text        NOT NULL,
    description text,
    version     integer     NOT NULL DEFAULT 1,
    wfd_json    jsonb       NOT NULL,
    created_by  uuid        NOT NULL REFERENCES wf.app_user(user_id),
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    -- scope='project' iken proje zorunlu, 'global' iken boş
    CHECK ((scope = 'project') = (project_id IS NOT NULL)),
    UNIQUE NULLS NOT DISTINCT (orgtnt_id, scope, project_id, name, version)
);
CREATE INDEX wfd_template_orgtnt_idx ON wf.wfd_template(orgtnt_id);
CREATE INDEX wfd_template_project_idx ON wf.wfd_template(project_id);

-- Global şablonun seçilebileceği projeler; satır yoksa TÜM projelerde seçilebilir.
CREATE TABLE wf.wfd_template_project (
    template_id uuid NOT NULL REFERENCES wf.wfd_template(template_id) ON DELETE CASCADE,
    project_id  uuid NOT NULL REFERENCES wf.project(project_id) ON DELETE CASCADE,
    PRIMARY KEY (template_id, project_id)
);

-- Şablonu görebilecek kullanıcılar; satır yoksa HERKES görebilir.
CREATE TABLE wf.wfd_template_user (
    template_id uuid NOT NULL REFERENCES wf.wfd_template(template_id) ON DELETE CASCADE,
    user_id     uuid NOT NULL REFERENCES wf.app_user(user_id) ON DELETE CASCADE,
    PRIMARY KEY (template_id, user_id)
);
