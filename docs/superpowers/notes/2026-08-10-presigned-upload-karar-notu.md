# Belge yüklemede presigned URL'e geçiş — KARAR NOTU

**Tarih:** 2026-08-10
**Durum:** ❗ **KARAR BEKLİYOR — uygulamaya geçilmedi, kod yazılmadı.**
**Karar mercii:** yönetim (aşağıdaki "Sorulacaklar" bölümü)
**İlgili:** `docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md`,
CLAUDE.md → "Attachments (ek-belge) sözleşmesi" ve "WFE not defteri",
`crates/server/src/attachment_store.rs`, `crates/server/src/attachments.rs`

> **Bu doküman bir spec DEĞİLDİR.** Onaylanmamış bir mimari değişikliğin gerekçesini,
> bedelini ve açık sorularını unutmamak için yazıldı. Onaylanırsa `docs/superpowers/specs/`
> altına tasarım dokümanı olarak taşınır ve fazlara bölünür.

---

## 1. Nereden çıktı

Not defteri (ad-hoc not + belge) özelliği tamamlanıp anlatılırken, belge yükleme akışı
şöyle özetlendi:

```
tarayıcı  →  PUT /wfe/:id/notes/:note_id/files  →  bizim backend  →  müşterinin bucket'ı
```

Beklenti bu değilmiş. İstenen:

```
tarayıcı  →  DOĞRUDAN müşterinin bucket'ı
bizim backend  →  yalnız imzalı URL üretir + metadata tutar; BAYT BİZE HİÇ UĞRAMAZ
```

Yani müşterinin belgesi hiçbir aşamada bizim sürecimizden/diskimizden geçmesin.

**Not:** Bugünkü mimaride bayt bize *uğruyor* ama bizde *kalmıyor* (`backend = s3` iken
belleğe alınıp müşterinin deposuna yazılıyor). `backend = local` iken zaten "müşterinin
deposu" bizim sunucumuzdaki bir dizin — orada duruyor. Engine (`wfe-core`) hiçbir
koşulda dosya I/O yapmıyor; yazan taraf `crates/server`'ın portal ucu. Bu ayrım
tartışma sırasında karıştı, kayda geçirilsin diye buraya yazıldı.

---

## 2. Ne düşünüyoruz

**Talep meşru ve savunulabilir.** "Müşterinin belgesi tedarikçinin sunucusundan geçmesin"
bankacılık/kamu tarafında sık istenen bir güvence. Presigned URL bunun standart cevabı.

Ama bu **taşıma yolunu değiştiren bir optimizasyon değil**; doğrulamanın *nerede*
yapıldığını değiştiren bir mimari karar. Bedelini bilerek almak lazım. Aşağıdaki üç
başlık, kararı verirken masada olması gerekenler.

---

## 3. Ne kaybediyoruz — açık hesap

### 3.1 Boyut/tip kuralı "önleme"den "yakalama"ya düşer

Bugün dosyayı **biz yazdığımız için** boyut ve MIME kuralını yükleme anında
*engelleyerek* uyguluyoruz (`notes::add_file`, `attachments::check_upload`).

Presigned PUT'ta bu mümkün değil: imzalı URL'i eline alan istemci oraya istediği boyutta
veri yükleyebilir. Elimizde kalan tek yol, yükleme bittikten sonra `stat` çekip
**tespit edip silmek**.

| | Bugün | Presigned PUT |
|---|---|---|
| Kota aşan dosya | hiç yazılmaz | yazılır, sonra silinir |
| Yasak MIME | hiç yazılmaz | yazılır, sonra silinir |
| Doğrulamanın anı | yükleme | yükleme sonrası + aksiyon kapısı |

Sert önleme isteniyorsa **presigned POST policy** gerekir: `content-length-range` şartı
imzanın içine gömülür, depo kendisi reddeder. Bedeli:
- opendal bunu üretmiyor → SigV4 POST policy imzasını elle yazmak gerekir
- her S3-uyumlu depo aynı şekilde desteklemiyor (MinIO destekliyor, Garage şüpheli)

