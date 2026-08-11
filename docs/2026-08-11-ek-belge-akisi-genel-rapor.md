# Ek Belge Akışı — Genel Rapor

**Tarih:** 2026-08-11
**Kime:** Teknik olmayan okuyucu dahil herkes
**Teknik karşılığı:** `docs/2026-08-11-ek-belge-akisi-teknik-rapor.md`

---

## Bir cümleyle

Akış başlatırken ve akış sırasında belge yüklemeyi, **hata durumunda arkada çöp bırakmayan
tek bir işleme** dönüştürdük.

---

## Neyi düzeltmeye başladık

Bir kullanıcı portalda belge isteyen bir akışı başlatmaya çalıştığında — diyelim kredi
başvurusu, kimlik fotokopisi ve gelir belgesi istiyor — şu oluyordu:

> Kullanıcının o akışı başlatma **yetkisi yoktu**. Sistem yine de dosyalarını depoya yazdı,
> sonra "yetkiniz yok" dedi. Akış hiç başlamadı ama belgeler depoda kaldı.

Aynı şey yolda bir hata olduğunda da oluyordu. İki dosyadan biri yüklenip ikincisi
reddedilirse, birincisi depoda öylece duruyordu — sahibi olmayan, kimseye ait olmayan bir
dosya olarak.

Bu dosyalar 24 saat sonra otomatik temizleniyordu, yani kalıcı bir birikme yoktu. Ama
"başarısız bir işlem arkasında iz bırakmamalı" kuralı çiğneniyordu. Kredi başvurusu gibi
belgelerin denetime tabi olduğu bir işte bu kabul edilebilir değil.

---

## Sorunun asıl kaynağı

Sorun tek bir hatalı satır değildi, **işlerin sırasıydı**.

Dosyanın depodaki adresi akışın kimlik numarasını içeriyor: `belgeler/{akış-no}/kimlik.pdf`.
Ama o kimlik numarası akış başlarken doğuyordu. Yani:

- Önce akışı başlatırsan → dosyaların adresi belli olur ama akış belgesiz başlamış olur.
  Yükleme yarıda kalırsa belgesiz bir başvuru ortada kalır.
- Önce dosyaları yüklersen → adres yok, yazacak yer yok.

Ekip bunu şöyle çözmüştü: **numarayı önden rezerve et.**

```
1. Sunucudan bir numara al        → POST /wfe/reserve
2. Dosyaları o numaranın altına yükle  → her dosya için ayrı istek
3. O numarayla akışı başlat       → POST /wfe
```

Çalışıyordu. Ama üç sorun getiriyordu:

**Birincisi: temizlik sorumluluğu yanlış taraftaydı.** Arayüzün "hangi hatada dosyaları
sileyim, hangisinde saklayayım" sorusunu bilmesi gerekiyordu. Eksik belge hatası geldiyse
saklamalı (kullanıcı eksiği tamamlayacak), geçersiz girdi hatası geldiyse silmeli, geçici
sunucu hatası geldiyse tekrar denemeli. Bu ayrım motorun bilgisi, arayüzün değil. Portal
bunu doğru yapsa bile, bu API'yi kullanan başka bir yazılım aynı disiplini sıfırdan kurmak
zorundaydı — kurmayan çöp bırakıyordu.

**İkincisi: yetki kapısı yanlış yerdeydi.** Yetki en sonda, akış başlatılırken soruluyordu.
Dosyalar çoktan yazılmış oluyordu.

**Üçüncüsü: yükleme yarıda kalabiliyordu.** Üç dosyadan ikisi yazılıp üçüncüsü reddedilince
ortada tutarsız bir küme kalıyordu. Kullanıcıya "kısmen yüklendi" diye anlatılabilecek bir
durum yok — ya hepsi ya hiçbiri olmalı.

---

## Ne yaptık

### Adım adım yeni akış

Kullanıcı "Başlat"a bastığında artık **tek bir istek** gidiyor. İçinde hem form bilgileri
hem dosyalar var. Sunucu şunları sırayla yapıyor:

| # | Adım | Hata olursa |
|---|---|---|
| 1 | Form bilgilerini oku (dosyalardan **önce** geliyor) | — |
| 2 | **Bu kullanıcı bu akışı başlatabilir mi?** | 403, tek bayt dosya okunmadan |
| 3 | "Bu istek az önce gelmiş miydi?" (çift tıklama kontrolü) | İlk sonucu döndür |
| 4 | Dosyaları depoya yaz | Yazılanları sil |
| 5 | Zorunlu belgeler tam mı? | Yazılanları sil |
| 6 | Akışı başlat | Yazılanları sil |
| 7 | Başarılı → numarayı döndür | — |

