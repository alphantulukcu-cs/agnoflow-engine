-- ================================================================
-- WFD taslak kilidi: HEARTBEAT (crash ağı).
--
-- 20260818000001 kilidin TTL'ini kaldırdı ("komşu akışa bakarken kilit
-- düşmesin" — bilinçli, bu göç ONU BOZMAZ). Ama TTL'siz kilidin gerçek açığı
-- farklıydı: tarayıcı çöktüğünde / ağ koptuğunda / OS süreci öldürdüğünde
-- `pagehide` de ateşlenmez, kilit sunucuda SÜRESİZ asılı kalır ve tek çıkış
-- yolu yönetici "Kilidi kır" olur.
--
-- Heartbeat bunu TTL'den AYRI bir eksende çözer: sekme AÇIK kaldığı sürece
-- (hangi taslağa bakılırsa bakılsın, "komşu akışa gidip gelme" davranışına
-- dokunmadan) periyodik ping akar; yalnız sekme GERÇEKTEN öldüğünde
-- (heartbeat de durduğunda) belirli bir sessizlikten sonra kilit stale sayılıp
-- devralınabilir hale gelir. Eşik istemci ping aralığından kasıtlı olarak kat
-- kat büyük tutulur (bkz. `wf_wfd::repo::LOCK_STALE_AFTER`) — arka plana
-- alınmış sekmelerde tarayıcı zamanlayıcı kısar, bu payı yanlış-pozitif
-- devralmayı önler.
--
-- Migration tek başına ve tekrar tekrar koşabilmeli (idempotent).
-- ================================================================
ALTER TABLE wf.wfd_meta
  ADD COLUMN IF NOT EXISTS lock_heartbeat_at TIMESTAMPTZ;

-- Halihazırda tutulan kilitlere başlangıç heartbeat'i ver — yoksa göç anında
-- canlı olan her kilit "hep stale" görünüp anında devralınabilir olurdu.
UPDATE wf.wfd_meta
   SET lock_heartbeat_at = COALESCE(lock_acquired_at, now())
 WHERE lock_user_id IS NOT NULL AND lock_heartbeat_at IS NULL;
