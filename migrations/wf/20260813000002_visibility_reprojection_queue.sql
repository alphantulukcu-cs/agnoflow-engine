-- ================================================================
-- Görünürlük yeniden-projeksiyon kuyruğu (2026-08-13)
--
-- Grant'lar ORGTRVLANG selector'larını (`self`, `parent`, `*:[type:x]`) SOMUT
-- `orgu_id` kümesine çözüp donduruyor (bkz. wf.wfe.view_c_a). Org AĞACI
-- değişirse — birim eklenir, adı/tipi değişir, taşınır, pasifleşir — o küme
-- eskir ve görünürlük sessizce yanlışa kayar.
--
-- Rol ATAMASI bu kuyruğa GİRMEZ: satırlar kullanıcı değil (birim, rol) çifti
-- tutuyor, dolayısıyla role sonradan atanan kişi işi ANINDA görür.
--
-- Neden kuyruk: org mutasyonu ucu tenant'ın tüm WFE'lerini senkron yeniden
-- projelendiremez (büyük tenant'ta istek dakikalar sürer, hata hâlinde yarım
-- kalır). Uç yalnız "bu tenant bayatladı" der; işi saatlik süpürücü partiler
-- hâlinde yapar ve ilerlemeyi `wf.wfe.grants_built_at` damgasında tutar.
--
-- Tenant başına EN FAZLA BİR bekleyen satır: aynı bakım penceresinde 50 birim
-- taşınırsa 50 tarama değil bir tarama olur (`requested_at` en son isteğe çekilir).
-- ================================================================

CREATE TABLE IF NOT EXISTS wf.visibility_reprojection (
    orgtnt_id    uuid        PRIMARY KEY,
    requested_at timestamptz NOT NULL DEFAULT now(),
    -- Neden bayatladı (log/teşhis için; kararı etkilemez).
    reason       text        NOT NULL,
    -- Süpürücünün ilerlemesi: bu damgadan ESKİ projeksiyonlar yeniden üretilir.
    -- İş bittiğinde satır SİLİNİR.
    started_at   timestamptz
);

COMMENT ON TABLE wf.visibility_reprojection IS
    'Org ağacı değişince görünürlük projeksiyonunun yeniden üretilmesi gereken tenant''lar; tenant başına tek satır';
