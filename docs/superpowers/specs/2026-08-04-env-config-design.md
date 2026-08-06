# Ortam Konfigürasyonu (`$env`) — Tasarım

**Tarih:** 2026-08-04
**Durum:** Onaylandı, uygulanmayı bekliyor
**İlgili:** `docs/superpowers/specs/2026-07-10-wfd-db-connections-design.md`,
`migrations/wf/20260804000001_db_connection_scope.sql`

## Problem

Bir WFD bir kez tasarlanır ve şirketin farklı ortamlarında (test, prod, uat…) çalışır.
Ortama göre değişen değerler vardır: autoexec'in istek attığı REST API domaini, bağlandığı
veritabanının hostu, API anahtarları. Bugün bu değerler WFD dokümanına gömülü sabitler ya da
`db_connection` satırındaki sabit alanlardır; ortam değiştirmek için WFD'yi ya da bağlantıyı
elle düzenlemek gerekir.

Hedef: proje genelinde `$env.AUTH_API`, `$env.MONGO_HOST` gibi anahtarlarla referans verilebilen
bir ortam konfigürasyonu; test ve prod değerleri birbirine karışmadan.

## Alınan kararlar ve gerekçeleri

### K1 — Ortam modeli: hem ayrı kurulum hem tek kurulumda çok ortam

Her ortamda ayrı bir agnoflow kurulumu olabilir (test kurulumu, prod kurulumu), VE tek bir
kurulum içinde birden fazla ortam tanımlanabilir. Tasarım ikisini de desteklemek zorunda.

### K2 — Ortam runtime'da çözülür, tasarım/publish anında değil

WFD dokümanı ortamdan bağımsızdır. Ortam WFE başlatılırken belirlenir; çağıran uygulama
ortamı dışarıdan geçirebilir.

### K3 — Konfigürasyon WFD başına, tenant tabanı sonraya

Değerlerin sahibi mantıksal WFD'dir. Tenant çapında paylaşılan bir taban katmanı ileride
eklenecek; bu yüzden çözüm zinciri kodda **katmanlı** yazılır ki taban zincire eklendiğinde
WFD conf'ları bozulmadan üst katman olarak kalsın.

### K4 — Konfigürasyon WFD dokümanının İÇİNDE olamaz

WFD JSON `(wfd_id, version)` bazında immutable ve cache'lidir. Conf'u dokümana koymak, prod
domaini değiştiğinde yeni versiyon publish etmeyi gerektirirdi ve golden fixture /
immutability sözleşmesini kirletirdi. Conf doküman dışında, `(project_id, wfd_name)`
mantıksal kimliğine bağlanır — lokal `db_connection`'ın bugün kullandığı anahtarın aynısı.

### K5 — Değerler DB'de, şifreleme anahtarı deployment'ta

**Araştırma bulgusu (GitLab).** GitLab CI/CD değişkenlerini PostgreSQL'de `ci_variables`
tablosunda, şifreli kolonlarda tutar — dosya yok, deployment'a itilen değer yok. Numara
depoda değil, **anahtarın nerede olmadığında**: `db_key_base` /
`active_record_encryption_primary_key` DB'de değil, Linux paketinde
`/etc/gitlab/gitlab-secrets.json`, cloud-native chart'ta **Kubernetes Secrets**, kaynaktan
kurulumda `config/secrets.yml` içindedir. Dokümanın açık uyarısı: anahtarı DB yedekleriyle
aynı yerde tutmayın. Tehdit modeli net — **DB dump'ı tek başına işe yaramaz.** Rotasyon:
`db_key_base` dizi olabilir; şifreleme daima son değerle, çözme tüm değerler denenerek.

