-- ================================================================
-- Tenant permission havuzu + rol = permission grubu (T‑A1, T‑A2).
-- Tasarım: docs/superpowers/specs/2026-08-11-tenant-permission-rol-modeli-design.md
--
-- agnoflow permission'ın ANLAMINI bilmez: "1043" ya da KREDI_ONAY neyi açar,
-- tenant'ın kendi uygulaması bilir. Bu şema yalnız saklar, dağıtır, cevaplar.
-- Motor (wfe-core) bu katmandan habersizdir: c_a / c_r modeli DEĞİŞMEZ.
--
-- Migration tek başına ve tekrar tekrar koşabilmeli (idempotent).
-- ================================================================
CREATE SCHEMA IF NOT EXISTS org;

-- 1) Havuz. code numara ("1043") da olabilir, isim (KREDI_ONAY) da.
--    code bir MAKİNE kimliğidir, gösterim metni değil (o display_name'de, serbest).
--    Bu yüzden ASCII harf/rakam + . _ : - ile sınırlı:
--      · boşluk yasak — dış uygulama listeyi boşlukla/virgülle ayırıp bölebilir,
--        içinde boşluk olan kod sessizce ikiye ayrılırdı;
--      · Türkçe harf yasak — benzersizlik lower(code) üzerinde ve PostgreSQL'in
--        lower()'ı ile Rust'ın to_lowercase()'i 'İ' üzerinde AYRIŞIR (libc noktayı
--        düşürür, Rust birleştirici nokta bırakır). Havuzda benzersiz sayılan iki
--        kod, /ext/permissions/check karşılaştırmasında farklı görünürdü.
CREATE TABLE IF NOT EXISTS org.p (
    p_id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id    uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    code         text        NOT NULL,
    display_name text        NOT NULL,
    description  text,
    is_active    boolean     NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT p_code_format CHECK (code ~ '^[A-Za-z0-9._:-]{1,128}$')
);
-- Harf duyarsız benzersizlik: KREDI_ONAY ile kredi_onay aynı yetkiyi anlatır,
-- ikisinin birlikte var olması dış uygulamanın hangisini yazdığını hatırlamasını
-- gerektirirdi.
CREATE UNIQUE INDEX IF NOT EXISTS p_code_unique  ON org.p (orgtnt_id, lower(code));
CREATE INDEX        IF NOT EXISTS p_orgtnt_idx   ON org.p (orgtnt_id);

-- 2) Rol = permission grubu. Atamalar p_id'ye bağlı → code yeniden adlandırılabilir.
CREATE TABLE IF NOT EXISTS org.rp (
    rp_id      uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    r_id       uuid        NOT NULL REFERENCES org.r(r_id),
    p_id       uuid        NOT NULL REFERENCES org.p(p_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (r_id, p_id)
);
CREATE INDEX IF NOT EXISTS rp_r_idx ON org.rp(r_id);
CREATE INDEX IF NOT EXISTS rp_p_idx ON org.rp(p_id);

-- 3) T‑A2: kişisel ıskarta. "Ahmet memur ama onda 1043 olmasın."
--    up_type CHECK'i şimdilik yalnız 'excluded': tablo şekli org.ur'nin aynısı
--    (ileride kişiye doğrudan grant istenirse yer var) ama tasarlanmamış
--    semantiği DB kabul etmez.
CREATE TABLE IF NOT EXISTS org.up (
    up_id       uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id   uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    u_id        uuid        NOT NULL REFERENCES org.u(u_id),
    p_id        uuid        NOT NULL REFERENCES org.p(p_id),
    up_type     text        NOT NULL DEFAULT 'excluded'
                CHECK (up_type IN ('excluded')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (u_id, p_id, up_type)
);
CREATE INDEX IF NOT EXISTS up_u_idx ON org.up(u_id);
CREATE INDEX IF NOT EXISTS up_p_idx ON org.up(p_id);

-- 4) Tenant kapsamlı salt-okuma API anahtarı (/ext ağacı).
--    Küresel ADMIN_API_KEY dış uygulamaya verilemez: o anahtar TÜM tenant'larda
--    tam org YAZMA yetkisi verir. Bu anahtar TEK tenant + salt okuma.
--    Aynı tenant'ta birden çok aktif satır = rotasyon (DB_CONN_SECRET'ın virgüllü
--    liste yaklaşımıyla aynı mantık).
CREATE TABLE IF NOT EXISTS org.orgtnt_api_key (
    key_id       uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id    uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    name         text        NOT NULL,
    prefix       text        NOT NULL UNIQUE,
    key_hash     text        NOT NULL,
    is_active    boolean     NOT NULL DEFAULT true,
    expires_at   timestamptz,
    last_used_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS orgtnt_api_key_orgtnt_idx ON org.orgtnt_api_key(orgtnt_id);
