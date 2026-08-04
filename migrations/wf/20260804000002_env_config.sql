-- Ortam konfigürasyonu ($env) — docs/superpowers/specs/2026-08-04-env-config-design.md
--
-- Bir WFD bir kez tasarlanır, şirketin farklı ortamlarında (test/prod/uat) koşar. Ortama
-- göre değişen değerler ($env.AUTH_API, $env.MONGO_HOST, API anahtarları) burada durur.
--
-- Depolama kararı: değerler DB'de, şifreleme anahtarı deployment'ta (DB_CONN_SECRET, K8s
-- Secret + SOPS). GitLab'ın ci_variables + gitlab-secrets.json mimarisiyle aynı hizada:
-- DB dump'ı tek başına işe yaramaz.

-- ---------------------------------------------------------------------------
-- Ortam kaydı — tenant düzeyinde.
--
-- Değerler WFD başına olsa da ADIN kendisi ortak olmalı: yoksa bir WFD'nin 'prod'u ile
-- diğerinin 'Prod'u sessizce farklı ortamlar olur ve WFE'yi başlatan çağıran hangi adı
-- geçireceğini bilemez. is_default, "çağıran ortam geçirmezse ne olacak"ın cevabıdır.
--
-- orgtnt_id'de FK YOK — wf şemasının konvansiyonu bu (bkz. wf.wfe, wf.db_connection).
-- ---------------------------------------------------------------------------
CREATE TABLE wf.environment (
    id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL,
    name       text        NOT NULL CHECK (name ~ '^[a-z][a-z0-9_-]*$'),
    label      text,
    is_default boolean     NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX environment_orgtnt_idx ON wf.environment(orgtnt_id);

-- Tenant başına EN FAZLA bir varsayılan.
CREATE UNIQUE INDEX environment_one_default
    ON wf.environment (orgtnt_id) WHERE is_default;

-- ---------------------------------------------------------------------------
-- Değerler — mantıksal WFD başına.
--
-- Sahiplik anahtarı (project_id, wfd_name)'dir, wfd_id DEĞİL: wfd_meta'da her versiyon
-- ayrı bir wfd_id satırıdır, wfd_id'ye bağlamak yeni versiyonda conf'u koparırdı. Lokal
-- db_connection'ın kullandığı anahtarın aynısı (20260804000001_db_connection_scope.sql).
-- Grup adı değişince wfd_name taşınır, gruptaki son satır silinince bu satırlar da
-- temizlenir — wf_wfd::repo::update_group_metadata / delete_draft.
--
-- env_id IS NULL = GitLab'ın '*' joker kapsamı: anahtar tüm ortamlarda geçerlidir.
-- Çözüm sırası tam eşleşme > joker > tanımsız(hata).
--
-- key büyük harf zorunlu: $env.AUTH_API ile $ctx.auth_api gözle ayrılsın, ve enterpolasyon
-- token sınırı tartışmasız olsun ("$env.AUTH_API/v1/users" ilk küçük harfte biter).
--
-- value_type: GitLab her şeyi string tutar; bizde bu "$env.TIMEOUT_MS > 1000" zen
-- karşılaştırmasında "Compare: Unsupported type" üretirdi. Tip kolonu o sınıfı kapatır.
-- ---------------------------------------------------------------------------
CREATE TABLE wf.wfd_env_var (
    id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    project_id uuid        NOT NULL REFERENCES wf.project(project_id) ON DELETE CASCADE,
    wfd_name   text        NOT NULL,
    env_id     uuid        REFERENCES wf.environment(id) ON DELETE CASCADE,
    key        text        NOT NULL CHECK (key ~ '^[A-Z][A-Z0-9_]*$'),
    value_type text        NOT NULL DEFAULT 'string'
                           CHECK (value_type IN ('string','number','boolean')),
    value      text,
    value_enc  bytea,
    is_secret  boolean     NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    -- Secret satırda düz değer BULUNMAZ, şifreli değer ZORUNLU; tersi de geçerli.
    CONSTRAINT wfd_env_var_secret_check CHECK (
        (      is_secret AND value IS NULL AND value_enc IS NOT NULL) OR
        (NOT   is_secret AND value_enc IS NULL)
    ),
    -- Secret bir değer sayı/boolean olamaz: maskeleme string ikamesiyle çalışır.
    CONSTRAINT wfd_env_var_secret_is_string CHECK (NOT is_secret OR value_type = 'string')
);

-- NULLS NOT DISTINCT: joker satır (env_id IS NULL) da anahtar başına tek olsun.
-- Repo'da zaten kullanılan biçim (bkz. 20260717000002_template_kind.sql).
CREATE UNIQUE INDEX wfd_env_var_key
    ON wf.wfd_env_var (project_id, wfd_name, key, env_id) NULLS NOT DISTINCT;

CREATE INDEX wfd_env_var_owner ON wf.wfd_env_var (project_id, wfd_name);

-- ---------------------------------------------------------------------------
-- WFE'nin ortamı — start'ta yazılır, ÖMÜR BOYU değişmez.
--
-- Timer sweeper, tick_timers, retry, escalation ve SLA'da ortada bir çağıran YOKTUR;
-- $env'i o anlarda çözebilmenin tek yolu ortamın örneğin üstünde durmasıdır. Çağıran
-- yalnız ortam ADINI geçirir (değerleri değil — değer geçirmek, publish edilmiş bir prod
-- akışını başkasının sunucusuna yönlendirmeye izin veren bir enjeksiyon yüzeyi olurdu).
-- ---------------------------------------------------------------------------
ALTER TABLE wf.wfe ADD COLUMN environment_id uuid REFERENCES wf.environment(id);

-- ---------------------------------------------------------------------------
-- Seed + backfill: mevcut kurulumlar hiç değişmeden çalışmaya devam etsin.
--
-- Varsayılan ortam, org.orgtnt'nin yanı sıra wf tablolarında geçen TÜM orgtnt_id'ler için
-- açılır — aksi hâlde org.orgtnt'de karşılığı olmayan bir tenant'ın WFE'leri backfill'de
-- NULL kalır ve aşağıdaki NOT NULL patlar.
-- ---------------------------------------------------------------------------
INSERT INTO wf.environment (orgtnt_id, name, label, is_default)
SELECT DISTINCT orgtnt_id, 'default', 'Varsayılan', true
FROM (
    SELECT orgtnt_id FROM org.orgtnt
    UNION SELECT orgtnt_id FROM wf.wfe
    UNION SELECT orgtnt_id FROM wf.wfd_meta
    UNION SELECT orgtnt_id FROM wf.db_connection
) t
ON CONFLICT (orgtnt_id, name) DO NOTHING;

UPDATE wf.wfe e
   SET environment_id = env.id
  FROM wf.environment env
 WHERE env.orgtnt_id = e.orgtnt_id
   AND env.is_default
   AND e.environment_id IS NULL;

ALTER TABLE wf.wfe ALTER COLUMN environment_id SET NOT NULL;

-- ---------------------------------------------------------------------------
-- db_connection alanları $env ile şablonlanabilir.
--
-- Tek bağlantı satırı tüm ortamlara hizmet eder: host='$env.MONGO_HOST',
-- port='$env.MONGO_PORT', secret='$env.MONGO_PW'. WFD'de değişiklik yok (connection
-- UUID'si aynı kalır), db_connection'a ortam kolonu eklenmez.
--
-- port integer'dı; "tüm bağlantı alanları şablonlanabilir" tek cümlelik kural olsun diye
-- text'e çevriliyor. Çözümden sonra parse edilir; CHECK ya salt sayı ya da tam bir
-- $env.KEY şablonu olmasını garanti eder (enterpolasyonlu port anlamsız).
-- ---------------------------------------------------------------------------
ALTER TABLE wf.db_connection
    ALTER COLUMN port TYPE text USING port::text;

ALTER TABLE wf.db_connection
    ADD CONSTRAINT db_connection_port_check CHECK (
        port IS NULL OR port ~ '^[0-9]+$' OR port ~ '^\$env\.[A-Z][A-Z0-9_]*$'
    );