**Bizim durumumuz:** `crates/wfe/src/db/crypto.rs` zaten aynısını, bir gömlek iyisini yapıyor
— AES-256-GCM (kimlik doğrulamalı; GitLab'ın legacy `attr_encrypted` + AES-256-CBC'si değil),
değer başına rastgele 12 byte nonce, anahtar `DB_CONN_SECRET` env değişkeninden. K8s'te bu bir
Secret, agnoflow-infra'da SOPS+age ile şifreli. Yani GitLab cloud-native chart mimarisinin
birebir aynısı.

**Sonuç:** depolama DB'dir. Değer başına "deployment referansı" (`source=deployment`, `ref=…`)
fikri **reddedildi**: ayrım anahtar düzeyinde olmalı, değer düzeyinde değil. Deployment'ta N
referans yerine 1 anahtar → "ref bulunamadı" hata sınıfı doğmaz, iki yere bakma sorunu yok,
self-servis bozulmaz, güvenlik özelliği birebir aynı.

**Eksiğimiz:** rotasyon. `crypto.rs` tek anahtar okuyor. GitLab'ın dizi çözümü alınacak.

### K6 — "Hiç tutmayalım, çağıran her istekte göndersin" neden elenmiştir

İki bağımsız kısıt:

1. **`$env`'i çözmesi gereken anların çoğunda ortada çağıran yoktur.** 60 saniyelik timer
   sweeper, `tick_timers`, retry/escalation, SLA — bunlar HTTP isteğiyle değil kendiliğinden
   koşar. Bir autoexec 3 gün sonra retry ederken `AUTH_API`'ye ihtiyaç duyar; `env.json`'ı
   elinde tutan app orada değildir. Aynı şekilde WFE'yi başlatan app ile 4. adımda onay veren
   portal kullanıcısı farklı taraflardır. "Start'ta gönder" mümkündür ama o da **saklamaktır**
   — `wfe` satırına şifresiz, örnek başına kopyalı, rotasyonsuz bir snapshot yazmak demektir.
2. **Çağıranın env *değerlerini* göndermesi bir enjeksiyon yüzeyidir.** WFE başlatabilen
   herhangi bir istemci publish edilmiş prod akışını `AUTH_API=https://benim-sunucum` diyerek
   kendi sunucusuna yönlendirip akışın POST ettiği ne varsa toplayabilir. Çağıranın **ortam
   adını** göndermesi (önceden onaylanmış kümeden seçim) bu yüzeyi açmaz ve K2'yi karşılar.

Elenen diğer seçenekler:

- **Deployment config (pod env var / mounted ConfigMap+Secret).** Kapsam kırılır: kurulum
  düzeyinde bir sözlüktür, tenant'a da WFD'ye de bağlı değildir; çok tenant'lı kurulumda
  tenant A `$env.B_API_KEY` yazıp B'nin anahtarını okur. Tenant'a göre bölünürse her yeni
  tenant/WFD için ops müdahalesi gerekir — self-servis biter, editörde "hangi anahtarlar var"
  gösterilemez. K3 ve K1'in ikinci yarısıyla çelişir.
- **Object storage'da JSON blob.** Gerçekten dosyadır ama payload için DB'den kesin olarak
  daha kötüdür: prod parolası bucket'ta düz metin, anahtar başına güncelleme
  read-modify-write, `updated_at`/audit yok. "Dosya olsun" hissi dışında üstünlüğü yok — ve o
  his zaten `GET`/`PUT` uçlarıyla korunuyor (Bölüm 5).

### K7 — Ortam başına ayrı doküman (inventory tarzı), tek `env.json` değil

Tek dosyanın (`{"test":{…},"prod":{…}}`) tek gerçek avantajı anahtar setinin eksiksizliğini
gözle görmektir; bedeli prod secret'larının test'i düzenleyenin elinde olmasıdır ve ortam
bazlı yetkiyi yapısal olarak imkânsız kılar. Ayrı doküman modeli izolasyonu yapısal verir; tek
dezavantajı olan **drift**'i validator publish-time hatasına çevirir (Bölüm 6) — bu, tek
dosyanın görsel garantisinden daha güçlüdür.

