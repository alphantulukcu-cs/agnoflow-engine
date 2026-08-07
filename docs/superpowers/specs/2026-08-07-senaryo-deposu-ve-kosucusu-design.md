# Senaryo deposu ve sunucu koşucusu (executable senaryo)

**Tarih:** 2026-08-07
**Kapsam:** `agnoflow-engine` (`wfd` storage · `wfe` koşucu · `server` rotaları) · `agnoflow-frontend` (Simülasyon sekmesi / ScenarioSection)
**İlgili görevler:** T‑B1 (senaryolara dizin yapısı), T‑B3 (senaryolar kaydedilebilmeli), T‑B5 (ağaçta parent'a gitme) — `gorevlendirme.md`

---

## 1. Sorun

Bu projede **senaryo** = kaydedilmiş bir simülasyon koşusu: başlangıç girdisi + aktör +
adımlar + beklenti. Tek tıkla uçtan uca tekrar koşturulur ve beklentiyle karşılaştırılır.
Yani WFD'nin regresyon testi.

Bugün çalışan hâli:

- Şekil `WfdScenario` = `{id, name, startInput, startActor?, steps[{action, input, actor}], expect{terminal?, contextContains?}}`
  (`agnoflow-frontend/src/utils/scenarioSidecar.ts`).
- Depo **yalnız `localStorage`** — `wfd-scenarios:<wfd_id>` anahtarı (a.g.e. `KEY_PREFIX`).
- Koşucu **tarayıcıda**: `simulateWfe` önce `/wfe/simulate/start`, sonra her adım için
  `/wfe/simulate/apply` çağırır ve `sim_state`'i elden ele taşır
  (`agnoflow-frontend/src/api/engineApi.ts`).
- Beklenti denetimi de tarayıcıda (`checkScenarioExpectations` + `deepContains` +
  `inferTerminalId`).
- Sunucu tarafında senaryo diye bir şey **yok** (`grep -ri scenario crates/` → sıfır).

Beş boşluk:

1. **Senaryolar kaybolur ve paylaşılamaz.** Başka makine/tarayıcı = senaryo yok. Takım
   arkadaşı aynı WFD'yi açtığında testleri göremez. Tarayıcı verisi temizlenince gider.
2. **Sunucusuz koşulamaz.** Senaryo ancak editör sekmesi açıkken çalışır; CI'dan,
   cron'dan, başka bir servisten tetiklenemez.
3. **Dizin yok.** Bir WFD'nin onlarca senaryosu düz listede boğulur.
4. **Paralel akışlar senaryolaştırılamaz.** `/simulate/apply` kol seçimini `node` alanıyla
   destekliyor (WOR‑31) ama `ScenarioStep` bu alanı taşımıyor.
5. **WFC'li akışlar senaryolaştırılamaz.** Alt akış çağrısında simülasyon
   `/simulate/call-return` verilene kadar bilinçli olarak durur; senaryonun bu durağı
   ifade edecek bir adım çeşidi yok.

Ayrıca birden çok start kuralı olan bir WFD'de senaryo hangi kuralın seçileceğini
söyleyemez (`/simulate/start` `action` alanını M16 ile destekliyor, senaryo taşımıyor) ve
`$env` bağlanamaz (`/simulate/*` gövdeleri `orgtnt_id + wfd_id + environment` alıyor).

## 2. Karar

Senaryolar **layout ile aynı desende** bir sidecar'a taşınır ve **koşucu sunucuya iner**.

Motor yeni bir yetenek kazanmıyor: simülasyon API'si zaten durumsuz (`sim_state` gidip
geliyor), yapılan iş bugün tarayıcıdaki döngünün Rust'ta yazılmasıdır.

### 2.1 Neden dokümanın İÇİNE değil, yanına

Senaryolar WFD JSON'unun gövdesine konmaz:

- `(wfd_id, version)` **immutable**'dır ve cache'lenir; yayınlanmış bir akışa yeni test
  eklemek yeni versiyon publish etmeyi gerektirirdi.
- Doküman v2.2 validator'ından geçiyor; golden fixture değişmez kuralı var (CLAUDE.md).

Emsal hazır: editör layout'u `{orgtnt}/wfd/{wfd_id}/{version}.layout.json` anahtarında,
dokümanın yanında duruyor (`crates/wfd/src/storage.rs::layout_key`), opak JSON olarak
okunup yazılıyor (`adapter.rs::save_layout` / `fetch_layout`), yeni draft'a best-effort
kopyalanıyor (`adapter.rs::new_draft_from`).

