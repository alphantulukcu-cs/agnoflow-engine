-- DB bağlantılarına kapsam (scope) eklenir:
--   global → tenant genelinde; her projedeki her WFD'de görünür/kullanılabilir.
--            Ayarlar sayfasından yönetilir.
--   local  → yalnızca TEK bir WFD'de görünür/kullanılabilir. WFD ayarları
--            sekmesinden yönetilir; başka bir projedeki (veya aynı projedeki başka)
--            WFD'de listelenmez.
--
-- Lokal sahiplik anahtarı (project_id, wfd_name)'dir — wfd_id DEĞİL: wfd_meta'da her
-- versiyon ayrı bir wfd_id satırıdır, dolayısıyla wfd_id'ye bağlamak yeni versiyonda
-- bağlantıyı koparırdı. Mantıksal WFD kimliği bu repoda her yerde (project_id, name)'dir
-- (bkz. wfd_meta_project_name_version_key, wfd_single_draft). Grup yeniden
-- adlandırıldığında wfd_name da taşınır (wf_wfd::repo::update_group_metadata).

ALTER TABLE wf.db_connection
    ADD COLUMN scope      text NOT NULL DEFAULT 'global',
    ADD COLUMN project_id uuid REFERENCES wf.project(project_id) ON DELETE CASCADE,
    ADD COLUMN wfd_name   text;

ALTER TABLE wf.db_connection
    ADD CONSTRAINT db_connection_scope_check CHECK (scope IN ('global', 'local'));

-- Global satırda sahiplik alanları BOŞ; lokal satırda ikisi de ZORUNLU.
ALTER TABLE wf.db_connection
    ADD CONSTRAINT db_connection_scope_owner_check CHECK (
        (scope = 'global' AND project_id IS NULL     AND wfd_name IS NULL) OR
        (scope = 'local'  AND project_id IS NOT NULL AND wfd_name IS NOT NULL)
    );

-- İsim benzersizliği kapsam başına iner: global'de tenant, lokal'de WFD grubu.
ALTER TABLE wf.db_connection DROP CONSTRAINT db_connection_orgtnt_id_name_key;

CREATE UNIQUE INDEX db_connection_global_name
    ON wf.db_connection (orgtnt_id, name)
    WHERE scope = 'global';

CREATE UNIQUE INDEX db_connection_local_name
    ON wf.db_connection (project_id, wfd_name, name)
    WHERE scope = 'local';

CREATE INDEX db_connection_local_owner_idx
    ON wf.db_connection (project_id, wfd_name)
    WHERE scope = 'local';
