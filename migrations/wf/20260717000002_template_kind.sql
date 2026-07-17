-- Predefined CONTEXT desteği: şablon kaydı artık iki tür taşır.
-- kind='workflow' → WFD dokümanı; kind='context' → yeniden kullanılabilir
-- context şeması ({properties, required}). Versiyonlama/scope/görünürlük/yetki
-- kuralları iki tür için AYNIDIR; aile anahtarına kind eklenir.

ALTER TABLE wf.wfd_template
    ADD COLUMN kind text NOT NULL DEFAULT 'workflow'
    CHECK (kind IN ('workflow','context'));

ALTER TABLE wf.wfd_template
    DROP CONSTRAINT wfd_template_orgtnt_id_scope_project_id_name_version_key;
ALTER TABLE wf.wfd_template
    ADD CONSTRAINT wfd_template_family_version_key
    UNIQUE NULLS NOT DISTINCT (orgtnt_id, kind, scope, project_id, name, version);
