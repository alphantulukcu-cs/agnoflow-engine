-- WFD draft/published yaşam döngüsü (2026-07-10 tasarımı).
-- status='draft' satırlar mutable ve validate edilmemiştir; publish'te 'published' olur.
ALTER TABLE wf.wfd_meta
  ADD COLUMN status      text        NOT NULL DEFAULT 'published'
      CHECK (status IN ('draft','published')),
  ADD COLUMN description text,
  ADD COLUMN tags        text[]      NOT NULL DEFAULT '{}',
  ADD COLUMN owner       text        NOT NULL DEFAULT 'admin',
  ADD COLUMN updated_at  timestamptz NOT NULL DEFAULT now();

-- Bir (tenant, isim) için aynı anda en fazla tek açık draft.
CREATE UNIQUE INDEX wfd_single_draft
  ON wf.wfd_meta (orgtnt_id, name)
  WHERE status = 'draft';
