-- WFE not dosyası — Faz 2 (ad-hoc belge iliştirme), 2026-08-10.
-- docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md
--
-- Storage anahtarı `notes/{wfe_id}/{file_id}` (server/src/attachments.rs::note_key) —
-- katalog attachment'ların `attachments/{wfe_id}/{grup}/{item}` prefiksinden AYRI: ad-hoc
-- dosyanın katalog karşılığı (grup/item, format kuralı) yoktur; aynı ağaca karışsa
-- `status_for_node`/gate mantığını yanıltırdı. `storage_key` kolonu bu anahtarın izidir
-- (bilgi/denetim amaçlı) — gerçek okuma/yazma/silme `AttachmentStore::note_write` /
-- `note_read` / `note_delete` ile `wfe_id`+`file_id`'den yeniden türetilir, kolon
-- yorumlanmaz.
--
-- K3 — yayınlanmış not DEĞİŞTİRİLEMEZ kuralı dosyayı da kapsar: `server/src/notes.rs`
-- `add_file`/`remove_file` `status != 'draft'` ise `409 code:"note.immutable"` döner.
-- `ON DELETE CASCADE` yalnız notun KENDİSİ silindiğinde (draft iken vazgeçme ya da
-- yetim taslak süpürmesi) devreye girer — published notun dosya kümesi sabit kalır.
--
-- K4 — belge yazımı `attachment_store::store_for_wfe_strict` ile RUNTIME'da zorlanır
-- (fallback YOK; `$env`'de depo tanımlı değilse 422 `attachment_storage.missing_env`).
-- Bu tablo yalnız metadata tutar, hangi deponun kullanıldığını bilmez/karar vermez.
--
-- Yetim draft dosyaları mevcut saatlik süpürücüyle (`server/src/reservation.rs` →
-- `notes::sweep_expired_drafts`, TTL 24 saat) DB satırı (CASCADE) + storage blob'u
-- birlikte temizlenir.
CREATE TABLE wf.wfe_note_file (
    file_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    note_id     uuid        NOT NULL REFERENCES wf.wfe_note(note_id) ON DELETE CASCADE,
    filename    text        NOT NULL,
    mime        text        NOT NULL,
    size_bytes  bigint      NOT NULL,
    storage_key text        NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wfe_note_file_note_idx ON wf.wfe_note_file(note_id);
