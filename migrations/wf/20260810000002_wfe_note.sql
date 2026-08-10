-- WFE not defteri — Faz 1 (SADECE METİN), 2026-08-10.
-- docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md
--
-- Not/belge insan-üretimi içerik olduğu için ne motorun context'ine (`$ctx`)
-- ne resmi defterine (`wf.wfah`, `$wfah` olarak ZEN'e akar) yazılır (K1): araya
-- sistem satırı koymak `count($wfah, ...)` gibi sayımları kaydırır, motorun
-- defterine insan yorumu karışmaz. Bu yüzden örnek (WFE) bazlı, şemasız, AYRI
-- bir tablo — engine core bu katmandan habersizdir, tüm iş `server` crate'inde
-- (attachments'ın bugün yaptığı gibi).
--
-- K3 — yayınlanmış not DEĞİŞTİRİLEMEZ: karar delilidir ("müdür yükselt dedi").
-- Publish sonrası `body` üzerinde UPDATE yoktur; silme yerine gizleme
-- (`hidden_at`/`hidden_by` dolar, gövde DB'de kalır, API `{hidden:true}` döner).
-- Draft aşamasında (yalnız yazarı görür) serbestçe düzenlenir/silinir.
--
-- K5 — draft → publish deseni: `POST /wfe/:id/notes` draft yaratır (yalnız
-- yazarı görür) → aksiyonla (`POST /wfe/:id/actions` body'sinde `note_id`) ya da
-- serbest (`POST .../notes/:note_id/publish`) yayınlanır. `wfah_seq`/`node`
-- yayınlama anında doldurulur — `node` geçişin `from_node`'udur (notun
-- yazıldığı adım), `wfah_seq` motorun defterine çapadır (K7, `wf.wfah.seq`).
-- Yetim draft'lar mevcut saatlik süpürücüyle (`server/src/reservation.rs`)
-- TTL 24 saatte temizlenir.
--
-- K6 — yetki: not, bağlı olduğu WFE'nin görünürlüğünü miras alır
-- (`executor.query(wfe_id, actor)` — attachment rotalarının kullandığı kapının
-- aynısı). Ayrı bir yetki modeli icat edilmiyor.
--
-- K9 — `audience jsonb` kolonu bu fazda şemaya girer (sonradan migration
-- gerekmesin) ama süzgeç Faz 3'e kadar UYGULANMAZ; bugün `{"kind":"all"}`
-- varsayılanıyla "published olan + kendi draft'ların" listelenir.
--
-- `wf.wfe_note_file` (Faz 2, belge iliştirme) ve `wf.wfe_note_read` (Faz 3,
-- kişi bazlı okundu takibi) BİLEREK bu migration'a girmiyor — sonraki fazlar.
CREATE TABLE wf.wfe_note (
    note_id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id          uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    orgtnt_id       uuid        NOT NULL,
    author_orgu_id  uuid        NOT NULL,
    author_user_id  uuid        NOT NULL,
    author_role     text        NOT NULL,
    body            text        NOT NULL,
    -- Yazıldığı/yayınlandığı andaki adım; paralelde kol node'u.
    node            text,
    -- Motorun defterine çapa: bu not hangi aksiyonla gitti. NULL = serbest not.
    wfah_seq        integer,
    audience        jsonb       NOT NULL DEFAULT '{"kind":"all"}',
    status          text        NOT NULL CHECK (status IN ('draft','published')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    published_at    timestamptz,
    hidden_at       timestamptz,
    hidden_by       uuid
);
CREATE INDEX wfe_note_wfe_idx    ON wf.wfe_note(wfe_id);
CREATE INDEX wfe_note_author_idx ON wf.wfe_note(author_user_id) WHERE status = 'draft';
