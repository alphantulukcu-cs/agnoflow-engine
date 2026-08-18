-- ================================================================
-- WFD taslak kilidi: SÜRE SINIRI KALDIRILDI.
--
-- Eski tasarım (20260811000004) kilidi 5 dakikalık tutuyor ve tazelemeyi insan
-- eylemine bağlıyordu. Yeni kural: kilit, editör taslağı AÇIK TUTTUĞU sürece
-- sahibindedir; yalnız BIRAKILDIĞINDA (ya da taslak publish/submit ile taslak
-- olmaktan çıktığında) serbest kalır. Dolayısıyla `lock_expires_at`'in taşıdığı
-- bilgi anlamsızlaştı — kolonu bırakmak, okuyan birine hâlâ TTL varmış izlenimi
-- verirdi.
--
-- MEVCUT KİLİTLER ÖNCE SERBEST BIRAKILIR. Kolon düşürülünce "süresi geçmiş kilit"
-- kavramı da kalkar: göç anında canlı olan her kilit KALICI hale gelirdi ve o
-- taslaklar bir daha düzenlenemezdi. Göç öncesi temizlik bu tuzağı kapatır —
-- kaybedilen tek şey, göç anında editörü açık olan kullanıcının kilidini yeniden
-- almasıdır (taslak açılışında kendiliğinden olur).
--
-- Migration tek başına ve tekrar tekrar koşabilmeli (idempotent).
-- ================================================================
UPDATE wf.wfd_meta
   SET lock_user_id = NULL, lock_acquired_at = NULL
 WHERE lock_user_id IS NOT NULL;

ALTER TABLE wf.wfd_meta
  DROP COLUMN IF EXISTS lock_expires_at;