### K8 — GitLab'dan alınan dört kural

| GitLab | Bizdeki karşılığı |
|---|---|
| `environment_scope` — tam eşleşme kazanır, yoksa `*` joker'ine düşülür | Anahtar tüm ortamlar için `*` ile bir kez tanımlanır, yalnız farklı olan ortamda ezilir |
| **Masked** — değer job log'unda `[MASKED]` | Secret değerler `$exec` bilgisinden, `resolved_config()`'ten, `ExecFailure` metinlerinden ve `/autoexec/test` yanıtından maskelenir |
| **Hidden** — UI'da geri okunamaz, **ve yalnız oluştururken işaretlenebilir** | İkinci kural şarttır: sonradan çevrilebilseydi önce okunur sonra çevrilirdi |
| **Protected** — yalnız korumalı branch'lerde açılır | Yalnız **published** WFD koşumunda çözülür; draft/`simulate` prod secret'ını görmez |
| Maskeleme ön koşulu: tek satır, ≥8 karakter, boşluksuz | Aynen alınır — kısa/boşluklu değeri maskelemek log'u kullanılamaz hâle getirir |

## Bölüm 1 — Veri modeli

```sql
CREATE TABLE wf.environment (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    orgtnt_id  uuid NOT NULL REFERENCES org.orgtnt(orgtnt_id) ON DELETE CASCADE,
    name       text NOT NULL CHECK (name ~ '^[a-z][a-z0-9_-]*$'),
    label      text,
    is_default boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX environment_tenant_name  ON wf.environment (orgtnt_id, name);
CREATE UNIQUE INDEX environment_one_default  ON wf.environment (orgtnt_id) WHERE is_default;

CREATE TABLE wf.wfd_env_var (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id uuid NOT NULL REFERENCES wf.project(project_id) ON DELETE CASCADE,
    wfd_name   text NOT NULL,
    env_id     uuid REFERENCES wf.environment(id) ON DELETE CASCADE,  -- NULL = '*' joker
    key        text NOT NULL CHECK (key ~ '^[A-Z][A-Z0-9_]*$'),
    value_type text NOT NULL DEFAULT 'string'
               CHECK (value_type IN ('string','number','boolean')),
    value      text,    -- is_secret = false
    value_enc  bytea,   -- is_secret = true (AES-256-GCM, DB_CONN_SECRET)
    is_secret  boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((is_secret     AND value IS NULL AND value_enc IS NOT NULL)
        OR (NOT is_secret AND value_enc IS NULL))
);
CREATE UNIQUE INDEX wfd_env_var_key ON wf.wfd_env_var
    (project_id, wfd_name, key,
     COALESCE(env_id, '00000000-0000-0000-0000-000000000000'::uuid));
CREATE INDEX wfd_env_var_owner ON wf.wfd_env_var (project_id, wfd_name);

ALTER TABLE wf.wfe ADD COLUMN environment_id uuid REFERENCES wf.environment(id);
```

**Neden ortam adı tenant düzeyinde kayıtlı:** değerler WFD başına olsa da adın kendisi ortak
olmalı; yoksa bir WFD'nin `prod`'u ile diğerinin `Prod`'u sessizce farklı ortamlar olur ve
çağıran hangi adı geçireceğini bilemez. `is_default`, "çağıran geçirmezse ne olacak"ın cevabı.

**Neden `(project_id, wfd_name)`, `wfd_id` değil:** her versiyon ayrı `wfd_id` satırıdır;
conf'u `wfd_id`'ye bağlamak yeni versiyonda koparırdı. CLAUDE.md'de lokal `db_connection` için
yazılı olan kuralın aynısı — dolayısıyla `wf_wfd::repo::update_group_metadata` (grup adı
değişince taşıma) ve `repo::delete_draft` (son satır silinince temizleme) bu tabloyu da
kapsamalıdır.

