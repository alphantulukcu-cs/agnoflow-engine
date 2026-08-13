-- ================================================================
-- Görünürlük projeksiyonu (2026-08-13) — "kim bu WFE'yi görebilir"
-- sorusunun SQL'de KESİN cevaplanabilmesi için.
--
-- Sorun: görünürlük bugün yalnız `can_view` (wfe-core/v22/visibility.rs)
-- ile, satır satır, belge + org portu okunarak cevaplanıyor. Liste ucu bu
-- yüzden ya istemci tarafında satır başına bir `GET /wfe/:id` probuna
-- (N+1) ya da havuzun `current_c_a` YAKLAŞIKLIĞINA düşüyordu. İkisi de
-- ölçeklenmiyor ve ikincisi liste ile detayın farklı cevap vermesine yol
-- açıyor (havuz `listable[].when` guard'ını yok sayıyor, `wf_admin`'i hiç
-- bilmiyor).
--
-- Çözüm: kararın kendisi commit anında hesaplanıp DENORMALIZE edilir;
-- liste, detay ve havuz AYNI jsonb containment predicate'ini kullanır.
--
-- Kural (ürün kararı, 2026-08-13):
--     görünür(WFE, viewer) :=
--          view_c_a    @> viewer          -- listable ∪ wf_admin, KALICI
--       OR (status='active' AND (
--              current_c_a @> viewer      -- tek-kol node c_a'sı
--           OR wfe_branch.c_a @> viewer   -- paralel kol c_a'sı
--           OR claimed_by @> viewer       -- iş onun elinde
--           OR kol claim'i viewer'da))
--
-- İş BİTTİĞİNDE (terminal/error/terminated) `current_c_a` boşaltılır ve
-- geriye YALNIZ `view_c_a` kalır — yani bitmiş işi görme yetkisi tamamen
-- `listable`/`wf_admin` tasarımına bağlıdır. Bu bilinçli bir üründür
-- kararıdır: "ya listable'dadır ve görür, ya iş onun havuzundadır ve görür".
-- ================================================================

-- Kalıcı görünürlük grant'ları: `wfd.listable[]` ∪ `wfd.wf_admin[]` kurallarının
-- ÇÖZÜLMÜŞ aday listesi (`current_c_a` ile AYNI biçim: CandidateActor[]).
-- `current_c_a`dan iki farkı var: (1) `when` guard'ı UYGULANMIŞ olarak yazılır,
-- (2) terminal'de BOŞALTILMAZ.
ALTER TABLE wf.wfe
ADD COLUMN IF NOT EXISTS view_c_a jsonb NOT NULL DEFAULT '[]'::jsonb;

COMMENT ON COLUMN wf.wfe.view_c_a IS 'Kalıcı görünürlük grant''ları (listable ∪ wf_admin, when uygulanmış, çözülmüş CandidateActor[]); terminal''de SİLİNMEZ';

-- `listable`/`wf_admin` kurallarının ORGTRVLANG ÇAPASI.
--
-- Bu kurallardaki `c_orgu` bugüne kadar VIEWER'ın birimine çapalanıyordu
-- (`resolve_c_orgu`nun default_anchor'ı = soruyu soran aktör, resolver.rs:36).
-- Sonuç: `{"c_orgu":"self","c_r":["mudur"]}` fiilen "hangi birimde olursa olsun
-- mudur" demek oluyordu — birim karşılaştırması kendisiyle yapıldığı için hep
-- true. Tasarımcının yazdığı "kendi birimimdekiler görsün" cümlesi sessizce
-- tenant geneli role dönüşüyordu.
--
-- 2026-08-13 kararı: bu kuralların çapası artık WFE'nin KENDİ birimidir —
-- akışı başlatan aktörün birimi, WFE ömrü boyunca sabit. Böylece `self`
-- "işin ait olduğu birim", `parent` "o birimin üstü" anlamına gelir; kural
-- viewer'dan bağımsız hale gelir ve commit anında çözülüp yazılabilir.
-- KIRICI: anchor'a bağlı selector kullanan yayınlanmış akışlarda görünürlük
-- DARALIR (ölçüm: 2 WFD).
ALTER TABLE wf.wfe ADD COLUMN IF NOT EXISTS origin_orgu_id uuid;

COMMENT ON COLUMN wf.wfe.origin_orgu_id IS 'Akışı başlatan aktörün birimi; listable/wf_admin c_orgu ifadelerinin ORGTRVLANG çapası (ömür boyu sabit)';

-- Backfill kapısı: projeksiyonu olmayan satır SQL süzgecinde sessizce
-- kaybolur. Sunucu bu kolona bakıp "bu tenant henüz backfill edilmedi"
-- diyebilsin diye damga tutulur (bkz. `visibility_backfill` komutu).
ALTER TABLE wf.wfe
ADD COLUMN IF NOT EXISTS grants_built_at timestamptz;

COMMENT ON COLUMN wf.wfe.grants_built_at IS 'view_c_a/kol c_a projeksiyonunun en son yazıldığı an; NULL = henüz backfill edilmedi';

-- Paralel kolların c_a'sı bugün HİÇ cache'lenmiyor: portal havuzu her istekte
-- kol kol canlı çözüyor (routes/portal/pool.rs ikinci döngü) — kol başına org
-- sorgusu, yani gizli bir N+1. Tek-kol yolundaki `wf.wfe.current_c_a`nın kol
-- karşılığı.
ALTER TABLE wf.wfe_branch
ADD COLUMN IF NOT EXISTS c_a jsonb NOT NULL DEFAULT '[]'::jsonb;

COMMENT ON COLUMN wf.wfe_branch.c_a IS 'Kolun çözülmüş aday listesi (CandidateActor[]) — wf.wfe.current_c_a''nın kol karşılığı';

-- Containment (@>) index'leri. jsonb_path_ops seçildi: yalnız @> destekler ama
-- varsayılan jsonb_ops''tan belirgin küçük ve hızlıdır — bu kolonlarda sorulan
-- TEK soru containment'tır.
CREATE INDEX IF NOT EXISTS wfe_view_c_a_gin ON wf.wfe USING gin (view_c_a jsonb_path_ops);

CREATE INDEX IF NOT EXISTS wfe_current_c_a_gin ON wf.wfe USING gin (current_c_a jsonb_path_ops);

CREATE INDEX IF NOT EXISTS wfe_claimed_by_gin ON wf.wfe USING gin (claimed_by jsonb_path_ops);

CREATE INDEX IF NOT EXISTS wfe_branch_c_a_gin ON wf.wfe_branch USING gin (c_a jsonb_path_ops);

-- Sayfalama: liste `orgtnt_id` süzüp `created_at DESC` sıralıyor (repo::wfe::
-- list_by_tenant). OFFSET'li sayfalamanın sıralama adımını index'ten okuması için.
CREATE INDEX IF NOT EXISTS wfe_tenant_created_idx ON wf.wfe (orgtnt_id, created_at DESC);

-- Katılımcı sorgusu ARTIK GÖRÜNÜRLÜK KURALI DEĞİL (kriter (b) kaldırıldı), ama
-- WFAH aktörüne göre arama başka yerlerde de yapılıyor; kural değişiminin geri
-- alınması gerekirse EXISTS sorgusunun index'i hazır olsun.
CREATE INDEX IF NOT EXISTS wfah_actor_user_idx ON wf.wfah ((actor ->> 'user_id'));