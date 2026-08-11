-- ================================================================
-- T‑B4: WFD taslak kilidi (pessimistic).
-- Tasarım: docs/superpowers/specs/2026-08-11-draft-kilidi-design.md
--
-- Kilit AYRI TABLO DEĞİL: taslakla 1:1 ve taslak zaten `wf.wfd_meta` satırıdır —
-- join yok, yazma yolu tek, kilit koşulu mutasyonun kendi WHERE'ine girebiliyor
-- (kontrol-sonra-yaz açığı olmadan).
--
-- Süresi geçmiş kilit SİLİNMEZ: `lock_expires_at <= now()` koşulu onu zaten geçirir,
-- ayrı süpürücüye gerek yok. Kolonlar son sahibin izini taşımaya devam eder —
-- "5 dakika önce kimdeydi" sorusu destek için değerli.
--
-- Migration tek başına ve tekrar tekrar koşabilmeli (idempotent).
-- ================================================================
ALTER TABLE wf.wfd_meta
  ADD COLUMN IF NOT EXISTS lock_user_id     uuid,
  -- Tazelemede DEĞİŞMEZ (COALESCE): "bu kişi bu taslağı ne zamandır tutuyor".
  ADD COLUMN IF NOT EXISTS lock_acquired_at timestamptz,
  ADD COLUMN IF NOT EXISTS lock_expires_at  timestamptz;

-- Kilit sorguları daima (wfd_id, version) ile gelir → PK yeter, ek indeks yok.
-- "Bu kullanıcının elindeki taslaklar" listesi ileride gerekirse eklenir.
