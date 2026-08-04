-- wfe.environment_id NOT NULL sıkılaştırması.
--
-- 20260804000002 kolonu nullable bırakmıştı: start yolu henüz yazmıyordu, NOT NULL o an
-- yeni WFE oluşturmayı kırardı. Artık `WfeExecutor::start_in` ortamı çözüp satıra yazıyor
-- (ve WFC'de çocuk ebeveynin ortamını miras alıyor), dolayısıyla kapatılabilir.
--
-- SIRA ÖNEMLİ: bu migration UYGULAMA GÜNCELLENDİKTEN SONRA koşmalıdır. Eski bir sürüm
-- hâlâ ayaktayken uygulanırsa o sürümün açtığı WFE'ler NOT NULL'a takılır.

-- Bu arada NULL yazılmış satırları (varsayılan ortama düşenler) doldur.
INSERT INTO wf.environment (orgtnt_id, name, label, is_default)
SELECT DISTINCT e.orgtnt_id, 'default', 'Varsayılan', true
FROM wf.wfe e
WHERE e.environment_id IS NULL
  AND NOT EXISTS (SELECT 1 FROM wf.environment x WHERE x.orgtnt_id = e.orgtnt_id)
ON CONFLICT (orgtnt_id, name) DO NOTHING;

UPDATE wf.wfe e
   SET environment_id = env.id
  FROM wf.environment env
 WHERE env.orgtnt_id = e.orgtnt_id
   AND env.is_default
   AND e.environment_id IS NULL;

-- Varsayılanı olmayan ama başka ortamı olan tenant'lar için son çare: herhangi biri.
UPDATE wf.wfe e
   SET environment_id = (
       SELECT x.id FROM wf.environment x WHERE x.orgtnt_id = e.orgtnt_id ORDER BY x.name LIMIT 1
   )
 WHERE e.environment_id IS NULL;

ALTER TABLE wf.wfe ALTER COLUMN environment_id SET NOT NULL;

DROP INDEX IF EXISTS wf.wfe_environment_idx;
CREATE INDEX wfe_environment_idx ON wf.wfe(environment_id);