**Kritik olan 2. adım.** Form bilgileri istekte dosyalardan önce geldiği için, yetkisiz bir
kullanıcı için sunucu yaklaşık 1 KB okuyup "yetkiniz yok" diyebiliyor. 200 MB'lık bir dosya
yığınını okuyup sonunda reddetmesi gerekmiyor.

**4-6 arasındaki her adımda** hata çıkarsa sunucu kendi yazdıklarını kendisi siliyor.
Arayüzün hiçbir temizlik çağrısı yapması gerekmiyor.

### Örnek 1 — Yetkisiz kullanıcı

*Eskiden:* Numara al → iki dosya yazıldı → "yetkiniz yok" → dosyalar 24 saat depoda.

*Şimdi:* İstek gider, sunucu form kısmını okur, yetkiyi sorar, **403 döner**. Dosyalar
depoya hiç yazılmaz.

### Örnek 2 — İkinci dosya çok büyük

*Eskiden:* Kimlik yazıldı → gelir belgesi 7 MB, sınır 5 MB → reddedildi → kimlik depoda
sahipsiz kaldı, portal ayrıca silme isteği atmak zorundaydı.

*Şimdi:* Kimlik yazılır, gelir belgesi reddedilir, **sunucu kimliği de siler** ve tek
cevapta hangi belgenin neden reddedildiğini satır satır bildirir:

> `basvuru_belgeleri/gelir_belgesi: dosya 5 MB sınırını aşıyor`

Kullanıcı doğru dosyayı seçip tekrar dener. Depoda hiçbir iz yok.

### Örnek 3 — Kullanıcı iki kez tıkladı

Yükleme 90 saniye sürdü, bağlantı koptu, kullanıcı hata gördü ve tekrar bastı. Akış aslında
başlamıştı — ikinci basış **ikinci bir kredi başvurusu** açacaktı.

Artık sunucu isteğin kendisinden bir parmak izi çıkarıyor (kim, hangi akış, hangi bilgiler).
Aynı parmak izi 60 saniye içinde tekrar gelirse iş tekrar çalıştırılmıyor, **ilk başvurunun
numarası** dönüyor. Kullanıcı ikinci başvuru açmıyor.

Bunun için arayüzün özel bir şey göndermesi gerekmiyor — koruma tamamen sunucuda. Kasten
aynı başvuruyu ikinci kez açmak isteyen bir sistem varsa bunu belirtebiliyor.

### Örnek 4 — Akış ortasında belge (yeni)

Müdür "Onayla" derken kredi raporunu da eklemek istiyor. Aynı desen, ama burada ek bir
zorluk var: o slotta **zaten bir dosya olabilir**.

Üzerine yazıp aksiyon başarısız olursa eski dosya geri getirilemez. Bu yüzden:

```
dosyalar → geçici alana yazılır (mevcut belge DEĞİŞMEZ)
kapı     → "depodakiler + şimdi gönderilenler" birlikte sayılır
aksiyon  → uygulanır
   başarı → geçici alandan yerine taşınır
   hata   → geçici alan silinir, eski belge olduğu gibi kalır
```

Yani **aksiyon geçmezse hiçbir belge değişmiyor.** İstediğin garanti bu.

### Örnek 5 — Sonradan gelen bir belge, karar vermeden

Bazen kullanıcı bir karara bağlamadan sadece eksik bir evrakı tamamlamak ister — örneğin
istenen bir imza sayfasını sonradan eklemek. Bu da artık aynı "hepsi ya da hiçbiri"
kuralına tabi: birden fazla dosya seçilip tek düğmeyle gönderiliyor, biri reddedilirse
hiçbiri kalıcı olarak yerine konmuyor. Eskiden bu dosya başına ayrı bir istekti; artık
tek istek.

**Not:** Yukarıda anlatılan "önce bir numara al" yöntemi (üç ayrı adım: numara → dosyalar
→ başlat) kontrol ettiğimizde bu sistemde artık kimse tarafından kullanılmıyordu —
tamamen kaldırıldı. Geriye kalan tek şey, sunucunun kendi içinde tuttuğu görünmez bir
numara kaydı: istek ortasında sunucu çökerse (bakım, yeniden başlatma gibi) hangi
dosyaların sahipsiz kaldığını bilmek için. Kullanıcı bunu hiç görmez, sadece "başarısız
oldu" ya da "başarılı oldu" cevabını görür.

