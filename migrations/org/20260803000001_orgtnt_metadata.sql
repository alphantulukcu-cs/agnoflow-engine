-- Kurumsal tenant metadata'sı + marka varlıkları (logo/favicon).
--
-- Tasarım: sık sorgulanan/raporlanan alanlar TİPLİ kolon olur (CHECK ve index
-- konabilsin), şema değiştirmeden eklenecek serbest tercihler `settings` jsonb'de
-- yaşar. Marka varlıklarının BAYT'ları DB'de DEĞİL, WFD JSON ile aynı
-- tenant-prefixli object storage'da tutulur (`{orgtnt_id}/logo/{slot}.{ext}`);
-- burada yalnız storage anahtarı + mime + zaman damgası saklanır. Anahtarın
-- tamamı saklanır ki uzantı değiştiren yeniden yüklemede eski blob silinebilsin.

ALTER TABLE org.orgtnt
    -- kimlik / marka
    ADD COLUMN display_name text,
    ADD COLUMN brand_color  text,
    -- yasal / mali
    ADD COLUMN legal_name text,
    ADD COLUMN tax_no     text,
    ADD COLUMN tax_office text,
    -- iletişim
    ADD COLUMN contact_email text,
    ADD COLUMN contact_phone text,
    ADD COLUMN website       text,
    ADD COLUMN address       text,
    ADD COLUMN city          text,
    ADD COLUMN country       text,
    -- yerelleştirme (mevcut tenant'lar için varsayılan; NOT NULL)
    ADD COLUMN timezone text NOT NULL DEFAULT 'Europe/Istanbul',
    ADD COLUMN locale   text NOT NULL DEFAULT 'tr',
    ADD COLUMN currency text NOT NULL DEFAULT 'TRY',
    -- entegrasyon: ERP/CRM eşleştirme anahtarı
    ADD COLUMN external_id text,
    -- marka varlıkları (storage referansı)
    ADD COLUMN logo_key           text,
    ADD COLUMN logo_mime          text,
    ADD COLUMN logo_updated_at    timestamptz,
    ADD COLUMN favicon_key        text,
    ADD COLUMN favicon_mime       text,
    ADD COLUMN favicon_updated_at timestamptz,
    -- şema değişmeden eklenebilen tercihler
    ADD COLUMN settings jsonb NOT NULL DEFAULT '{}'::jsonb;

-- Boş string YOK: API katmanı "" → NULL normalize eder, DB de bunu garanti eder.
ALTER TABLE org.orgtnt
    ADD CONSTRAINT orgtnt_brand_color_hex
        CHECK (brand_color IS NULL OR brand_color ~ '^#[0-9A-Fa-f]{6}$'),
    ADD CONSTRAINT orgtnt_country_iso2
        CHECK (country IS NULL OR country ~ '^[A-Z]{2}$'),
    ADD CONSTRAINT orgtnt_currency_iso4217
        CHECK (currency ~ '^[A-Z]{3}$'),
    ADD CONSTRAINT orgtnt_locale_bcp47_lite
        CHECK (locale ~ '^[a-z]{2}(-[A-Z]{2})?$'),
    ADD CONSTRAINT orgtnt_contact_email_shape
        CHECK (contact_email IS NULL OR contact_email ~ '^[^@[:space:]]+@[^@[:space:]]+\.[^@[:space:]]+$'),
    ADD CONSTRAINT orgtnt_settings_is_object
        CHECK (jsonb_typeof(settings) = 'object'),
    -- Varlık kolonları ya birlikte dolu ya birlikte boş — yarı yazılmış durum olmasın.
    ADD CONSTRAINT orgtnt_logo_complete
        CHECK ((logo_key IS NULL) = (logo_mime IS NULL)
           AND (logo_key IS NULL) = (logo_updated_at IS NULL)),
    ADD CONSTRAINT orgtnt_favicon_complete
        CHECK ((favicon_key IS NULL) = (favicon_mime IS NULL)
           AND (favicon_key IS NULL) = (favicon_updated_at IS NULL)),
    -- Boş metin girilmesin (normalize edilmiş alanlar).
    ADD CONSTRAINT orgtnt_no_blank_text
        CHECK (btrim(name) <> '' AND btrim(code) <> ''
           AND (display_name  IS NULL OR btrim(display_name)  <> '')
           AND (legal_name    IS NULL OR btrim(legal_name)    <> '')
           AND (tax_no        IS NULL OR btrim(tax_no)        <> '')
           AND (tax_office    IS NULL OR btrim(tax_office)    <> '')
           AND (contact_phone IS NULL OR btrim(contact_phone) <> '')
           AND (website       IS NULL OR btrim(website)       <> '')
           AND (address       IS NULL OR btrim(address)       <> '')
           AND (city          IS NULL OR btrim(city)          <> '')
           AND (external_id   IS NULL OR btrim(external_id)   <> ''));

-- ERP/CRM eşleştirmesi kurulum genelinde tek olmalı; NULL'lar serbest.
CREATE UNIQUE INDEX orgtnt_external_id_unique
    ON org.orgtnt (external_id) WHERE external_id IS NOT NULL;
