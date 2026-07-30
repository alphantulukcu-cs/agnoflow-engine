-- WFC — İş Akışı Çağrısı (Workflow Call). Plan: docs/plans/workflow-call.md,
-- kararlar: docs/spec/decisions.md → WFC.
--
-- Bir WFE'nin başka bir WFD'yi çalıştırması. Üç mod tek tabloyu paylaşır:
--   wait      alt akış, çağıran node'da BEKLER    → sonuç $call.* ile döner
--   detached  alt akış, çağıran devam eder        → dönüş yok
--   terminal  ardıl akış, çağıran BİTER           → dönüş yok
--
-- Tek tablo olması bilinçli: outbox, sweeper taramaları ve idempotency mantığı
-- üçünde de aynıdır; ayırmak aynı kodu üç kez yazdırırdı.

-- ---------------------------------------------------------------------------
-- 1) wfd_meta: doküman kimliğini indeksle
-- ---------------------------------------------------------------------------
-- `calls.<key>.wfd_id` çağrılan WFD'nin DOKÜMAN `id`'sine, `version` ise doküman
-- semver'ine atıfta bulunur. Tablo şimdiye kadar WFD'yi yalnız
-- (orgtnt_id, name, integer version) ile indeksliyordu — yani bir çağrıyı çözmek
-- için tenant'ın TÜM WFD JSON'larını okumak gerekirdi. Bu iki kolon o taramayı
-- tek indeksli sorguya indirir.
--
-- NULL bırakılan eski satırlar çağrılamaz durumda kalır (yeniden yayınlanınca
-- dolar) — sessiz yanlış eşleşmeye yeğ tutulur.
ALTER TABLE wf.wfd_meta ADD COLUMN IF NOT EXISTS doc_id      text;
ALTER TABLE wf.wfd_meta ADD COLUMN IF NOT EXISTS doc_version text;

CREATE INDEX IF NOT EXISTS wfd_meta_doc_idx
    ON wf.wfd_meta (orgtnt_id, doc_id, doc_version);

-- ---------------------------------------------------------------------------
-- 2) wfe_call: çağrı outbox'ı + çağıran↔çağrılan bağı
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS wf.wfe_call (
    id             uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id      uuid        NOT NULL,
    caller_wfe_id  uuid        NOT NULL REFERENCES wf.wfe(wfe_id),

    -- Çağrının yapıldığı yer. 'node' → çağrı node'unun slug'ı,
    -- 'terminal' → terminal id'si. Mod ile birlikte "nasıl çağrıldı"yı tamamlar.
    site_kind      text        NOT NULL CHECK (site_kind IN ('node','terminal')),
    site_key       text        NOT NULL,

    call_key       text        NOT NULL,              -- calls.<key>
    mode           text        NOT NULL CHECK (mode IN ('wait','detached','terminal')),

    callee_wfe_id  uuid        NULL REFERENCES wf.wfe(wfe_id),

    -- queued   : commit ile aynı tx'te yazıldı, çağrılan henüz başlatılmadı
    -- running  : çağrılan başlatıldı
    -- returned : çağrılan bitti, çağıranın işlemesi bekleniyor (yalnız wait)
    -- consumed : dönüş işlendi (ya da wait olmayan modda başlatma tamamlandı)
    -- failed   : başlatılamadı / çağrılan hata verdi
    -- cancelled: WFC-CASCADE ya da süre sınırı ile iptal
    -- skipped  : derinlik sınırı aşıldı — başlatılmadı (çağıran etkilenmez)
    status         text        NOT NULL CHECK (status IN
                       ('queued','running','returned','consumed','failed','cancelled','skipped')),

    -- WFC-IN: çağıranın ctx'ine göre ÇÖZÜLMÜŞ girdi (çağrılanın start ACT input'u).
    input          jsonb       NOT NULL DEFAULT '{}',

    -- `wait` süre sınırı, mutlak zamana çevrilmiş (SLA-3 deadline'ıyla aynı
    -- gerekçe: her tick'te ISO parse etmemek).
    deadline       timestamptz NULL,

    -- Yalnız terminal modu.
    start_as       text        NOT NULL DEFAULT 'actor' CHECK (start_as IN ('actor','system')),
    max_next       integer     NULL CHECK (max_next IS NULL OR max_next >= 1),

    -- WFC-OUT — yalnız mode='wait' doldurulur.
    end_response   jsonb       NULL,
    call_status    text        NULL CHECK (call_status IS NULL OR call_status IN
                       ('completed','failed','terminated','timeout','started')),

    -- İKİ AYRI sayaç: frenleri de ayrı. `depth` alt akış yuvalanması (cap 8),
    -- `next_depth` ardıl zinciri (cap 16 ya da max_next). Ardıl döngüsü
    -- (A bitince B, B bitince A) sonsuz WFE üretir — bu kolon runtime frenidir.
    depth          integer     NOT NULL DEFAULT 0,
    next_depth     integer     NOT NULL DEFAULT 0,

    created_at     timestamptz NOT NULL DEFAULT now(),
    returned_at    timestamptz NULL,

    -- Çift start koruması (idempotent outbox). Terminal'de doğal olarak tekil —
    -- bir WFE bir kez biter. Node'da aynı çağrı node'una ikinci giriş bu kısıt
    -- yüzünden engellenir; yeniden girişe izin verilecekse `attempt` kolonu
    -- eklenip UNIQUE dörtlüye çıkarılır (bkz. plan §9.2).
    UNIQUE (caller_wfe_id, site_kind, site_key)
);

-- Outbox/dönüş taramaları: (status, deadline) ile hem `queued`/`returned` kuyruğu
-- hem süresi geçmiş `running` satırlar tek indeksten okunur.
CREATE INDEX IF NOT EXISTS wfe_call_status_idx   ON wf.wfe_call (status, deadline);
CREATE INDEX IF NOT EXISTS wfe_call_callee_idx   ON wf.wfe_call (callee_wfe_id);
CREATE INDEX IF NOT EXISTS wfe_call_caller_idx   ON wf.wfe_call (caller_wfe_id);