**Görüşümüz:** İlk sürümde `stat` ile yakala-sil yeterli. Gerçek bir suistimal görülürse
POST policy'ye o zaman yatırım yapılır. Ama bu, "kullanıcı 5 GB yükleyip bucket'ı şişirdi"
senaryosunun ilk sürümde **mümkün** olduğu anlamına gelir — bilerek kabul edilmeli.

### 3.2 Katalog belgelerinde bir açık doğuyor

Katalog (WFD'de tanımlı) belgelerde bugün "yüklendi mi" sorusu **doğrudan depodan**
soruluyor (`AttachmentStore::exists`), DB kaydı yok. Presigned'da istemci, doğrulama
adımını (`complete`) hiç çağırmadan dosyayı yükleyebilir — ve aksiyon kapısı
doğrulanmamış dosyayı geçirir.

**Çözüm:** kapı anında (`apply_action`/`submit_action`) `stat` çekilip boyut/tip katalog
kurallarına ORADA doğrulanmalı. Doğrulama, yükleme anından *önemli olduğu ana* taşınır.
Bu aslında daha sağlam bir yer — ama ek bir depo turu demek.

### 3.3 Müşterinin bucket adı ve nesne yolu tarayıcıya görünür

Presigned URL bunu zorunlu olarak açar. Bugün portal kullanıcısı bu bilgiyi görmüyor.
Sızan şey kimlik bilgisi değil (imza kısa ömürlü ve tek nesneye kapsamlı), ama
altyapı bilgisi. Müşteriye göre bu önemsiz de olabilir, itiraz konusu da.

---

## 4. Yeni operasyonel şart — en kritik madde

Tarayıcı doğrudan bucket'a `PUT` atacaksa **müşterinin bucket'ında CORS ayarı** olmak
zorundadır: bizim portal origin'imize `PUT` izni + imzaladığımız header'lara izin.

**Bunu biz yapamayız — müşteri yapar.** Yapılmazsa yükleme tarayıcıda anlaşılmaz bir
CORS hatasıyla düşer ve hata mesajı hiçbir şey anlatmaz.

Bu, satış/kurulum sürecine yeni bir madde ekler: *"belge yükleyecekseniz bucket'ınızda
şu CORS ayarını yapmanız gerekiyor."* Bugün böyle bir şart yok.

**Görüşümüz:** Plana bir **depo sağlık kontrolü** ucu konmalı — ayarlar girilirken
presign üretip bir probe atsın, "bu bucket portal'dan yazılabiliyor mu" sorusu akış
canlıya çıkmadan cevaplansın. Aksi halde sorun ilk gerçek kullanıcıda, üretimde patlar.

---

## 5. `local` backend ne olacak

Yerel dosya sistemi (`opendal` `Fs`) presign desteklemiyor.

- **(a)** `local`'ı belge toplayan akışlarda tamamen yasakla → her müşteri S3-uyumlu depo
  kurmak zorunda kalır
- **(b)** `local` kalsın, orada bugünkü proxy yolu işlemeye devam etsin; `s3`'te presigned

**Görüşümüz: (b)**, ama `local` açıkça **geliştirme/test yolu** olarak işaretlenmeli.
Zaten `local` = "müşterinin deposu bizim diskimiz" demek; presign'ın orada anlamı yok ve
"bayt bize uğramasın" hedefi orada zaten geçersiz.

---

## 6. Tasarlanan akış (onaylanırsa)

```
1. POST .../files   {filename, mime, size}
   → server: not draft mi, yazarı mı, kota/limit uygun mu → DB'ye 'pending' satır
   → presigned PUT URL + kısa TTL döner

2. PUT <presigned url>   bayt
   → tarayıcıdan DOĞRUDAN müşterinin bucket'ına. Bize uğramaz.

3. POST .../files/:file_id/complete
   → server: stat → nesne var mı, boyut beyanla uyuşuyor mu, sınırı aşıyor mu
   → uygunsa 'ready'; değilse nesne SİLİNİR, satır reddedilir
```

- Yalnız `ready` dosyalar not görünümünde listelenir ve kotaya sayılır.
- `pending` kalan satırlar + karşılık gelen nesneler süpürücüyle temizlenir (yetim yükleme).
- Rezervasyon akışı (başlatma öncesi belge) kavramsal olarak aynı kalır.

---

## 7. Az önce yazdığımızın ne kadarı etkilenir

Not defteri özelliği (Faz 0–4) **tamamlandı ve çalışıyor**. Presigned'a geçilirse:

| Parça | Durum |
|---|---|
| `wf.wfe_note` / `wfe_note_read` / wfah izi (`from_node`/`to_node`) | **Değişmez** |
| Not mantığı: draft→publish, `audience`, IDOR kapsamı, gizleme, okundu | **Değişmez** |
| `wf.wfe_note_file` tablosu | **Kalır**, `status` (`pending`/`ready`) + doğrulanmış boyut alanları eklenir |
| Not dosyası yükleme/indirme rotaları | **Yeniden yazılır** (3 adımlı akış) |
| Limit uygulama yeri (`notes::add_file`) | **Taşınır** — beyan anında ön kontrol + `complete`'te gerçek doğrulama |
| Katalog attachments rotaları + gate | **Değişir** (kapıda `stat`) |
| Portal UI yükleme akışı | **Yeniden yazılır** |

Kabaca: **not defterinin kendisi sağlam, dosya taşıma katmanı yeniden yazılıyor.**

---

## 8. Fazlar (onaylanırsa)

| Faz | İş |
|---|---|
| P0 | Presign yeteneği + config doğrulama + **depo sağlık kontrolü** ucu |
| P1 | Not dosyaları presigned'a: `pending`/`ready` yaşam döngüsü, `complete`, yetim süpürücü |
| P2 | Katalog attachments presigned'a + kapıda `stat` doğrulaması |
| P3 | İndirme kararı (aşağıdaki S1) |
| P4 | Portal UI: iki akış da 3 adımlı yüklemeye |
| P5 | CLAUDE.md + spec güncellemesi + **müşteriye CORS kurulum dokümanı** |

---

## 9. Sorulacaklar (yönetim kararı)

### S1 — İndirme de presigned mi olsun?

Presigned GET kısa ömürlüdür ama **link paylaşılabilir**: TTL boyunca linki eline geçiren
herkes dosyayı indirir, bizim yetki kontrolümüz devrede olmaz.

- Seçenek A: indirme de presigned (bayt hiç uğramaz, ama link paylaşılabilir)
- Seçenek B: indirme bizden geçmeye devam etsin (her istekte yetki kontrolü)

**Görüşümüz: B.** Hacim yüklemededir, indirmede değil; asıl kazanç yüklemeyi taşımakta.
Ayrıca "kim ne zaman indirdi" izini korumak denetim açısından değerli.

### S2 — `local` backend kalsın mı?

**Görüşümüz: kalsın, dev-only olarak işaretlensin** (bkz. §5).

### S3 — Sert boyut önlemesi (presigned POST policy) ilk sürümde olsun mu?

**Görüşümüz: hayır.** `stat` ile yakala-sil yeterli. Ama bu, ilk sürümde kota aşan
yüklemenin *mümkün* olduğu anlamına gelir (bkz. §3.1) — bu risk kabul ediliyor mu?

### S4 — Bucket adı / nesne yolunun tarayıcıya görünmesi kabul mü? (bkz. §3.3)

### S5 — CORS şartı müşteriye anlatılabilir mi?

Belge yükleyen her müşteri bucket'ında CORS ayarı yapmak zorunda (bkz. §4). Bu satış ve
kurulum süreçlerine yeni bir madde ekler. Kabul mü, yoksa "müşteriden ek ayar
isteyemeyiz" mi?

**Bu sorunun cevabı "isteyemeyiz" ise presigned mimarisi çalışmaz** — o durumda bugünkü
proxy mimarisinde kalınır ve §1'deki talep karşılanamaz. Diğer dört sorudan önce bunun
netleşmesi verimli olur.

---

## 10. Özet

- Talep meşru, standart cevabı presigned URL.
- Bedeli: doğrulama önlemeden yakalamaya düşer, katalog kapısı `stat`'a bağlanır,
  müşteriden CORS ayarı istenir, altyapı bilgisi tarayıcıya açılır.
- Not defteri özelliği bundan **etkilenmez**; yeniden yazılan yalnız dosya taşıma katmanı.
- **En kritik soru S5 (CORS).** Cevabı olumsuzsa geri kalanı tartışmaya gerek kalmaz.
