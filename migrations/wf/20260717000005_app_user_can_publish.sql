-- Kullanıcı bazlı doğrudan yayın yetkisi: admin kullanıcıyı açarken seçer.
-- false = yayın onay sürecine girer (/submit), true = doğrudan /publish.
-- Tenant admin ve proje adminleri bayraktan bağımsız her zaman yayınlar/onaylar.
ALTER TABLE wf.app_user ADD COLUMN can_publish boolean NOT NULL DEFAULT false;