### 2.2 Neden tek dosya, S3 dizini değil

Seçenek B, senaryoyu obje başına bir S3 anahtarında tutmak ve dizini gerçek anahtar
öneki yapmaktı (`…/{version}/scenarios/Onaylar/Müdür/limit.json`). Reddedildi:

| | Tek dosya | Çok obje |
|---|---|---|
| Listeleme | tek okuma | `list` + N obje okuma |
| Yazma | atomik | obje başına; klasör yeniden adlandırma = N kopyala+sil, yarıda kalırsa ağaç bozulur |
| Yeni draft | tek kopya | N kopya |
| Eşzamanlı düzenleme | son yazan kazanır | farklı senaryolarda çakışma yok |

Çok objenin tek kazancı (bağımsız yazma) bu ölçekte işe yaramıyor — bir WFD'nin senaryo
sayısı onlarca mertebesinde. Ve o kazanç **aynı** senaryoyu iki kişinin düzenlemesini
zaten çözmüyor; kilit ayrı bir iştir (T‑B4), depolama şekli onu belirlemez.

### 2.3 Versiyon bağı

Senaryolar `(wfd_id, version)`'a bağlıdır — layout gibi. Yeni draft açılınca kopyalanır;
yayınlanmış versiyonun seti kendi versiyonunda kalır.

Alternatif (senaryo setinin versiyonların üstünde, `(project_id, wfd_name)` sahipliğinde
durması) değerlendirildi ve seçilmedi: bu turda amaç senaryoyu kalıcı ve koşulabilir
kılmak; versiyonlar arası regresyon karşılaştırması ("v1 yeşildi, v2 kırmızı") ayrı bir
yetenek ve koşu geçmişi deposu ister (§6).

## 3. Senaryo dosyası

