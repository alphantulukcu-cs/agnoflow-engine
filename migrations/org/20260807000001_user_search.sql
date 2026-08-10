-- Kullanıcı arama (c_u tamamlaması) — SUNUCU tarafı.
--
-- Editörün `c_u` alanı kişileri yazdıkça öneriyor. İlk sürüm tenant'ın TÜM kullanıcı
-- listesini indirip istemcide süzüyordu; on binlerce kullanıcılı bir tenant'ta bu hem
-- ilk açılışı hem belleği taşırır. Arama artık `GET /org/orgtnt/{id}/users?q=` ile
-- veritabanında yapılıyor (`repo::user_role::search_users`) — bu indeksler onu taşır.
--
-- `ILIKE '%…%'` başı serbest bir kalıptır: B-tree onu KULLANAMAZ, her sorgu seq scan olur.
-- Trigram GIN indeksi bu kalıbı destekler ve `pg_trgm` karşılaştırmayı kendisi
-- küçük harfe indirger, ayrıca `lower()` ifade indeksi gerekmez.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX IF NOT EXISTS u_username_trgm  ON org.u USING gin (username  gin_trgm_ops);
CREATE INDEX IF NOT EXISTS u_full_name_trgm ON org.u USING gin (full_name gin_trgm_ops);
CREATE INDEX IF NOT EXISTS u_email_trgm     ON org.u USING gin (email     gin_trgm_ops);
