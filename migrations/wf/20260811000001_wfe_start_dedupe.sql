-- Tek istekli başlatmada ÇİFT WFE koruması (2026-08-11, K6).
--
-- Sorun: tek istekli akışta gövde (ek-belgeler dahil) büyük olabilir, istek uzun sürer;
-- süre uzadıkça timeout/bağlantı kopması ihtimali artar. En kötü senaryo: WFE commit
-- oldu ama cevap istemciye ulaşmadı, kullanıcı "Başlat"a tekrar bastı → aynı başvuru
-- İKİNCİ kez oluşur. Standart çözüm istemcinin `Idempotency-Key` üretmesidir; bu tasarımda
-- İSTEMCİ HİÇBİR ŞEY GÖNDERMEZ (K4'ün sözü: UI yalnız dosya+girdi toplar, tek istek atar).
--
-- Bunun yerine anahtar isteğin KENDİSİNDEN türetilir (bkz. `server::start_dedupe::fingerprint`):
--   sha256(actor_user_id, wfd_id, version, action, canonical_json(input), canonical_json(attachments))
--
-- Bu tablo o türetilmiş anahtarın DEFTERİDİR:
--   fingerprint NULL wfe_id  → iş şu an koşuyor, satırı biri sahiplendi (InProgress → 409)
--   fingerprint dolu wfe_id  → iş DEDUPE_WINDOW içinde tamamlandı, sonuç TEKRAR verilir (Replay)
--
-- `created_at` yaşı DEDUPE_WINDOW'u (varsayılan 60 sn, `WFE_START_DEDUPE_WINDOW_SECS`) geçmiş
-- satır yok sayılır ve üzerine yazılır (satır çakışsa bile yeniden sahiplenme mümkün olsun).
-- Fiziksel temizlik ayrı bir TTL'dir (1 saat) ve mevcut saatlik süpürücüde yapılır
-- (rezervasyon + taslak not ile aynı tur, bkz. `server::reservation::sweep`).
CREATE TABLE wf.wfe_start_dedupe (
    fingerprint   text        PRIMARY KEY,  -- istekten türetilir, istemciden GELMEZ
    actor_user_id uuid        NOT NULL,
    wfe_id        uuid,                     -- NULL = iş hâlâ koşuyor (satırı sahiplenen henüz bitirmedi)
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX wfe_start_dedupe_created_idx ON wf.wfe_start_dedupe(created_at);