Anahtar: `{orgtnt_id}/wfd/{wfd_id}/{version}.scenarios.json`
(`storage::scenarios_key`, `layout_key`'in tıpkısı).

```json
{
  "scenarios_version": "1",
  "scenarios": [
    {
      "id": "3f2a…",
      "name": "Limit aşımı — müdür onaylıyor",
      "path": "Onaylar/Müdür",
      "description": "150k üstü başvuruda müdür onayı zorunlu",
      "environment": "test",
      "startActor": { "orguId": "…", "userId": "…", "role": "musteriTemsilcisi" },
      "startAction": "basvur",
      "startInput": { "tutar": 150000 },
      "steps": [
        { "action": "onayla", "actor": { "orguId": "…", "userId": "…", "role": "mudur" }, "input": {}, "node": null }
      ],
      "expect": { "terminal": "onaylandi", "contextContains": { "durum": "onaylı" } }
    }
  ]
}
```

**Alan adları bugünkü `WfdScenario` ile aynı tutulur** (`startInput`, `startActor`,
`steps`, `expect`) — mevcut localStorage dizisi `{scenarios_version:"1", scenarios: <dizi>}`
sarmalayıcısına konunca dönüştürmesiz yüklenir. Aktör de depolandığı gibi **camelCase**
kalır (`{orguId, userId, role}`); motorun snake_case `Actor`'una çevirme bugün editörde
olduğu gibi (`scenarioActorToSimActor`) koşu anında yapılır, artık Rust tarafında.

**Aktörsüz adım** bugünkü davranışı korur: senaryo/adım aktörü eksikse çağıranın verdiği
yedek aktöre düşülür (editör bunu `readStoredEngineConfig`'ten alıyor). Koşu uçlarının
gövdesinde bu yüzden opsiyonel `fallback_actor` bulunur; o da yoksa senaryo
`"adım N için aktör çözülemedi"` ile kalır (hata değil, başarısız senaryo).

Eklenen alanlar:

| alan | zorunlu | gerekçe |
|---|---|---|
| `path` | hayır (boş = kök) | dizin yapısı (T‑B1/B5) |
| `description` | hayır | senaryonun ne test ettiği |
| `environment` | hayır | `$env` bağlama; verilmezse boş ortam |
| `startAction` | hayır | birden çok start kuralı olan WFD'de determinizm (M16) |
| `steps[].node` | hayır | paralel modda kol seçimi (WOR‑31) |

### 3.1 Dizin `path` ile temsil edilir

Ağaç ayrı bir `folders[]` listesiyle değil, senaryonun üstündeki `path` dizesinden
**türetilir** (`"Onaylar/Müdür"`). Böylece öksüz klasör / döngülü parent gibi tutarsız
durumlar tanım gereği oluşamaz ve elle JSON yazan için okunaklı kalır.

Bedeli kabul edildi: **boş klasör olamaz** — klasör ancak içinde senaryo varsa vardır.
Taşıma = `path` dizesini değiştirmek, klasör yeniden adlandırma = önek toplu değiştirme.

### 3.2 Adımın iki çeşidi

`steps[]` ayrık bir birleşimdir:

```json
{ "action": "onayla", "actor": { "…" }, "input": {}, "node": null }
{ "call_return": { "status": "completed", "result": { "skor": 82 } } }
```

`call_return`, WFC alt akış çağrısındaki durağı ifade eder (`/simulate/call-return`
gövdesiyle aynı alanlar: `status` ∈ `completed|failed|terminated|timeout`, `result`
yalnız `completed` için anlamlı). Bu varyant olmadan alt akış çağıran hiçbir WFD uçtan
uca koşturulamaz.

Ayrımın serde temsili `#[serde(untagged)]` bir enum'dur (`CuItem` ile aynı desen):
`action` anahtarı taşıyan nesne aksiyon adımı, `call_return` taşıyan nesne çağrı dönüşü.

## 4. API

| Uç | Yetki | İş |
|---|---|---|
| `GET /wfd/{id}/{ver}/scenarios` | `require_design_on_wfd` | Sidecar'ı döner; blob yoksa boş set (hata değil) |
| `PUT /wfd/{id}/{ver}/scenarios` | `require_design_on_wfd` | Setin tamamını yazar (atomik) |
| `POST /wfd/{id}/{ver}/scenarios/{sid}/run` | `require_design_on_wfd` | Tek senaryoyu uçtan uca koşar |
| `POST /wfd/{id}/{ver}/scenarios/run` | `require_design_on_wfd` | Seti koşar; gövdedeki `path_prefix` ile daraltılabilir |

Koşu uçlarının gövdesinde **opsiyonel `wfd`** bulunur:

- verilirse o doküman koşar — editördeki **kaydedilmemiş** hâl (bugünkü davranış; asıl
  değeri burada),
- verilmezse depodaki `(id, ver)` dokümanı koşar — CI/otomasyon yolu.

Doküman her iki yolda da `routes/simulate.rs::parse_and_validate` ile aynı kapıdan geçer.
Koşu hiçbir şey yazmaz: `sim` durumsuzdur, WFE yaratılmaz, WFAH'a iz düşmez.

Yanıt:

```json
{ "results": [ {
  "id": "3f2a…", "name": "…", "ok": false,
  "failures": ["terminal beklendi \"onaylandi\", gelen \"reddedildi\""],
  "steps_executed": 2, "terminal": true, "terminal_id": "reddedildi",
  "dynctx": { "…": "…" }
} ] }
```

**`GET`'e yetki konur.** `GET /wfd/{id}/{ver}/layout` bugün kimlik doğrulaması istemiyor
(`routes/wfd.rs::get_layout`); senaryolar aktör kimlikleri ve iş girdileri taşıdığı için
o hâl kopyalanmaz. Layout'un kendi durumu bu turda değiştirilmez, ayrı iş olarak not
edilir.

**Yayınlanmış versiyonda senaryo yazılabilir.** Sidecar doküman değildir; yayınlanmış bir
akışa test eklemek akışı değiştirmez. Layout da böyle davranır (`get_meta_any`, status'e
bakmaz).

### 4.1 Yaşam döngüsü

- `new_draft_from`: layout gibi senaryo seti de yeni drafta best-effort kopyalanır.
- `delete_draft`: doküman blob'unun yanında senaryo sidecar'ı da silinir. **Bitişik
  düzeltme:** bu fonksiyon bugün layout blob'unu silmiyor, öksüz bırakıyor
  (`adapter.rs::delete_draft` yalnız `meta.s3_key`'i siliyor). Aynı satırda layout da
  temizlenir — yanına yeni bir öksüz eklemek anlamsız olurdu.

## 5. Koşucu

**Yer: `crates/wfe/src/scenario.rs`** (yeni modül). `wfe` crate'i `sim`'i zaten
barındırıyor; senaryo onun doğal komşusu. İçerik:

- tipler: `ScenarioSet`, `Scenario`, `ScenarioStep`, `Expect`, `ScenarioResult`
- koşucu: `run(&Engine, &Wfd, &Scenario) -> ScenarioResult`
- saf beklenti denetimi: `check_expectations`, `deep_contains`, `infer_terminal_id`

I/O yok — HTTP yok, DB yok, dosya yok. Testler `crates/wfe/tests/scenario.rs` altında
ağsız koşar.

Sunucu ucu ince sarmalayıcıdır: sidecar'ı oku (ya da gövdedeki `wfd`'yi al) → `Engine`'i
`sim_start`'ın kurduğu gibi kur (`OrgAdapter`, `LiveAutoexecRunner`,
`routes::env::resolve_run_env`) → koşucuyu çağır.

### 5.1 Adım mantığının ortaklaştırılması (mevcut kodda hedefli düzeltme)

Adım mantığı bugün route handler'larının **içinde** yaşıyor: `sim_start` (`engine.start`
→ `SimState::from_new_wfe`), `sim_apply` (`engine.apply` → `apply_commit`),
`sim_call_return`. Koşucu bunları yeniden yazarsa aynı döngü iki yerde yaşar ve zamanla
ayrışır — simülasyonda geçen bir senaryo koşucuda kalabilir.

