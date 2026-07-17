-- Türetme izi: bir WFD hangi predefined şablondan açıldıysa kaydı tutulur.
-- Şablon versiyonu silinirse iz NULL'a düşer (WFD kopya olduğundan etkilenmez).
ALTER TABLE wf.wfd_meta
    ADD COLUMN source_template_id uuid
    REFERENCES wf.wfd_template(template_id) ON DELETE SET NULL;
CREATE INDEX wfd_meta_source_template_idx ON wf.wfd_meta(source_template_id);
