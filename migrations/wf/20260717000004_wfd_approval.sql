-- Yayın onay süreci: draft → pending_approval → published | (reddet) draft.
-- pending satır edit edilemez (draft status-gate'leri korur); onay/dogrudan
-- yayın yetkisi tenant admin + proje admininde.
ALTER TABLE wf.wfd_meta DROP CONSTRAINT wfd_meta_status_check;
ALTER TABLE wf.wfd_meta ADD CONSTRAINT wfd_meta_status_check
    CHECK (status IN ('draft', 'pending_approval', 'published'));
-- Son ret gerekçesi (draft'a düşünce gösterilir) + onaya gönderen.
ALTER TABLE wf.wfd_meta ADD COLUMN review_note   text;
ALTER TABLE wf.wfd_meta ADD COLUMN submitted_by  text;
-- Tek-açık-taslak kuralı onay bekleyeni de kapsasın: aynı (proje, ad) için
-- aynı anda tek draft VEYA tek pending satır (yarış: onaydayken yeni draft
-- açılabilir ama ikinci bir taslak/pending açılamaz).
DROP INDEX wf.wfd_single_draft;
CREATE UNIQUE INDEX wfd_single_draft ON wf.wfd_meta (project_id, name)
    WHERE status IN ('draft', 'pending_approval');
