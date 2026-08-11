-- Yükleme STAGING alanı — `POST /uploads` (2026-08-11, K8 / Faz 3).
--
-- Sorun: 500 MB'lık bir raporun baytlarının engine üzerinden (tek istekli `POST /wfe`
-- multipart yolu ya da rezervasyon yolu) geçmesi yanlış — bant genişliği, timeout ve
-- retry maliyeti engine'e biner, istemci tarayıcısı da tek isteğin tamamını taşımak
-- zorunda kalır. Çözüm: baytlar İSTEKTEN ÖNCE bir staging alanına konur; başlatma
-- isteğine yalnız tutamağı (`upload_id`) girer. Sunucu doğrulayıp nihai anahtara
-- server-side COPY eder (`crate::staging::take`) — dosya istemciye geri inip
-- tekrar yüklenmez.
--
-- Bu tablo staging'in DEFTERİDİR — `wf.wfe_reservation` ile AYNI gerekçe iki soruyu
-- cevaplar: (1) `take()` dosyayı hangi WFD'nin katalogına göre (grup/item) doğrulayıp
-- hangi depoya (bkz. aşağıda `environment_id`) bakacağını bilmeli, (2) süpürücü hangi
-- staging nesnelerinin sahipsiz kaldığını nereden bilecek (istemci `POST /uploads` alıp
-- hiç başlatmadan vazgeçebilir).
--
-- `environment_id` NEDEN gerekli: ek-belge deposu WFD BAŞINA `$env` ile çözülür
-- (2026-08-07, `crates/server/src/attachment_store.rs::store_for_wfd`) — bir akışın
-- belgeleri müşterinin S3'ünde, bir diğerininki sunucu diskinde durabilir, hatta aynı
-- WFD'de ortama göre (test/prod) değişebilir. Staging nesnesi NİHAİ anahtarla **AYNI**
-- depoda olmalı ki `take()` server-side `Operator::copy` yapabilsin — ayrı depoda olsaydı
-- taşıma indirip-yeniden-yükleme olurdu, staging'in tüm amacı (baytları bir kez taşımak)
-- boşa çıkardı. Ortam `POST /uploads` anında (rezervasyondaki gibi) SABİTLENİR; `take()`
-- aynı ortamla depoyu çözer.
--
-- Anahtar ailesi `staging/{upload_id}` — `attachments/{wfe_id}/{grup}/{item}` ve
-- `notes/{wfe_id}/{file_id}` köklerinden KASITLI olarak AYRI (bkz. `crate::staging`
-- modül başlığı): `AttachmentStore::remove_all` yalnız o iki kökü tarar; staging bu
-- ağaca karışsaydı henüz hiçbir WFE'ye bağlanmamış, yarım/vazgeçilmiş bir dosya
-- "yüklenmiş" sayılırdı.
--
-- Satır `take()` başarıyla tamamlanınca SİLİNİR (nesne nihai anahtara taşındı, staging
-- kopyası çöp). Başlatılmadan/tamamlanmadan bırakılan staging kayıtları saatlik
-- süpürücüyle (`crate::staging::sweep_expired`) dosyalarıyla birlikte temizlenir
-- (TTL 24 saat — `wf.wfe_reservation` ile AYNI süre, aynı gerekçe: kullanıcının
-- belgeleri toplayıp başlatana kadar geçen makul üst sınır).
CREATE TABLE wf.upload_staging (
    upload_id      uuid        PRIMARY KEY,
    orgtnt_id      uuid        NOT NULL,
    wfd_id         uuid        NOT NULL,
    wfd_version    integer     NOT NULL,
    -- `POST /uploads` anında seçilen ortam: `take()` depoyu bununla çözer. NULL =
    -- tenant varsayılanı (rezervasyondaki `environment_id` ile AYNI sözleşme).
    environment_id uuid,
    -- Hedef katalog slotu — dosya HANGİ grup/item'a ait, `take()` bunu nihai anahtarın
    -- (`attachments/{wfe_id}/{grup}/{item}`) parçası yapmak için kullanır. Sütun adı
    -- `grp`: `group` Postgres'te ayrılmış kelimedir (`wf.wfe_attachment` ile AYNI kural).
    grp            text        NOT NULL,
    item           text        NOT NULL,
    -- Yükleyen aktör — başka bir kullanıcının staging kaydına yazmayı/taşımayı engeller
    -- (`crate::staging::owned_by`, `wf.wfe_reservation`in `actor_orgu_id`/`actor_user_id`
    -- ile AYNI desen).
    actor_orgu_id  uuid        NOT NULL,
    actor_user_id  uuid        NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX upload_staging_created_idx ON wf.upload_staging(created_at);
