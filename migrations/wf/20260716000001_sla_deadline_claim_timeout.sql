-- ================================================================
-- SLA (2026-07-16 sözleşmesi): instance deadline (SLA-3), claim-timeout
-- (SLA-1) ve `terminated` statüsü (SLA ihlali sonlanması).
--
-- 1. wf.wfe.deadline    — çözülmüş mutlak workflow deadline'ı (start'ta
--    resolve edilir: min(start.deadline, wfd.timeout) ya da tek biri; NULL
--    = SLA-3 yok).
-- 2. wf.wfe.claimed_at  — claim CAS'ta set edilir; claimed_by temizlenince
--    NULL'lanır (node değişimi dahil) — SLA-1 sayaç sıfırlaması.
-- 3. status check constraint'e 'terminated' eklenir. Terminated aktif
--    DEĞİLDİR: list_active_ids / claim CAS zaten status='active' filtreler,
--    bu migration yalnız kolon + constraint ekler.
-- ================================================================

ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS deadline   timestamptz;
ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS claimed_at timestamptz;

ALTER TABLE wf.wfe DROP CONSTRAINT IF EXISTS wfe_status_check;
ALTER TABLE wf.wfe ADD CONSTRAINT wfe_status_check
    CHECK (status IN ('active', 'terminal', 'error', 'terminated'));

COMMENT ON COLUMN wf.wfe.deadline IS
    'SLA-3: çözülmüş mutlak workflow deadline''ı (start''ta hesaplanır); NULL = yok';
COMMENT ON COLUMN wf.wfe.claimed_at IS
    'SLA-1: claim CAS''ta set edilir; claimed_by temizlenince NULL''lanır (node değişimi dahil)';