**Neden `key` büyük harf:** `$env.AUTH_API` ile `$ctx.auth_api` gözle ayrılır ve enterpolasyon
token sınırını tartışmasız kılar (Bölüm 3).

**Neden `value_type`:** GitLab her şeyi string tutar; bizde bu `$env.TIMEOUT_MS > 1000`
karşılaştırmasında zen'in `Compare: Unsupported type` hatasını üretir — CLAUDE.md'de zaten
yazılı olan tuzağın aynısı. Tip kolonu bu sınıfı baştan kapatır.

**Migration ayrıca:** her tenant'a bir `default` ortamı seed eder ve `wfe.environment_id`'yi
ona backfill eder; mevcut örnekler ve tenant'lar hiç değişmeden çalışmaya devam eder. Backfill
sonrası kolon `NOT NULL`'a çekilir.

Bir "dosya" = `(project_id, wfd_name, env_id)` üçlüsünün satır kümesidir.

## Bölüm 2 — Ortamın runtime'a bağlanması

- **Start:** istek `environment: "prod"` (ad) alır. Verilmezse tenant'ın `is_default` ortamı.
  Ad tenant'ın kayıtlı ortamlarında yoksa `422 code:"environment.unknown"` — serbest metin
  kabul edilmez, tipo sessiz yeni ortam yaratmaz.
- **Örnek ömrü boyunca sabit.** `wfe.environment_id` start'ta yazılır, sonraki hiçbir uç
  değiştiremez. Timer sweeper, `tick_timers`, retry, escalation bu kolonu okur — K6'daki
  "çağıran yok" probleminin tek çözümü budur.
- **WFC:** çağrılan çocuk WFE **ebeveynin ortamını miras alır**, geçersiz kılınamaz. Aksi
  hâlde prod bir akış test ortamında bir çocuk koşturur.
- **Simulate:** `/wfe/simulate` istekte ortam adı alır, hiçbir yere yazmaz.

## Bölüm 3 — `$env` yüzeyi

### Enterpolasyon

`$env`, ara-değer çözülen **tek** namespace'tir: `"$env.AUTH_API/v1/users"` çalışır. Bugün
`$-string`'ler yalnız tam eşleşmedir (`runner.rs` `resolve_config_string` `match s`); bu
asimetri bilinçlidir ve iki gerekçeye dayanır:

1. Anahtar karakter kümesi `[A-Z][A-Z0-9_]*` olduğu için token sınırı tartışmasızdır — ilk
   küçük harf / `/` / `:` karakterinde biter, ayrıştırma belirsizliği yoktur.
2. `$env` değerleri her zaman skalerdir (`value_type` üçlüsü), `$ctx` gibi obje/dizi olamaz —
   "tam eşleşme mi enterpolasyon mu" tip çelişkisi doğmaz.

`$ctx` için aynısını yapmak ikisini de ihlal ederdi. Enterpolasyon daima string'e çevirir;
tam eşleşme (`"$env.TIMEOUT_MS"`) tipli değeri döndürür.

### Eksik anahtar hatadır, `null` değil

`$ctx.X` eksikse null okur — motorun yerleşik kuralı. `$env` bunun **istisnasıdır**:
`$env.MISSING` → `EngineError` / `ExecFailure`. Çünkü null bir domain
`https://null/v1/users` üretir ya da daha kötüsü yanlış bir hosta gider. Validator bunu
publish anında yakalar; runtime hatası son savunma hattıdır.

### Çözüm sırası

Tam eşleşme (`env_id = <koşum ortamı>`) > joker (`env_id IS NULL`) > tanımsız (hata).
Zincir katmanlı yazılır (K3) — tenant tabanı ileride joker'in altına eklenecektir.

### Secret'lar `EvalEnv` ve `EffectEnv`'e HİÇ girmez

