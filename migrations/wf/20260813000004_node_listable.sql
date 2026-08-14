-- ================================================================
-- Node-seviyesi görünürlük (2026-08-13) — `nodes.<key>.listable[]`in
-- projeksiyonu. `20260813000001_visibility_grants.sql`ın DEVAMIDIR:
-- orada kurulan "kararı commit anında hesapla, SQL'de containment ile
-- sor" düzeni aynen sürüyor, yalnız yeni bir ÖMÜR sınıfı ekleniyor.
--
-- Sorun: o migration'ın tanıdığı iki kolonun İKİSİ DE node listable'ı
-- yanlış anlatıyor —
--   * `wfe.view_c_a` (listable ∪ wf_admin) KALICIDIR, terminal'de
--     silinmez. Node listable oraya girerse aktör WFE o node'dan
--     çıktıktan SONRA da (hatta iş bittikten sonra da) görürdü; oysa
--     kural "WFE bu node'da İKEN görsün" diyor.
--   * `wfe.current_c_a` / `wfe_branch.c_a` = ACT ADAYLARIDIR. Node
--     listable oraya girerse claim/ACT yetkisi kazanır ve portal
--     havuzunda iş olarak listelenir; oysa kural yalnız GÖRME verir.
-- WOR-44'ün `listable` katlaması tam bu ikinci hatayı yapıyordu ve
-- 2026-08-13'te bu yüzden kaldırıldı — aynı hatayı node ekseninde
-- tekrar etmemek için kolon AYRI olmak zorunda.
--
-- Kural (`crates/wfe/src/visibility.rs::sql`, kriter (f)):
--     görünür(WFE, viewer) :=
--          view_c_a         @> viewer      -- KALICI grant (MEVCUT)
--       OR (status='active' AND (
--              current_c_a      @> viewer  -- tek-kol node c_a'sı
--           OR current_view_c_a @> viewer  -- tek-kol node listable'ı  [YENİ]
--           OR claimed_by      @> viewer
--           OR EXISTS(aktif kol:  b.c_a      @> viewer
--                              OR b.view_c_a @> viewer  -- kol node listable'ı [YENİ]
--                              OR b.claimed_by @> viewer)))
--
-- Yeni kolonlar `status='active'` KOLUNUN İÇİNDEDİR: durum-bağımlılığı
-- ifade eden şey budur. `current_c_a` ile AYNI ANDA yazılır ve
-- terminal/error/terminated'da onunla BİRLİKTE boşaltılır — node'dan
-- çıkan iş, o node'un görme hakkını da bırakır.
-- ================================================================

-- Node listable'ın çözülmüş aday listesi (`current_c_a` / `view_c_a` ile AYNI
-- biçim: CandidateActor[], `when` guard'ı UYGULANMIŞ, ORGTRVLANG çapası
-- `wfe.origin_orgu_id`). İki komşusundan farkları TAM OLARAK şunlardır:
--   * `view_c_a`dan farkı: KALICI DEĞİL — terminal'de boşaltılır.
--   * `current_c_a`dan farkı: ACT VERMEZ — havuz sorgusuna girmez, claim
--     kapısı bu kolonu hiç sormaz (yetki daima matcher'dan okunur).
ALTER TABLE wf.wfe
ADD COLUMN IF NOT EXISTS current_view_c_a jsonb NOT NULL DEFAULT '[]'::jsonb;

COMMENT ON COLUMN wf.wfe.current_view_c_a IS 'Aktif node''un listable[] grant''ları (çözülmüş CandidateActor[], when uygulanmış); current_c_a ile AYNI ANDA yazılır ve terminal''de BİRLİKTE boşaltılır. view_c_a''dan farkı KALICI OLMAMASI, current_c_a''dan farkı ACT VERMEMESİ';

-- Kol karşılığı: paralel modda `wfe.current_node` NULL'dır ve "aktif node"
-- kümesi kol satırlarıdır (bkz. `can_view` (c)/(f) aynı kümeyi paylaşır) —
-- yani tek-kol yolundaki `current_view_c_a`nın kol eşleniği. `wfe_branch.c_a`
-- NEREDE yazılıyorsa (fork'ta her kol, kol hareketinde hareket eden kol) bu da
-- orada yazılır; kol satırı düştüğünde onunla birlikte gider.
ALTER TABLE wf.wfe_branch
ADD COLUMN IF NOT EXISTS view_c_a jsonb NOT NULL DEFAULT '[]'::jsonb;

COMMENT ON COLUMN wf.wfe_branch.view_c_a IS 'Kolun node listable[] grant''ları — wf.wfe.current_view_c_a''nın kol karşılığı; wfe_branch.c_a ile aynı yerlerde yazılır, ACT VERMEZ';

-- Containment (@>) index'leri — komşu kolonlarla AYNI gerekçe ve aynı
-- adlandırma: jsonb_path_ops yalnız @> destekler, bu kolonlarda sorulan tek
-- soru da containment'tır (bkz. 20260813000001).
CREATE INDEX IF NOT EXISTS wfe_current_view_c_a_gin ON wf.wfe USING gin (current_view_c_a jsonb_path_ops);

CREATE INDEX IF NOT EXISTS wfe_branch_view_c_a_gin ON wf.wfe_branch USING gin (view_c_a jsonb_path_ops);
