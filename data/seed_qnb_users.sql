-- ================================================================
-- QNB Simülasyon Kullanıcıları + Rol Atamaları (u / ur / u_orgu)
-- ================================================================
-- seed_qnb_rbac.sql'in bahsettiği "ayrıca oluşturulacak kullanıcı
-- seed dosyası". Simülasyon konsolu için "Şubeler Ağacı"ndaki HER
-- birime bir kullanıcı koyar; böylece her org-traversal (parent/
-- children/self) akışında yetkili bir aktör bulunur.
--
-- Her kullanıcı iki role sahiptir:
--   • "rol"                    — jenerik; her WF/traversal için
--   • tipe göre: sube→personel, ilce→uzman, diğer→mudur
--
-- Idempotent (ON CONFLICT / DELETE-then-insert) — tekrar çalıştırılabilir.
-- Uygulama: psql -f data/seed_qnb_users.sql  (veya sim seed aracı)
-- ================================================================

DO $$
DECLARE
    c    uuid := '3c1811a6-1e63-4261-a1ce-658da1fbfa6b'; -- QNB Finansbank tenant
    tree uuid := '627c6fbc-02f0-49b8-8724-41db1f33bdf7'; -- Şubeler Ağacı
    r_rol uuid; r_mudur uuid; r_uzman uuid; r_personel uuid;
    rec record;
    uid uuid;
    type_role uuid;
BEGIN
    INSERT INTO org.r (orgtnt_id, name, display_name) VALUES
        (c, 'rol',      'Rol'),
        (c, 'mudur',    'Müdür'),
        (c, 'uzman',    'Uzman'),
        (c, 'personel', 'Personel')
    ON CONFLICT (orgtnt_id, name) DO NOTHING;

    SELECT r_id INTO r_rol      FROM org.r WHERE orgtnt_id = c AND name = 'rol';
    SELECT r_id INTO r_mudur    FROM org.r WHERE orgtnt_id = c AND name = 'mudur';
    SELECT r_id INTO r_uzman    FROM org.r WHERE orgtnt_id = c AND name = 'uzman';
    SELECT r_id INTO r_personel FROM org.r WHERE orgtnt_id = c AND name = 'personel';

    -- Önceki sim_ kullanıcılarını temizle (deterministik ~50 için)
    DELETE FROM org.ur     WHERE u_id IN (SELECT u_id FROM org.u WHERE orgtnt_id=c AND username LIKE 'sim\_%');
    DELETE FROM org.u_orgu WHERE u_id IN (SELECT u_id FROM org.u WHERE orgtnt_id=c AND username LIKE 'sim\_%');
    DELETE FROM org.u      WHERE orgtnt_id=c AND username LIKE 'sim\_%';

    -- İlk ~50 birim ltree path sırasıyla = bağlı bir alt-ağaç dilimi
    -- (root → bölge → şehir → ilçe → şube derinlemesine; parent zincirleri + şubeler dahil).
    FOR rec IN
        SELECT o.orgu_id, o.name, o.orgu_type->>'type' AS otype
        FROM org.orgu o
        JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
        WHERE oo.orgt_id = tree AND o.is_active = true AND oo.is_active = true
        ORDER BY oo.path::text
        LIMIT 50
    LOOP
        INSERT INTO org.u (orgtnt_id, username, full_name)
        VALUES (c, 'sim_' || substr(rec.orgu_id::text, 1, 8), rec.name)
        ON CONFLICT (orgtnt_id, username) DO UPDATE SET full_name = EXCLUDED.full_name
        RETURNING u_id INTO uid;

        INSERT INTO org.u_orgu (orgtnt_id, u_id, orgu_id, is_primary)
        VALUES (c, uid, rec.orgu_id, true)
        ON CONFLICT (u_id, orgu_id) DO NOTHING;

        -- atamaları sıfırla (idempotent) ve yeniden ata
        DELETE FROM org.ur WHERE u_id = uid AND orgu_id = rec.orgu_id;

        INSERT INTO org.ur (orgtnt_id, u_id, r_id, orgu_id, ur_type)
        VALUES (c, uid, r_rol, rec.orgu_id, 'granted');

        type_role := CASE rec.otype
            WHEN 'sube' THEN r_personel
            WHEN 'ilce' THEN r_uzman
            ELSE r_mudur
        END;

        INSERT INTO org.ur (orgtnt_id, u_id, r_id, orgu_id, ur_type)
        VALUES (c, uid, type_role, rec.orgu_id, 'granted');
    END LOOP;