---

## Yol boyunca bulduğumuz iki gerçek hata

Bunlar planlanan iş değildi, çalışırken ortaya çıktı.

**1. Dosya boyutu sınırı yalan söylüyordu.** Akış tanımında "en fazla 20 MB" yazan bir belge
slotu, pratikte 2 MB'ta hata veriyordu. Sunucunun genel bir 2 MB sınırı vardı ve belge
kuralları hiç devreye girmeden dosyayı reddediyordu. Kural belgede yazıyor, davranış başka.
Düzeltildi.

**2. Dosya tipine körü körüne güveniliyordu.** Bir dosyanın tipini istemci bildiriyordu.
Yani `.exe` bir dosyayı "bu bir PDF" diye göndermek yeterliydi, sistem kabul ediyordu. Artık
dosyanın **içeriğinin ilk baytlarına** bakılıyor; beyanla içerik çelişirse reddediliyor.
(Word/Excel dosyalarının teknik olarak zip dosyası olması gibi meşru durumlar ayrıca ele
alındı, yanlış alarm vermiyor.)

---

## Sessiz bir risk daha kapatıldı

Her müşteri belgelerini **kendi deposuna** yazıyor — banka A'nın belgeleri banka B'nin
deposuna hiç uğramıyor. İzolasyon bundan geliyor.

Ama bir akış depo ayarını vermemişse sistem sessizce **bizim sunucumuzun diskine** yazıyordu.
O durumda farklı müşterilerin belgeleri bizim diskimizde yan yana durur.

Yayınlama sırasında bunu engelleyen bir kontrol vardı, ama tek savunma olarak yeterli
değildi: o kontrol konmadan önce yayınlanmış akışlar, sonradan silinen ayarlar ve yeni
eklenen ortamlar arasından geçebiliyordu.

Artık **yazma işlemi ayar yoksa hata veriyor**, sessizce bizim diske düşmüyor. Okuma ve
silme işlemleri hoşgörülü bırakıldı — eskiden bizim diske yazılmış dosyalar hâlâ
okunabilmeli ve temizlenebilmeli, yoksa mevcut belgeler bir anda erişilemez olurdu.

---

## Yeni kazanılan yetenekler

**Kim ne zaman ne yükledi belli.** Eskiden bir dosyanın sistemde hiçbir kaydı yoktu; tek
gerçeklik depodaki dosyanın kendisiydi. Artık dosya adı, tipi, boyutu, kim yükledi, ne zaman
yükledi kayıt altında. Aynı slota tekrar yükleme eskisini silmiyor, **yeni sürüm** açıyor —
"karar verildiği anda hangi belge oradaydı" sorusu cevaplanabilir. Denetimin aradığı şey bu.

**Büyük dosyalar sistemi yormuyor.** Artık dosya sunucunun belleğine alınmıyor, akarak
yazılıyor. 200 MB'lık bir istek de 2 MB'lık bir istek kadar bellek kullanıyor. Ayrıca çok
büyük dosyalar için ayrı bir yol var: dosya doğrudan müşterinin deposuna yükleniyor, sunucuya
hiç uğramıyor, başlatma isteğine sadece "şu dosyayı kullan" bilgisi giriyor.

---

## Neyi bilerek yapmadık

Dürüst olmak gerekirse dört şey eksik ve bunlar kod yazarak çözülecek şeyler değil:

| Eksik | Neden |
|---|---|
| **Virüs taraması** | Ortada tarama yapacak bir servis yok. Kurmak ayrı bir sistem ayağa kaldırmak demek. |
| **Müşteri başına şifreleme anahtarı** | Altyapı kararı: hangi kasa, kim erişir, anahtar nasıl yenilenir. |
| **Saklama süresi / değiştirilemezlik** | Hukuk ve uyum kararı: hangi belge kaç yıl tutulacak? |
| **Depo sağlayıcısının otomatik temizliği** | Ayrı bir repoda yönetiliyor ve geliştirme ortamında karşılığı yok — bunun yerine kendi temizleyicimizi yazdık, iki ortamda da çalışıyor. |

---

## Kabul ettiğimiz tek boşluk

Akış ortasında bir aksiyon **başarıyla uygulandıktan sonra** dosya yerine taşınamazsa,
aksiyon geri alınmıyor. Sebep: o noktada geçiş kaydedilmiş durumda, geri sarmak akışın
geçmişini bozar.