Bu mantık `wfe::sim`'e yardımcı fonksiyonlar olarak çıkarılır (`start_step`, `apply_step`,
`call_return_step`); hem `routes/simulate.rs` hem `scenario::run` onları çağırır. Route
handler'ları HTTP gövdesi ↔ yardımcı çağrısı çevirisine iner.

### 5.2 Beklenti denetimi Rust'a taşınır

`checkScenarioExpectations` (ve iç yardımcısı `deepContains`) TypeScript'ten **silinir**;
editör sunucudan gelen `failures[]`'i gösterir. Gerekçe `validate-expression` ile aynı:
**kural motorun, editör aynı cevabı gösteren taraf**. İki yerde yaşayan denetim iki farklı
cevap üretir.

`inferTerminalId` / `terminalCandidatesFromSerialized` / `dynctxToRecord` ise **kalır** —
bunları `SimulationTab` senaryodan bağımsız olarak, interaktif canlı koşuda ulaşılan
terminali göstermek için kullanıyor. Rust tarafında `infer_terminal_id`'nin bir kopyası
koşucu için gerekir; iki uygulama aynı sözleşmeyi paylaşır (etkileri dynctx'in alt kümesi
olan TEK terminal aday; birden çoksa `null`). Bu ikizleme bilinçlidir ve editör testiyle
(`scenarioSidecar.test.ts`) Rust birim testi aynı vakaları kapsar.

`deep_contains` sözleşmesi bugünküyle birebir korunur: nesneler **alt küme** (recursive),
diziler ve skalerler **tam eşleşme**. Hata metinleri sunucuda üretilir; i18n anahtarı
değil düz metin döner (editör bugün `i18n.t` ile üretiyor — bu tur metin sunucudan gelir).

## 6. Kapsam dışı (bilinçli)

- **T‑B4 draft kilidi.** Ayrı iş; depolama şekli onu belirlemiyor. Bugünkü `PUT /wfd/draft`
  son-yazan-kazanır davranışı bu turda değişmiyor, senaryo `PUT`'u da aynı davranışta.
- **Koşu geçmişi** ve versiyonlar arası karşılaştırma ("v1 yeşildi, v2 kırmızı"). Ayrı
  tablo ister.
- **Sunucu tarafında zorlanan publish kapısı.** Publish kapısı **zaten var ama istemci
  tarafında**: `TopBar.tsx` yayından önce `loadScenarios(wfdId)` ile senaryoları okuyup
  hepsini tarayıcıda koşturuyor ve biri kalırsa yayını durduruyor. Bu tur o kapıyı yeni
  uca **taşır** (§8, zorunlu iş — taşınmazsa kapı sessizce etkisizleşir), ama kapıyı
  API'de zorlamaz: `publish` ucu senaryo koşturmaz. Bir kullanıcı doğrudan
  `POST /wfd/draft/{id}/{ver}/publish` çağırarak kapıyı bugün de atlayabiliyor; bu tur
  o durumu değiştirmiyor.
- **CI boru hattı.** Uç hazır olur; entegrasyon ayrı iş.
- **WFD'ler arası paylaşılan senaryo.**
- **`GET /layout`'un kimlik doğrulamasız oluşu.** Not edildi, dokunulmuyor.