| Yol | Secret olmayan | Secret |
|---|---|---|
| `AutoexecDef.config` (`resolve_config_value`) | ✅ | ✅ |
| ZEN ifadeleri (`when`, calc, `join_when`) | ✅ | ❌ |
| `wfes_effects` `$-string` (ctx'e yazar) | ✅ | ❌ |

Bu, maskeleme gereksinimini çalışma zamanı kontrolüyle değil **inşa yoluyla** karşılar: secret
bir değer ctx'e yazılamıyorsa portalda görünemez, `$exec` üzerinden sızamaz. Ayrı env
struct'ları bunu tip düzeyinde uygular.

Kalan sızıntı noktaları — `resolved_config()` (`/autoexec/test` için config'in çözülmüş hâlini
döndürür) ve `ExecFailure` metinleri (HTTP yanıt parçacığı içerir): çözülmüş secret değerlerin
listesi elde olduğu için değer bazlı `[MASKED]` ikamesi yapılır.

### Protected

Draft WFD ve `simulate` koşumunda secret anahtarlar çözülmez; kullanan autoexec
`env.secret_unavailable_in_draft` ile **başarısız olur**. Boş string'le devam edip dış sisteme
kimliksiz istek atmaktansa yüksek sesle patlamak doğrudur.

## Bölüm 4 — `db_connection` şablonlama

`host`, `port`, `database`, `username`, `options` ve çözülmüş `secret` içinde `$env.KEY`
çözülür. Tek `db_connection` satırı tüm ortamlara hizmet eder; WFD'de hiçbir değişiklik yok
(`config.connection` UUID'si aynı kalır), `db_connection`'a ortam kolonu eklenmez.

Çözüm noktası `runner.rs` `run_sql_on_connection` — satır okunduktan sonra, `DbConfig`
kurulmadan önce. Bu autoexec config yoludur, dolayısıyla **secret'lar dahil** çözülür.

**`port` kolonu `int` → `text`'e çevrilir**, çözümden sonra parse edilir; şablonsuz değerler
için CHECK sayısallığı korur. Gerekçe: "tüm bağlantı alanları şablonlanabilir" tek cümlelik
kuraldır, dokümante edilecek istisna yoktur. Maliyet: `Option<i32>` → `Option<String>` + parse,
birkaç dosyada mekanik değişiklik.

**Önbellek:** `LiveAutoexecRunner.registry` anahtarı bugün `(connection_id, updated_at)`.
Şablonlu bir bağlantı ortama göre farklı handle üretir; anahtar
`(connection_id, environment_id, updated_at)` olur.

`POST /db/connections/{id}/test` opsiyonel `environment` alır; verilmezse tenant varsayılanı.
UI'da bağlantı test düğmesinin yanına ortam seçici gelir.

## Bölüm 5 — API

Yeni top-level nest `/env`, `/db` ile aynı biçim (`orgtnt_id` query param) ve kimlik doğrulama
duruşu.

| Uç | İş |
|---|---|
| `GET/POST /env/environments?orgtnt_id=` | Tenant ortam kaydı (Ayarlar sayfası) |
| `PATCH/DELETE /env/environments/{id}` | Ad/etiket/varsayılan; son ortam silinemez |
| `GET /env/vars?orgtnt_id=&wfd_id=` | Matris: satır=anahtar, sütun=ortam (`*` dahil). Secret'lar `value:null, is_secret:true` |
| `GET /env/vars/{ortam}?wfd_id=` | Tek ortamın düz JSON objesi — indirilen "dosya" |
| `PUT /env/vars/{ortam}?wfd_id=` | Tam değiştirme — yüklenen "dosya" |
| `PATCH /env/vars/{ortam}?wfd_id=` | Anahtar bazlı upsert/sil |

`wfd_id` → `(project_id, wfd_name)` çözümü `/db/connections`'ın bugün yaptığının aynısıdır.
`GET`/`PUT` çifti, DB'de satır tutmamıza rağmen dosya yüzeyini verir: `env.prod.json` indirilip
başka bir kuruluma `PUT` ile taşınabilir (K1'in "ayrı kurulum" yarısı).

`is_secret` yalnız **oluşturmada** işaretlenebilir; `PATCH` ile mevcut bir değişken secret'a
çevrilemez (K8 "Hidden"). Secret değerler hiçbir `GET` yanıtında dönmez.

Maskeleme ön koşulu `PUT`/`PATCH`'te doğrulanır: secret değer tek satır, ≥8 karakter,
boşluksuz olmalı; değilse `422 code:"env.unmaskable_secret"`.

## Bölüm 6 — Validator

Core'a I/O'suz `validator::env_references(&Wfd) -> BTreeSet<String>` eklenir: dokümandaki tüm
`$env.*` referanslarını (autoexec config, ZEN ifadeleri, effects) toplar ve biçimi doğrular.
`validator::expression_issues` `$env` token'ını tanır — böylece CLAUDE.md'nin kuralı gereği
`POST /wfd/validate-expression` ve WFD validator'ı **aynı fonksiyondan** beslenir ve editör
anında uyarır.

DB'yle karşılaştırma server'da, publish ucunda yapılır (core I/O yapmaz):

- Referans kümesi × WFD'nin satırı olan her ortam → eksik anahtar `422 code:"env.missing_key"`.
- Hiç satırı olmayan tenant ortamı → uyarı.

Bu, K7'de kabul edilen drift dezavantajının karşılığıdır.

## Bölüm 7 — Şifreleme rotasyonu

`crypto.rs` `DB_CONN_SECRET`'ı virgülle ayrılmış liste olarak okur:

- Şifreleme daima **listenin ilki** ile.
- Çözme listedeki tüm anahtarlar sırayla denenerek; hiçbiri tutmazsa `CryptoError::Decrypt`.
- Tek anahtarlı mevcut kurulumlar (tek elemanlı liste) hiç değişmeden çalışır.
- Mevcut `db_connection.secret_enc` satırları da otomatik faydalanır.

## Bölüm 8 — Test stratejisi

- `crypto.rs`: eski anahtarla şifrelenen değer yeni listeyle çözülür; şifreleme daima ilkiyle;
  tek anahtarlı kurulum bozulmaz; hiçbiri tutmazsa temiz hata.
- `wfe-core`: enterpolasyon; eksik anahtar → hata; `value_type` tiplemesi; `*` joker'in tam
  eşleşmeye yenilmesi; secret'ın `EvalEnv`/`EffectEnv`'e girmediği regresyon testi.
- `tests/editor_zen_contract.rs` genişletilir (CLAUDE.md'nin işaret ettiği sözleşme testi).
- Server: bilinmeyen ortam → 422; WFC ortam mirası; timer sweeper'ın `wfe.environment_id`'yi
  okuması; draft/`simulate`'te secret reddi; publish-time eksik anahtar; `resolved_config()`
  ve `ExecFailure` maskelemesi.
- **Golden fixture değişmez** — `$env` opsiyonel bir yüzeydir, `kredi-basvuru` kullanmaz.

## Kapsam dışı (bilinçli)

- **Tenant çapında taban katmanı** (K3). Çözüm zinciri katmanlı yazılır, katman sonra eklenir.
- **Envelope encryption / KMS.** GitLab'ın yeni Secrets Manager'ı bunu yapıyor (şifreli data
  key ciphertext'in yanında, KEK bulut KMS'inde) ama bir KMS bağımlılığı getiriyor; bizim
  ölçeğimizde gereksiz.
- **Ortam bazlı yetkilendirme** (kim hangi ortamın conf'unu düzenleyebilir). Veri modeli buna
  hazır (ortam başına ayrı satırlar), kural sonra.
- **`python` / `lambda` autoexec tipleri** — bugün zaten desteklenmiyor.
