-- Başlatma öncesi ek-belge yüklemesi — wfe_id REZERVASYONU (2026-08-07).
--
-- Sorun: dosya anahtarı `attachments/{wfe_id}/{grup}/{item}`, wfe_id ise akış başlarken
-- doğuyordu (executor::start_in). Yani "belgeler yüklenmeden akış başlamasın" kuralı
-- başlatma aksiyonunda SUNUCUDA zorlanamıyordu: ya WFE belgesiz doğuyor ya da kapı
-- istemciye bırakılıyordu.
--
-- Karar: wfe_id başlatmadan ÖNCE üretilir. Portal `POST /wfe/reserve` ile id alır,
-- dosyaları NİHAİ anahtarına yükler, sonra `POST /wfe` gövdesinde o id'yi geçirir.
-- Engine storage'a bakar; zorunlu belge eksikse WFE HİÇ oluşmaz (422). Dosya taşınmaz —
-- alternatif taslak alan (`attachments/draft/…`) her başlatmada kopyalama isterdi.
--
-- Bu tablo rezervasyonun DEFTERİDİR. Gerekçesi iki tane:
--   1. Yükleme rotası dosyayı hangi WFD'nin katalogına göre doğrulayacağını bilmeli
--      (grup/item gerçekten var mı) — rezerve edilmemiş rastgele bir uuid'ye yazılamaz.
--   2. Başlatılmayan rezervasyonlar sahipsiz dosya bırakır; süpürücü neyi sileceğini
--      ancak bir kayıttan bilebilir.
--
-- Satır start'ta SİLİNİR (wfe artık gerçek). Kalanları timer servisi süpürür.
CREATE TABLE wf.wfe_reservation (
    wfe_id         uuid        PRIMARY KEY,
    orgtnt_id      uuid        NOT NULL,
    wfd_id         uuid        NOT NULL,
    wfd_version    integer     NOT NULL,
    -- Rezervasyon anında seçilen ortam: yükleme rotası storage'ı bununla çözer
    -- (WFD başına $env storage konfigürasyonu). NULL = tenant varsayılanı.
    environment_id uuid,
    -- Rezerve eden aktör — başka bir kullanıcının rezervasyonuna yazmayı engeller.
    actor_orgu_id  uuid        NOT NULL,
    actor_user_id  uuid        NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX wfe_reservation_created_idx ON wf.wfe_reservation(created_at);
CREATE INDEX wfe_reservation_orgtnt_idx  ON wf.wfe_reservation(orgtnt_id);
