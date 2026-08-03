-- ================================================================
-- WOR-73: ZEN join koşulu (`join_mode: expr` + `join_when`) + kol KİMLİĞİ.
--
-- 1. wf.wfe.join_when — fork'ta persist edilen çözülmüş ZEN join koşulu.
--    Çözülmüş join kuralı (Wfes::join_rule) iki kolonun BİRLEŞİMİdir:
--      join_threshold NULL, join_when NULL  → All   (AND-join, WOR-31)
--      join_threshold = k                   → Quorum(k)                (WOR-72)
--      join_when = '<zen>'                  → Expr(<zen>)              (WOR-73)
--    İkisi birden dolu olamaz (CHECK) — "mod hem or hem expr" diye bir hâl yok.
--
-- 2. wf.wfe_branch.entry_node — kolun DEĞİŞMEZ kimliği (fork'taki giriş node'u).
--    `branch_node` kol içinde aksiyon alındıkça değişir (BranchMoveTo), dolayısıyla
--    "finans kolu vardı mı" sorusunun cevabı o kolon DEĞİLDİR. Join koşulu
--    (`$branches.<entry_node>`) ve varış-kümesi doğrulaması bu kolonla çalışır.
--    Mevcut satırlar için backfill = branch_node (koşan WFE'lerde kol hareket etmiş
--    olabilir; o kolun gerçek giriş node'u WFAH'taki `_fork` marker'ından okunabilir
--    ama koşan akışlarda AND-join kullanıldığı için kimlik hiç kullanılmıyordu —
--    en yakın doğru değer mevcut node'dur).
-- ================================================================

SET search_path = wf, org, public;

ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS join_when text;

ALTER TABLE wf.wfe DROP CONSTRAINT IF EXISTS wfe_join_rule_single;
ALTER TABLE wf.wfe ADD CONSTRAINT wfe_join_rule_single
    CHECK (join_threshold IS NULL OR join_when IS NULL);

ALTER TABLE wf.wfe_branch ADD COLUMN IF NOT EXISTS entry_node text;
UPDATE wf.wfe_branch SET entry_node = branch_node WHERE entry_node IS NULL;

COMMENT ON COLUMN wf.wfe.join_when IS
    'WOR-73: fork''ta persist edilen ZEN join koşulu (join_mode: expr); NULL = eşik/AND kuralı';
COMMENT ON COLUMN wf.wfe_branch.entry_node IS
    'WOR-73: kolun değişmez kimliği = fork''taki giriş node''u ($branches.<entry_node>)';
