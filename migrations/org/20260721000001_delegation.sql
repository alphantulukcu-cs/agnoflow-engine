-- ================================================================
-- Madde 6: Vekalet / delegasyon (delegation).
--
-- Bir kullanıcı (delegator) izne/tatile çıkarken TEK bir koltuğunun
-- (seat = orgu + rol) claim eligibility'sini ve görünürlüğünü geçici olarak bir
-- ALICIYA (grantee) devreder. Grantee bir CandidateActor'dır (JSONB): belirli bir
-- kişi (c_u) VEYA bir havuz (c_orgu + c_r). "Tüm koltuklarım" grant anında birden
-- çok satıra genişler (her koltuk = bir satır).
--
-- Yetki kararı (wfe-core matcher `authorize_with_delegation`): önce doğrudan
-- eşleşme; olmazsa claimant'a aktif vekalet veren her satır için (a) claimant
-- grantee'ye uyuyor mu VE (b) delegator'ın koltuğu node c_a'sına uyuyor mu.
-- Zincirleme (transitif) vekalet YOKTUR — tek seviye.
-- ================================================================

-- Migration tek başına koşabilmeli (org/initial ile aynı gerekçe).
-- NOT: uuid_generate_v4() (uuid-ossp) yerine çekirdek gen_random_uuid() (PG13+,
-- pg_catalog) kullanılır — eklenti/search_path bağımlılığı yoktur; tek bir CREATE
-- TABLE ifadesi izole çalıştırılsa bile çözülür.
CREATE SCHEMA IF NOT EXISTS org;

CREATE TABLE IF NOT EXISTS org.delegation (
    delegation_id     uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id         uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    -- Şapka sahibi: c_u node'ları için kimlik + audit + koltuk sahipliği doğrulaması.
    delegator_user_id uuid        NOT NULL REFERENCES org.u(u_id),
    -- Delege edilen koltuk (seat): orgu + rol adı (rol adları c_a.c_r ile aynı uzay).
    seat_orgu_id      uuid        NOT NULL REFERENCES org.orgu(orgu_id),
    seat_role         text        NOT NULL,
    -- Alıcı: CandidateActor JSONB — kişi ({c_u:[...]}) VEYA havuz ({c_orgu, c_r}).
    grantee           jsonb       NOT NULL,
    valid_from        timestamptz NOT NULL DEFAULT now(),
    valid_to          timestamptz NOT NULL,
    active            boolean     NOT NULL DEFAULT true,
    -- self-service'te = delegator_user_id; amir/admin atamasında = atayan aktör.
    created_by        uuid        NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    CHECK (valid_to > valid_from)
);

-- Aktif vekalet aday taraması: tenant + zaman + active ile daraltılır; alıcı
-- eşleşmesi (grantee JSONB) SQL'de DEĞİL matcher'da yapılır (havuz olabilir).
CREATE INDEX IF NOT EXISTS delegation_active_idx
    ON org.delegation (orgtnt_id, active, valid_from, valid_to);
-- "bana verilen / benim verdiğim" listeleri için.
CREATE INDEX IF NOT EXISTS delegation_delegator_idx
    ON org.delegation (delegator_user_id);

COMMENT ON TABLE org.delegation IS
    'Madde 6: koltuk-bazlı vekalet; grantee CandidateActor (kişi veya havuz); tek seviye (zincir yok)';
COMMENT ON COLUMN org.delegation.grantee IS
    'CandidateActor JSONB: {c_u:[...]} (kişi) veya {c_orgu, c_r:[...]} (havuz)';
COMMENT ON COLUMN org.delegation.seat_role IS
    'Delege edilen koltuğun rol adı (c_a.c_r ile aynı uzay); delegator bu koltuğu gerçekten taşımalı (grant anında doğrulanır)';
