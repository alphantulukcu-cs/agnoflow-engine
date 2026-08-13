-- ================================================================
-- wf.wfe → wf.wfd_meta yabancı anahtarı (2026-08-13)
--
-- Bugüne kadar `wf.wfe.wfd_id` düz bir `uuid` kolonuydu: DB, işaret ettiği WFD
-- satırının var olup olmadığını KONTROL ETMİYORDU. Sonuç, üretimde görüldü —
-- bir WFD satırı silindi, ona bağlı 4 WFE öksüz kaldı:
--   * `WfdStore::fetch` başarısız → detay ucu `500 wfd not found`,
--   * görünürlük projeksiyonu üretilemez (grant hesaplanamaz),
--   * backfill/worker satırı atlamak zorunda,
--   * ve etiket/c_a/graf bilgisinin HİÇBİRİ geri getirilemez.
-- Yani veri kaybı sessizce kalıcı hale geliyordu. Öksüz satırlar
-- `orphan_wfe_cleanup --apply` ile temizlendi; bu kısıt sınıfın TEKRARINI önler.
--
-- Neden yalnız `wfd_id`: `wf.wfd_meta`'nın birincil anahtarı `wfd_id`'dir ve
-- HER SÜRÜM AYRI SATIRDIR (`version` kolonu satırın bir alanı, kimliğin parçası
-- değil). Dolayısıyla `wfd_id` tek başına tam bir referanstır; `wfd_version`
-- kolonu WFE'de okunabilirlik/sorgu kolaylığı için durur.
--
-- Neden ON DELETE yok (yani NO ACTION): koşan/bitmiş iş varken WFD sürümünü
-- silmek REDDEDİLMELİ. CASCADE, tarifi silmenin işleri de silmesi demekti —
-- denetim izini bir DDL hatasıyla kaybetmenin en kısa yolu. SET NULL de
-- olamaz: `wfd_id` NOT NULL ve tarifsiz WFE anlamsız.
--
-- UYARI: bu kısıt taslak silmeyi (`repo::wfd::delete_draft`) etkilemez —
-- taslaktan WFE başlatılamaz (`get_meta` yalnız published satırı döner),
-- dolayısıyla silinen taslağa bağlı WFE olamaz.
-- ================================================================

ALTER TABLE wf.wfe
    ADD CONSTRAINT wfe_wfd_fk
    FOREIGN KEY (wfd_id) REFERENCES wf.wfd_meta (wfd_id);

COMMENT ON CONSTRAINT wfe_wfd_fk ON wf.wfe IS
    'WFE tarifsiz kalamaz: WFD satırı silinmek istenirse (ona bağlı örnek varken) reddedilir';
