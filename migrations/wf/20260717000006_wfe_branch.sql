-- ================================================================
-- WOR-31: paralel fork/join kalıcılığı (T3).
--
-- 1. wf.wfe_branch  — paralel modda her kolun runtime durumu. claim/entered
--    alanları KOL-bazlıdır (escalation dwell + claim_timeout paralel modda kol
--    üzerinden işler). `branch_node` kolun beklediği node slug'ıdır; alt-graf'lar
--    ayrık olduğundan (validator garanti eder) bir kolu node adıyla tekilleştirir.
-- 2. wf.wfe.join_target — fork'ta persist edilen AND-join hedefi ({node}/{terminal}
--    untagged JSON). NOT NULL ise WFE paralel moddadır (o an current_node NULL'dır).
--
-- Yarış çözümü: son-varış (JoinComplete) ve ara-varış (BranchArrived) commit'leri
-- wfe satırını `SELECT ... FOR UPDATE` ile kilitler; kol CAS + aktif-kol sayımı
-- eşleşmezse Conflict döner → executor reload edip engine'i yeniden koşar (T3).
-- ================================================================

-- WOR-15 ile aynı gerekçe: migration tek başına koşabilmeli.
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
SET search_path = wf, org, public;

CREATE TABLE IF NOT EXISTS wf.wfe_branch (
    branch_id   uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id      uuid        NOT NULL REFERENCES wf.wfe(wfe_id) ON DELETE CASCADE,
    branch_node text        NOT NULL,
    status      text        NOT NULL DEFAULT 'active'
                            CHECK (status IN ('active', 'arrived', 'cancelled')),
    claimed_by  jsonb,
    claimed_at  timestamptz,
    entered_at  timestamptz NOT NULL DEFAULT now(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, branch_node)
);
CREATE INDEX IF NOT EXISTS wfe_branch_wfe_idx    ON wf.wfe_branch(wfe_id);
CREATE INDEX IF NOT EXISTS wfe_branch_status_idx ON wf.wfe_branch(wfe_id, status);

ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS join_target jsonb;

COMMENT ON TABLE wf.wfe_branch IS
    'WOR-31: paralel mod kol durumları; claim/entered_at KOL-bazlı (SLA-1/SLA-2)';
COMMENT ON COLUMN wf.wfe.join_target IS
    'WOR-31: fork''ta persist edilen AND-join hedefi ({node}/{terminal}); NOT NULL = paralel mod';