Bunun sessiz kalmaması için üç önlem var: taşıma birkaç kez deneniyor, olmazsa kullanıcıya
"aksiyon geçti ama şu belge yerine konamadı, tekrar yükleyin" uyarısı gösteriliyor, ve dosya
gerçekten yerinde olmadığı için o belgeyi isteyen bir sonraki adım geçmiyor. Yani akış eksik
belgeyle sessizce ilerlemiyor.

---

## Devreye alınmadan önce

1. **Üç veritabanı değişikliği bugün uygulandı** (sırayla). Bu adım tamam; yeni yollar artık
   çalışıyor.
2. **Bir davranış değişikliği var:** depo ayarı olmayan bir akışa belge yüklemek artık hata
   veriyor. İstenen bu, ama böyle bir akış varsa kullanıcı ilk yüklemede karşılaşacak.
3. **Henüz gerçek ortamda uçtan uca denenmedi.** Veritabanı değişiklikleri uygulandı ve kod
   derleme + birim testlerinden geçti — ama gerçek bir kullanıcı isteğiyle, gerçek bir dosya
   deposuna karşı baştan sona hiç denenmedi. "Veritabanı hazır" ile "akış gerçekten
   çalışıyor" farklı şeylerdir.

---

## İlk kez gerçek ortamda çalıştırdık — ve üç hata çıktı

O güne kadar her şey "derleniyor ve birim testleri geçiyor" seviyesindeydi. Sonunda sunucuyu
gerçek veritabanı ve dosya deposuyla ayağa kaldırıp 12 senaryoyu baştan sona denedik. 12'si
de geçti — ama yol boyunca üç gerçek hata ortaya çıktı. Bunlar önemli, çünkü hiçbiri birim
testlerinde görünmüyordu; ancak sistem gerçekten ayağa kalkınca fark edildi.

**1. Sunucu ilk açılışta çöküyordu.** Aynı adrese iki farklı "yükle" komutu bağlanmış — sistem
hangisinin çalışacağını bilemeyip daha ilk saniyede duruyordu. İki bölüm iki ayrı yardımcı
tarafından yazıldığı için biri doğru, diğeri hatalı kurulmuştu. Düzeltildi.

**2. Bir "hızlı yükleme" kapısı denetimi atlıyordu.** Hazır bir akış tanımını doğrudan yükleyip
yayınlayan bir kısayol vardı; bu kısayol "belge deposu ayarlı mı?" kontrolünü yapmıyordu.
Normal yayınlama yolu yapıyordu, bu kısayol atlıyordu. Yani belge toplayan bir akış, deposu
hiç ayarlanmadan yayınlanabiliyordu. Düzeltildi — artık bu kısayol da aynı kontrolden geçiyor.

**3. En ciddisi: hiçbir akış başlatılamıyordu.** Bir akış başlatırken sistem "hangi ortamda
çalışsın?" bilgisini bekliyordu (test mi, canlı mı gibi). Arayüz bunu göndermiyordu ve sistem
"varsayılanı kullan" diyordu — ama o varsayılanı bir yere not etmeyi unutuyordu, sonra "boş
bırakılamaz" diye hata veriyordu. Sonuç: portaldan hiçbir akış başlatılamazdı. Bu hata bizim
belge çalışmamızdan önce de vardı, sadece kimse denk gelmemişti. Düzeltildi — sistem artık
varsayılanı bulup not ediyor.

---

## Özet tablo

| Konu | Önce | Sonra |
|---|---|---|
| Başlatma isteği | 3 + dosya sayısı kadar | 1 |
| Hata sonrası temizlik | Arayüzün işi | Sunucunun işi |
| Yetkisiz kullanıcının dosyaları | Depoya yazılıyordu | Hiç yazılmıyor |
| Yarım kalan yükleme | Depoda kalıyordu | Siliniyor |
| Çift tıklama | İkinci başvuru açıyordu | Aynı başvuru dönüyor |
| Boyut sınırı | Tanımdaki değer geçersizdi | Tanımdaki değer geçerli |
| Dosya tipi | İstemcinin beyanına güveniliyordu | İçerikten doğrulanıyor |
| Kim ne yükledi | Kayıt yoktu | Kayıt altında, sürümlü |
| Ayarsız akış | Sessizce bizim diske | Hata veriyor |
| Aksiyon başarısızsa belgeler | Üzerine yazılmış olabilirdi | Değişmiyor |
| Karar vermeden belge ekleme | Dosya başına ayrı istek | Tek istek, hepsi ya da hiçbiri |
| "Önce numara al" yöntemi | Ayrı bir yol olarak duruyordu | Kullanan kalmadığı görülünce kaldırıldı |
