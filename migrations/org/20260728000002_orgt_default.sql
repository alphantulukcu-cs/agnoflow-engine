-- Tenant başına çoklu org ağacı: "varsayılan" salt UI/bootstrap kolaylığıdır — motor
-- (traversal/yetkilendirme) her zaman anchor node'un kendi ağacını çözer, is_default'a
-- hiç bakmaz.

ALTER TABLE org.orgt ADD COLUMN is_default boolean NOT NULL DEFAULT false;

-- Geriye dönük uyumluluk: her tenant'ın bugün var olan (en eski) aktif ağacı varsayılan olsun.
UPDATE org.orgt o
SET is_default = true
WHERE o.orgt_id = (
    SELECT o2.orgt_id FROM org.orgt o2
    WHERE o2.orgtnt_id = o.orgtnt_id AND o2.is_active = true
    ORDER BY o2.created_at ASC LIMIT 1
);

-- Tenant başına en fazla bir varsayılan — DB seviyesinde garanti.
CREATE UNIQUE INDEX orgt_one_default_per_tenant
    ON org.orgt (orgtnt_id) WHERE is_default = true;
