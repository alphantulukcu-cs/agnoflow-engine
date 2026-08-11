-- Ek-belge METADATA tablosu (2026-08-11, K7).
--
-- Bugün bir dosyanın DB'de hiçbir kaydı yok; tek gerçeklik storage'daki nesne. Bunun üç
-- sonucu var: (1) kapı kontrolü her seferinde N adet storage `exists()` çağrısı, (2) "kim
-- ne zaman yükledi, hangi ad, hangi boyut" sorusunun cevabı yok, (3) attachments
-- sözleşmesinin "görünürlük DB'dedir" sözü teknik olarak boş — görünürlük hâlâ storage'a
-- bakıyor. Bu tablo kapı kontrolünü tek SQL'e indirir, audit'i bedavaya getirir.
--
-- FK + ON DELETE CASCADE bilinçli bir seçimdir, tasarımdaki "satırlar WFE'yi yaratan
-- transaction'ın İÇİNDE yazılır" sözünün TAM karşılığı değildir: o transaction `wf_wfe`
-- crate'inin `WfeStore::commit`'i içinde açılıp kapanıyor; `server` crate'i (bu tabloyu
-- yazan taraf) o transaction'a katılamıyor — iki crate arasında transaction sınırı
-- paylaştırmak (seam açmak) kararlaştırılmadı, gereksiz bir bağımlılık olurdu. Bunun
-- yerine değişmez bir FK ile aynı garanti başka yoldan kurulur: **satır varsa WFE
-- vardır** (FK ihlali insert'i reddeder), WFE silinince satırlar da CASCADE ile gider —
-- WFE'siz/hayalet metadata satırı hiçbir zaman oluşamaz, WFE'si silinmiş satır hiçbir
-- zaman kalıcı olamaz. Metadata yazımı WFE commit'inden SONRA, ayrı bir adımda olur
-- (`server::wfe_attachment::insert_many`); o adım başarısız olsa bile WFE gerçektir
-- (bkz. çağrı yerindeki NEDEN yorumu — metadata denetim/gösterim katmanıdır, kapı
-- DEĞİLDİR).
CREATE TABLE wf.wfe_attachment (
    wfe_id        uuid        NOT NULL REFERENCES wf.wfe(wfe_id) ON DELETE CASCADE,
    grp           text        NOT NULL,   -- katalog grup key'i
    item          text        NOT NULL,   -- slot id
    -- Aynı slota tekrar yükleme ÜZERİNE YAZMAZ, yeni sürüm açar — kapı daima EN YÜKSEK
    -- version'a bakar; denetimde "karar anında hangi belge oradaydı" cevaplanabilir kalır.
    version       integer     NOT NULL DEFAULT 1,
    storage_key   text        NOT NULL,   -- attachments/{wfe_id}/{grp}/{item}
    filename      text,                   -- kullanıcının verdiği ad (sanitize edilmiş)
    content_type  text        NOT NULL,
    size_bytes    bigint      NOT NULL,
    sha256        text        NOT NULL,
    uploaded_by   uuid        NOT NULL,
    uploaded_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (wfe_id, grp, item, version)
);
CREATE INDEX wfe_attachment_wfe_idx ON wf.wfe_attachment(wfe_id);
