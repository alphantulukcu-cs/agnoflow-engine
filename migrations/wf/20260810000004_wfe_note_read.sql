-- WFE not okundu takibi — Faz 3 (K9, kişi bazlı), 2026-08-10.
-- docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md
--
-- Faz 1'de liste ekranlarında kayıt tutmayan basit bir SAYAÇ (`note_count`)
-- yeterliydi: memur listeye bakınca notun varlığını görürdü. Havuz rozeti artık
-- "okunmamış"a döndüğü için (kaç not var değil, kaç YENİ not var) kişi bazlı bir
-- iz gerekiyor — sayaç tek başına "okudum ama sayı hâlâ orada" karışıklığı üretir.
--
-- `user_id` TEK sütun (org birimi yok): okundu bilgisi kişiye aittir, hangi
-- birimden okuduğuna bağlı değildir — `wf.wfe_note`'un yazar kimliğinin
-- (`author_orgu_id` + `author_user_id`) aksine burada birim ekseni yok.
--
-- `ON DELETE CASCADE`: not gizlenirse (`hidden_at`) okundu izi kalır (gizleme
-- UPDATE'tir, silme değil); not DRAFT iken vazgeçilip silinirse (`notes::hide`)
-- ya da yetim taslak süpürücüyle temizlenirse (`notes::sweep_expired_drafts`)
-- okundu satırları da onunla gider — zaten draft'ın okundu kaydı olamaz
-- (yalnız published notlar işaretlenebilir, bkz. `notes::mark_read`).
--
-- Ayrı sayaç tablosu/trigger YOK: "kaç okunmamış" sorusu `wf.wfe_note` LEFT JOIN
-- `wf.wfe_note_read` ile TEK sorguda hesaplanır (`notes::unread_count_by_wfe`,
-- `wf_wfe::repo::wfah::max_seq_by_wfe` ile aynı N+1'siz desen) — WFE listesi
-- boyutu küçük, denormalize sayaç bakımı (yazma yolunda +1/-1) karmaşıklığa
-- değmez.
CREATE TABLE wf.wfe_note_read (
    note_id uuid        NOT NULL REFERENCES wf.wfe_note(note_id) ON DELETE CASCADE,
    user_id uuid        NOT NULL,
    read_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (note_id, user_id)
);