END $$;

-- ================================================================
-- Birim-rolleri (org.orgu_r): eski 'special' regex'i artık org.orgu_r grant'ı
-- üretir. Döviz birimlerine ayrıca kullanıcı + "rol" + "doviz" rolleri seed'lenir;
-- böylece `*:[role:doviz]` / `[role:doviz]` filtreli akışlar için aktör bulunur.
-- (Tüm orgu'lar seed edildikten SONRA çalışır → 5/5.)
-- ================================================================
DO $$
DECLARE
    c uuid := '3c1811a6-1e63-4261-a1ce-658da1fbfa6b';
    r_rol uuid; r_doviz uuid; r_kredi uuid;
    rec record;
    uid uuid;
BEGIN
    INSERT INTO org.r (orgtnt_id, name, display_name) VALUES
        (c, 'doviz', 'Döviz'), (c, 'kredi', 'Kredi')
    ON CONFLICT (orgtnt_id, name) DO NOTHING;
    SELECT r_id INTO r_rol   FROM org.r WHERE orgtnt_id = c AND name = 'rol';
    SELECT r_id INTO r_doviz FROM org.r WHERE orgtnt_id = c AND name = 'doviz';
    SELECT r_id INTO r_kredi FROM org.r WHERE orgtnt_id = c AND name = 'kredi';

    -- Birim-rol grant'ları (eski seed_orgu_type regex'i → org.orgu_r).
    INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
    SELECT c, o.orgu_id, r_kredi, 'granted'
    FROM org.orgu o
    JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
    WHERE oo.orgtnt_id = c AND o.is_active = true AND oo.is_active = true
      AND ( (o.metadata->>'code') ~* 'kredi'
            OR o.name ~* 'kredi|finansman|fon yonetimi|fon yönetimi' )
    ON CONFLICT (orgu_id, r_id) DO NOTHING;

    INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
    SELECT c, o.orgu_id, r_doviz, 'granted'
    FROM org.orgu o
    JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
    WHERE oo.orgtnt_id = c AND o.is_active = true AND oo.is_active = true
      AND ( (o.metadata->>'code') ~* 'havalimani|pasaport|serbest-bolge|international|laleli|karakoy|sultanhamam|nisantasi|bodrum|marmaris|fethiye|kusadasi|cesme|yalikavak|ortakoy'
            OR o.name ~* 'havaliman|pasaport|serbest bolge|serbest bölge|international|laleli|karakoy|karaköy|sultanhamam|nisantasi|nişantaşı|bodrum|marmaris|fethiye|kuşadası|çeşme|yalıkavak|ortaköy' )
    ON CONFLICT (orgu_id, r_id) DO NOTHING;

    FOR rec IN
        SELECT o.orgu_id, o.name
        FROM org.orgu o
        JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
        JOIN org.orgu_r orr   ON orr.orgu_id = o.orgu_id AND orr.r_id = r_doviz
        WHERE oo.orgtnt_id = c AND o.is_active = true AND oo.is_active = true
    LOOP
        INSERT INTO org.u (orgtnt_id, username, full_name)
        VALUES (c, 'sim_' || substr(rec.orgu_id::text, 1, 8), rec.name)
        ON CONFLICT (orgtnt_id, username) DO UPDATE SET full_name = EXCLUDED.full_name
        RETURNING u_id INTO uid;

        INSERT INTO org.u_orgu (orgtnt_id, u_id, orgu_id, is_primary)
        VALUES (c, uid, rec.orgu_id, true)
        ON CONFLICT (u_id, orgu_id) DO NOTHING;

        DELETE FROM org.ur WHERE u_id = uid AND orgu_id = rec.orgu_id;
        INSERT INTO org.ur (orgtnt_id, u_id, r_id, orgu_id, ur_type) VALUES
            (c, uid, r_rol,   rec.orgu_id, 'granted'),
            (c, uid, r_doviz, rec.orgu_id, 'granted');
    END LOOP;
END $$;
