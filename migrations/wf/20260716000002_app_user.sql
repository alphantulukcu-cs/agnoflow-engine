-- Tasarım-zamanı kullanıcı katmanı (editör/yönetim); portal çalışanları (org.u) AYRI kalır.
-- Roller: tenant 'admin' (her şey + kullanıcı yönetimi) / 'member' (proje üyelikleriyle çalışır).
-- Proje yetkileri wf.project_member'da: 'admin' (project admin) / 'user' (tasarımcı).

SET search_path = wf, org, public;
CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA org;

CREATE TABLE wf.app_user (
    user_id       uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id     uuid        NOT NULL,
    email         text        NOT NULL,
    display_name  text        NOT NULL,
    password_hash text        NOT NULL,
    role          text        NOT NULL DEFAULT 'member' CHECK (role IN ('admin','member')),
    is_active     boolean     NOT NULL DEFAULT true,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, email)
);
CREATE INDEX app_user_orgtnt_idx ON wf.app_user(orgtnt_id);

ALTER TABLE wf.project_member
    ADD CONSTRAINT project_member_user_fk
    FOREIGN KEY (user_id) REFERENCES wf.app_user(user_id) ON DELETE CASCADE;

-- Her tenant'ın zorunlu bir admin'i olur: mevcut tenant'lara seed (email: admin / şifre: admin123).
-- API katmanı son aktif admin'in silinmesini/düşürülmesini reddeder.
INSERT INTO wf.app_user (orgtnt_id, email, display_name, password_hash, role)
SELECT t.orgtnt_id, 'admin', 'Tenant Admin', crypt('admin123', gen_salt('bf', 10)), 'admin'
FROM org.orgtnt t
ON CONFLICT (orgtnt_id, email) DO NOTHING;