## 7. Kabul kriterleri

1. Senaryolar sunucuda saklanır; başka tarayıcıdan aynı WFD açıldığında görünür.
2. `POST …/scenarios/run` tek çağrıda seti koşar ve her senaryo için `{ok, failures[]}`
   döner; tarayıcı gerekmez (`curl` ile doğrulanabilir).
3. Gövdede `wfd` verilerek editördeki kaydedilmemiş doküman koşturulabilir; verilmeden
   depodaki versiyon koşturulabilir.
4. Paralel kol seçimi (`node`) ve WFC çağrı dönüşü (`call_return`) içeren senaryolar
   uçtan uca koşar.
5. Senaryolar `path` ile klasörlenir; editörde ağaç görünür ve parent'a gidilebilir.
6. localStorage'daki mevcut senaryolar **açık bir "içeri aktar" eylemiyle** sunucuya
   taşınır (otomatik/sessiz yükleme yok).
7. Tasarım izni olmayan kullanıcı senaryo okuyamaz/yazamaz/koşturamaz (403).
8. Yeni draft açılınca senaryo seti kopyalanır; draft silinince sidecar da silinir.
9. Publish kapısı çalışmaya devam eder: kırık senaryosu olan bir taslak editörden
   yayınlanamaz (kapı yeni uca taşınmış, sessizce boş listeye düşmemiştir).
10. `cargo test --workspace` geçer; **golden fixture değişmemiştir**.

## 8. Editör tarafı

- `scenarioSidecar.ts` localStorage yerine `GET/PUT …/scenarios` kullanır; `loadScenarios`
  / `saveScenarios` imzaları async olur.
- `ScenarioSection` `path`'lerden klasör ağacı türetir; breadcrumb ile parent'a gitme
  (T‑B5). Taşıma = `path` düzenleme; sürükle-bırak yok (YAGNI).
- Koşma tek çağrıya iner (`runOne` → `POST …/{sid}/run`, `runAll` → `POST …/run`); bugünkü
  N adım = N gidiş-geliş kalkar.
- **`TopBar.tsx` publish kapısı yeni uca taşınır — zorunlu iş, unutulursa sessiz gerileme.**
  Kapı bugün `loadScenarios(wfdId)` ile localStorage'dan okuyor ve senaryoları tarayıcıda
  koşturuyor (satır 222-274). Senaryolar sunucuya taşındığında bu okuma **boş liste**
  döner; kapı hata vermeden tümüyle etkisizleşir ve kırık senaryolu WFD'ler yayınlanır
  hâle gelir. Kapı `POST …/scenarios/run` (gövdede `wfd` = serialize edilmiş taslak)
  çağrısına dönüşür; `failedNames` sunucudan gelen sonuçlardan üretilir.
- **Göç:** ilk açılışta localStorage'da senaryo varsa ve sunucuda set yoksa açık bir
  "içeri aktar" düğmesi çıkar. Otomatik yükleme yok — iki kişinin tarayıcısındaki farklı
  setler sessizce birbirinin üstüne yazabilirdi.
- Kaydedilmemiş doküman bugünkü gibi `serializeWfdPreview` çıktısıyla gövdede gider.

## 9. Testler

**Engine (`crates/wfe/tests/scenario.rs`)** — fixture WFD üzerinde: geçen senaryo, kalan
senaryo (terminal uyuşmazlığı / `contextContains` eksiği), `deep_contains` tablo testleri
(alt küme vs tam eşleşme), paralel kol seçimi, `call_return` adımı, `startAction` ile
start kuralı seçimi, adım ortasında terminale ulaşan senaryonun kalan adımları atlaması.

**Storage/adapter** — sidecar yaz/oku/yok, `new_draft_from` kopyalar, `delete_draft` siler.

**Route** — tasarım izni olmayan 403 (dört uçta da), gövdede `wfd` verilen ve verilmeyen
iki yol, yayınlanmış versiyona yazma, bozuk sidecar JSON'unda anlamlı hata.

**Editör** — `path` → ağaç türetme, göç düğmesi (sunucuda set varken çıkmaz), sonuç
gösteriminin sunucudan gelen `failures[]`'i kullanması ve **publish kapısının kalan
senaryoda yayını durdurması** (kapının etkisizleşmesini yakalayan test).

Golden fixture DEĞİŞMEZ.
