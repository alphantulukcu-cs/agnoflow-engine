# PLAN — İş Akışı Çağrısı (WFC): Alt Akış Node'u + Ardıl Akış

> Durum: **TASARIM / ONAY BEKLİYOR.** Kod yazılmadı. Onaylanan kısımlar
> `docs/spec/terminology.md`, `runtime-semantics.md`, `schema.json` ve
> `decisions.md`'ye taşınacaktır.
>
> **Rev 2 (bu sürüm):** `sync` modu **kaldırıldı**; üçüncü mod `terminal` oldu
> (bir akışın bitişi diğerinin başlangıcı). "Subflow" adı her yerden çıkarıldı —
> çünkü ardıl akış bir *ast* değildir. Katalog adı `subflows` → **`calls`**.

Bir WFE'nin başka bir WFD'yi çalıştırması. **Tek katalog (`calls`), tek `mode` ekseni,
üç mod:**

| `mode` | Nerede durur | Ne olur | Sonuç döner mi |
|---|---|---|---|
| **`wait`** | `nodes.<k>.call` | Çağıran o node'da **bekler**; çağrılan bitince kaldığı yerden devam eder | ✔ `$call.*` |
| **`detached`** | `nodes.<k>.call` | Çağrılan başlatılır, çağıran **hemen devam eder** | ✖ |
| **`terminal`** | `terminals[].call` | Çağıran **biter**; ardıl akış onun bittiği yerden başlar | ✖ (bitiş) |

Çağrılan akışın girdileri çağıranın context'inde bulunmak zorundadır — ya var olan
alanlara **maplenir** ya da context'e **otomatik üretilir**. Validator bunu zorlar.

---

## 1. TERMINOLOJİ (yeni kanonik terimler)

Mevcut kısaltma ailesine (WFD / WFE / WFES / WFAH / ACT / C_A / WFT / P_ACT) eklenir.

| Terim | Kısaltma | Tanım |
|---|---|---|
| **Workflow Call** | **WFC** | Bir WFE'nin başka bir WFD'yi örnekleme sözleşmesi. Root `calls` kataloğundaki bir kayıt — **ne çağrılacağını ve hangi girdiyle** söyler. *Nasıl* çağrıldığını `mode` söyler. Katalog↔referans ayrımı `autoexec`↔`trigger` ayrımının aynısıdır. |
| **Call Mode** | **WFC modu** | `wait` / `detached` / `terminal`. Çağrının **tek belirleyici eksenidir**; yerleşimi de mod belirler (§2.4). |
| **Caller** | **WFE-P** | Çağıran WFE. |
| **Sub Callee** | **WFE-C** | `wait`/`detached` ile yaratılan **alt** WFE. WFE-P yaşamaya devam eder. |
| **Successor Callee** | **WFE-N** | `terminal` ile yaratılan **ardıl** WFE. WFE-P zaten bitmiştir. **Ast değildir, ardıldır** — hiyerarşi değil sıra. |
| — | — | Üçünde de: ayrı satır, ayrı WFES, ayrı DynCtx — **paylaşılan durum YOKTUR**. |
| **Call Node** | **WFC node** | `call` bloğu taşıyan node (`mode: wait\|detached`). Bir bekleme *havuzu* değildir: bu node'dan insan ACT'i alınamaz. |
| **Handoff Terminal** | **WFC terminal** | `call` bloğu taşıyan terminal (`mode: terminal`). WFE-P bu terminal'de **normal biçimde sonlanır** (`completed`), ardından WFE-N başlar. |
| **Call Input Map** | **WFC-IN** | `calls.<key>.input` — çağrılanın start ACT girdilerinin, WFE-P bağlamındaki kaynaklarına eşlenmesi. Moddan bağımsızdır (§4 namespace kısıtı sayesinde) — bu yüzden aynı katalog kaydı üç modda da kullanılabilir. |
| **Projected Field** | **WFC-PROJ** | WFC-IN'in bir girdisi için WFE-P'nin `context.properties`'ine editörün **otomatik ürettiği** alan (çağrılanın şemasından kopyalanır). "Otomatik yaratma" modunun ürünü. |
| **Call Result** | **WFC-OUT** | WFE-C'nin `wfe_end_response`'unun WFE-P'ye taşınması. Namespace `$call.*`. **Yalnız `mode: wait`'te vardır.** |
| **Return Edge** | **WFC-RETURN** | WFC node'unun `call.wfes_effects` + `call.wft`'si. WFE-C bitince işleyen, insan ACT'i olmayan kenar. `escalation`/`claim_timeout` kenarlarıyla aynı sınıftır. |
| **Cascade** | **WFC-CASCADE** | WFE-P sonlandığında hâlâ koşan **WFE-C**'lerin `cancelled` edilmesi. **WFE-N'ye UYGULANMAZ** — ardıl, çağıranın ömrüne bağlı değildir. |
| **Nesting Depth** | **WFC-DEPTH** | Alt akış yuvalanma derinliği (`wait`/`detached`). Üst sınır 8. |
| **Successor Depth** | **WFC-NDEPTH** | Ardıl zincirinin uzunluğu (`terminal`). A bitince B, B bitince A → sonsuz WFE üretir; freni budur. Global üst sınır 16 veya sitedeki `max_next`. |
| **Handoff Isolation** | — | **Kural:** ardıl çağrı, WFE-P'nin sonucunu ASLA değiştirmez. WFE-N başlatılamasa bile WFE-P `completed` kalır; hata yalnız WFAH marker'ı + çağrı satırında görünür. |

**Expression namespace eklentisi** (mevcut `$ctx $wfah $node $actor $timestamp $wfe_id
$action.input.* $exec.result.*` listesine):

```text
$call.result.*   WFE-C'nin wfe_end_response'u            (yalnız WFC-RETURN bağlamında)
$call.status     "completed" | "failed" | "terminated" | "timeout"
$call.wfe_id     WFE-C'nin id'si
```

`$exec.result.*` ile **birleştirilmez** — autoexec (bir sistem çağrısı) ile WFC (bir WFE
örneği) ayrı kavramlardır. WFC-RETURN bağlamında `$action.input.*` **YOKTUR**
(SLA effects'teki `sla_effect_namespace` kuralının ikizi).

**Hata taksonomisi eklentisi:** `WFD.CallNotFound`, `WFD.CallUnauthorized`,
`WFD.CallFailed`, `WFD.CallTimeout`.

**Model özeti (terminology.md'deki bloğa eklenecek satırlar):**

```text
calls              = reusable WFD çağrısı katalogu (ne + hangi girdi)   ← YENİ
nodes.<k>.call     = alt akış çağrısı   (mode: wait | detached)         ← YENİ
terminals[].call   = ardıl akış çağrısı (mode: terminal)                ← YENİ
$call.*            = WFC-OUT namespace'i (yalnız mode: wait)            ← YENİ
```

---

## 2. WFD ŞEMA YÜZEYİ

**Katalog = NE çağrılır. Referans = NASIL çağrılır.** (`autoexec` ↔ `trigger` ayrımı.)
Bu yüzden aynı katalog kaydı hem alt akış hem ardıl olarak kullanılabilir.

### 2.1 Root katalog — `calls`

```json
"calls": {
  "kredi_skor_sorgusu": {
    "wfd_id": "kredi-skor",
    "version": "1.4.0",
    "description": "Skor akışını çalıştırır.",
    "start": "start__type_branch__branchClerk",
    "input": {
      "musteri_no":   "$ctx.basvuru.musteri_no",
      "talep_tutari": "$ctx.credit_info.amount_requested",
      "kaynak":       "ana-akis"
    }
  }
}
```

| Alan | Zorunlu | Not |
|---|---:|---|
| `wfd_id` | Evet | Çağrılacak WFD'nin id'si (aynı ORGTNT — cross-tenant yasak). |
| `version` | Hayır | **Onaylanan karar:** verilmezse çağrı anındaki **en son yayınlanmış** versiyon. Verilirse o versiyona pinlenir. Yaratılan WFE her hâlde start anında bir versiyona sabitlenir. |
| `start` | Koşullu | Çağrılanın `start[]` kuralı ≥2 ise zorunlu (`startRule.id`); tek start varsa opsiyonel. |
| `input` | Evet* | WFC-IN. Değer: `$ctx.<yol>` / `$wfe_id` / `$actor` / `$timestamp` / **literal**. `$action.input.*` **YASAK** (§4). *Çağrılanın start ACT'i hiç girdi bildirmiyorsa `{}`. |

`mode` ve süre sınırı **katalogda değildir** — referanstadır. Böylece katalog moddan
bağımsız kalır.

### 2.2 Alt akış çağrısı — `nodes.<key>.call` (`mode: wait | detached`)

```json
"self__skorBekleme": {
  "label": "Skor Bekleniyor",
  "c_a": { "c_orgu": "self", "c_r": ["creditAnalyst"] },
  "call": {
    "use": "kredi_skor_sorgusu",
    "mode": "wait",
    "timeout": "P2D",
    "wfes_effects": {
      "set": { "skor": "$call.result.skor", "skor_alindi": "$timestamp" }
    },
    "wft": {
      "conditions": [
        { "when": "$call.status != 'completed'", "node": "self__creditAnalyst" },
        { "when": "$call.result.skor >= 700",    "node": "self__branchManager" }
      ],
      "default": { "terminal": "terminal_rejected" }
    }
  }
}
```

| Alan | Zorunlu | Not |
|---|---:|---|
| `use` | Evet | `calls` katalog key'i. |
| `mode` | Hayır | `"wait"` (default) veya `"detached"`. |
| `timeout` | Hayır | ISO 8601 duration. `wait`'te aşılırsa WFC-RETURN `$call.status == "timeout"` ile işler (WFE-C iptal edilir). |
| `wfes_effects` | Hayır | WFC-RETURN effects. `detached`'ta `$call.result.*` boştur, `$call.status == "started"`. |
| `wft` | **Evet** | WFC-RETURN hedefi (`escalation_wft_required`'ın ikizi). Node **veya** terminal olabilir — SLA'nın "terminal hedef yasak" kısıtı burada geçerli DEĞİL. |

**Neden node'da, transition'da değil:** WFE-C günler sürebilir. Bekleme bir *durum*tur,
transition adımı değil. `WFES = current_node + assignment + DynCtx + WFAH` değişmezi
korunur; beklemenin kalıcı yeri `current_node`'dur.

**Neden `c_a` hâlâ zorunlu:** "Node key = slug(c_a)" ve "aynı canonical c_a ikinci
node'da olamaz" değişmezleri **hiç dokunulmadan** korunur. WFC node'unda `c_a`'nın anlamı
daralır: *"alt akış sürerken bu WFE'yi kim görür ve kim iptal edebilir"* — ACT/claim
vermez. (Start node'un "referans ile türetilmiş kimlik" desenine paralel: node'da `kind`
alanı yok; node'u WFC node'u yapan şey `call` bloğunun varlığıdır.)

### 2.3 Ardıl akış çağrısı — `terminals[].call` (`mode: terminal`)

> *"Bir iş akışının bitişi başka bir iş akışının başlangıcı."* Alt akış **değildir**:
> yuvalanma yok, dönüş yok, sahiplik yok. Sıradaki akış.

```json
{
  "id": "terminal_approved",
  "wfes_effects": { "set": { "onay_tarihi": "$timestamp" } },
  "wfe_end_response": { "sonuc": "onaylandi", "limit": "$ctx.onaylanan_limit" },
  "call": {
    "use": "kredi_kullandirim",
    "mode": "terminal",
    "start_as": "actor",
    "max_next": 1
  }
}
```

| Alan | Zorunlu | Not |
|---|---:|---|
| `use` | Evet | `calls` katalog key'i. |
| `mode` | Evet | `"terminal"`. Yerleşimden çıkarılabilir olsa da **açıkça yazılır**: JSON kendi kendini anlatır, editör/validator tek alan okur, ileride 4. mod eklenirse şema kırılmaz. |
| `start_as` | Hayır | `"actor"` (default) = terminal'e getiren ACT'in aktörü ile başlat; `"system"` = sistem aktörü. |
| `max_next` | Hayır | Ardıl döngüsüne **açık izin** + üst sınır (§5 `call_next_cycle` kaçışı). Verilmezse döngü validator tarafından reddedilir ve global WFC-NDEPTH sınırı geçerlidir. |

`wfes_effects` ve `wft` **yoktur** — dönecek bir yer yok.

**Sıralama (kesin):** terminal'in `wfes_effects`'i → `wfe_end_response` üretimi →
WFE-P `completed` olarak **commit** → *ondan sonra* WFE-N başlatılır.

- WFC-IN, terminal effects **uygulandıktan sonraki** ctx'e göre değerlendirilir. Ardıla
  taşınacak bir şey varsa önce terminal `wfes_effects`'i ile ctx'e yazılır — WOR-70'in
  "ctx'e tek yazma yolu effects" kuralıyla tam tutarlı.
- **Handoff Isolation:** WFE-N başlatılamazsa bile WFE-P `completed` kalır. Hata WFAH
  marker `call:next_failed` + çağrı satırı `status='failed'` olarak görünür.
- Yalnız `Terminal` sonucunda çalışır; `Failed` / `Terminated` (SLA ihlali, engine
  hatası) **ardıl tetiklemez** — bunlar başarılı bitiş değildir. *(Telafi akışı
  senaryosu için `on: [...]` alanı — §9.7 açık soru.)*
- **Ardıl şeffaf DEĞİLDİR:** WFE-P kendisi bir WFE-C ise (birisi `wait` ile bekliyorsa),
  bekleyen WFE-P'nin terminal'inde normal dönüşünü alır; ardıl bundan bağımsız, ayrı bir
  dal olarak koşar. Büyükbaba ardılı beklemez.

### 2.4 Mod ↔ yerleşim eşlemesi (validator zorunlu kılar)

```text
mode: wait      → yalnız nodes.<k>.call
mode: detached  → yalnız nodes.<k>.call
mode: terminal  → yalnız terminals[].call
```

`wait`/`detached` bir terminal'de, `terminal` bir node'da → `call_mode_placement`.

### 2.5 `sync` modu — DEĞERLENDİRİLDİ, KAPSAM DIŞI

Erken taslakta bir "trigger gibi bloklayan" mod (`sync`) vardı: WFC bir trigger kalemi
olarak koşar, runner çağrılanı başlatır ve `timeout_seconds` içinde bitmesini poll eder.
**Elendi**, çünkü:

- **Yeni yetenek getirmiyor.** Çağrılan tam otomatik ise `wait` zaten saniyeler içinde
  döner (WFE-C terminal commit'inde opportunistic nudge → sweeper beklenmez). Aynı sonuç,
  bloklamadan.
- **İçinde bir insan havuzu varsa daima `WFD.CallTimeout`** — ve validator "bu WFD
  insansız mı" diye statik karar veremez → sessiz üretim tuzağı.
- Pipeline'ı ve HTTP isteğini dakikalarca açık tutar; WFE-P'nin commit'i gecikir.

Şema `mode`'u serbest bir string enum olarak taşıdığı için ileride gerekirse kırıcı
olmayan biçimde eklenebilir.

### 2.6 WFC node'unda YASAK olanlar (validator)

- `transitions[].from` içinde yer almak (insan ACT'i alınamaz)
- `escalation`, `claim_timeout` (WFE-C'yi terkedip başka node'a taşımak yanlış — üst
  sınır için `call.timeout` ya da root `timeout`/SLA-3 kullanılır)
- `attachments`, `reassign`
- `start[].from` olmak (giriş node'u olamaz)
- `call.wft`'siz olmak

---

## 3. MODLARIN DEĞERLENDİRMESİ

Üçü **birbirinin alternatifi değil**. Ortak yüzey (katalog + WFC-IN + validator + editör
+ `wf.wfe_call` tablosu + outbox) üçünde de aynı; değişen tek şey devam mekanizması.

### `wait` — çağıran node'da bekler *(asıl ihtiyaç)*

WFC node'una girilir → WFE-C yaratılır → WFE-P o node'da `current_node` olarak durur →
WFE-C terminal'e ulaşınca WFC-RETURN işler ve WFE-P ilerler.

- ✅ İnsan adımı içeren alt akışları destekler (tek gerçekçi model)
- ✅ Pipeline atomikliği bozulmaz: WFE-C yaratımı WFE-P'nin commit tx'i **içinde** outbox
  satırı olarak stage edilir; gerçek start ayrı bir tx'te koşar
- ✅ WFE-P/WFE-C bağımsız sorgulanabilir, audit edilebilir
- ✅ Tam otomatik çağrılanda `sync` kadar hızlı (opportunistic nudge)
- ❌ En büyük kapsam: migration + resume + sweeper + cascade

### `detached` — ateşle ve devam et

WFE-C yaratılır, WFE-P **hemen** WFC-RETURN'e geçer. `$call.wfe_id` yazılır,
`$call.status == "started"`, `$call.result.*` boş.

- ✅ `wait` altyapısının ~%20'si; onunla birlikte pratikte bedava gelir
- ✅ Gerçek kullanım: "bildirim akışını başlat, sonucu beni ilgilendirmiyor"
- ❌ Sonuca göre karar vermek yok — tek başına hedef değil

### `terminal` — bitiş = ardılın başlangıcı *(yeni gereksinim)*

WFE-P terminal'de `completed` olur → WFE-N başlar. Dönüş, bekleme, cascade yok.

- ✅ Uzun süreçleri **ayrı ömürlü aşamalara** böler: "Başvuru" biter, "Kullandırım"
  başlar. WFE-P kapanır (raporlama/SLA/arşiv temiz), WFE-N sıfırdan ölçülür
- ✅ Mekanik olarak `detached`'ın **aynısı** — tek fark tetik noktası terminal commit'i.
  `detached` yapıldıysa marjinal maliyeti çok düşük
- ✅ Handoff Isolation sayesinde risk düşük: ardıl hatası WFE-P'nin sonucunu bozamaz
- ⚠️ **Tek gerçek tehlike: ardıl döngüsü.** A→B→A sonsuz WFE üretir; autoexec'te
  karşılığı olmayan yeni bir başarısızlık sınıfı. İki katmanlı fren **zorunlu**: statik
  `call_next_cycle` (reddet) + runtime WFC-NDEPTH (cap 16). Meşru döngü isteyen
  `max_next: N` ile açıkça izin verir
- ⚠️ `start_as: "actor"` ile taşınan aktörün c_a'sı WFE-N'nin start node'uyla eşleşmezse
  ardıl kopar (statik doğrulanamaz → runtime marker). SLA-3 ile de ulaşılabilen
  terminal'de `"system"` şart
- ⚠️ Görünürlük: "işim nerede?" artık iki WFE'ye yayılır → portal'da ardıl kartı **Faz
  2'de zorunlu** (Faz 3'e bırakılamaz)

### Karar

| | `wait` | `detached` | `terminal` | ~~`sync`~~ |
|---|---|---|---|---|
| Yerleşim | node | node | terminal | — |
| Sonuç döner | ✔ | ✖ | ✖ | elendi |
| WFE-P ömrü | sürer | sürer | **biter** | (§2.5) |
| Cascade | ✔ | ✔ | ✖ | — |
| Ek engine işi | Yüksek | Düşük | Çok düşük | — |
| Yeni risk | — | — | ardıl döngüsü | — |
| Faz | **2** | **2** | **2** | kapsam dışı |

**Üçü Faz 2'de birlikte teslim edilir** — aynı katalogu, aynı çağrı tablosunu ve aynı
outbox'ı paylaştıkları için ikinci ve üçüncünün marjinal maliyeti düşük.

---

## 4. WFC-IN — GİRDİ SÖZLEŞMESİ VE İKİ MOD

Kullanıcının vurguladığı çekirdek: *"çağrılan akışın input variable'ları çağıranın
context'inde de bulunmak zorunda; istersek mevcut context değerlerine maplenir, istersek
otomatik yaratılır."*

### Girdi kümesi nereden okunur

WOR-70 gereği zorunluluk **tek yerde** bildirilir: `actions.<ad>.input.required`.
Dolayısıyla çağrılanın girdi kümesi = `start[]` kuralının işaret ettiği ACT'in
`input.required` + `input.optional` listesidir. Alanların **tipi** çağrılanın
`context.properties`'inden okunur (start ACT'inin `wfes_effects`'i
`$action.input.<x>` → `ctx.<y>` eşlemesini verir; tip o `y` alanından gelir).

### Namespace kısıtı — neden `$action.input.*` yasak

WFC-IN'de yalnız `$ctx.<yol>`, `$wfe_id`, `$actor`, `$timestamp` ve literal'ler geçerli
(`call_input_namespace`). Gerekçe:

1. **Moddan bağımsızlık:** `mode: terminal`'de ACT girdisi güvenilir biçimde mevcut değil
   (SLA-3 ile ulaşılan terminal'de hiç yok). Yasak olunca aynı katalog kaydı üç modda da
   aynı anlamı taşır.
2. **WOR-70 ile tutarlılık:** ctx'e tek yazma yolu `wfes_effects`'tir. Bir ACT girdisini
   çağrılana geçirmek isteyen, onu önce effects ile ctx'e yazar — böylece "çağrılana ne
   gitti" DynCtx'te **denetlenebilir** kalır, uçucu bir ara değer olmaz.

### Mod 1 — MAP (mevcut alana eşle)

```json
"input": { "musteri_no": "$ctx.basvuru.musteri_no" }
```

Validator: kaynak WFE-P'nin `context.properties`'inde **bildirilmiş** olmalı ve tipi
çağrılanın karşılık gelen ctx alanıyla **uyumlu** olmalı.

### Mod 2 — PROJECT (WFC-PROJ — otomatik yarat)

Editörde "Otomatik oluştur": eşlenmemiş her girdi için WFE-P'nin `context.properties`'ine
çağrılanın şemasından **kopyalanan** bir alan üretilir ve kimlik eşlemesi yazılır:

```json
// çağıranın context.properties'ine editörün eklediği alan (WFC-PROJ)
"kredi_skor_sorgusu__musteri_no": { "type": "string", "description": "…" }
// ve WFC-IN satırı
"input": { "musteri_no": "$ctx.kredi_skor_sorgusu__musteri_no" }
```

- Çakışmayı önlemek için `<<call-key>>__` öneki (editör üretir, kullanıcı yeniden
  adlandırabilir).
- **Kritik sonuç (WOR-70 zinciri):** üretilen alanın ctx'e girmesi için birileri onu
  **yazmak** zorundadır. İki yol: (a) alanı WFE-P'nin daha önceki bir ACT'inin
  `input`'una ekleyip `wfes_effects` satırını da üretmek, (b) alanı bir
  autoexec/WFC-RETURN effects'inin hedefi yapmak. Editör (a)'yı önerir; hiçbiri yoksa
  mevcut `context_field_never_written` kuralı WFD'yi zaten reddeder — yani "otomatik
  yaratma" **yarım bırakılamaz**, bu bilinçli bir kilittir.
- Literal girdiler (`"kaynak": "ana-akis"`) hiçbir ctx alanı gerektirmez.

---

## 5. VALIDATOR KURALLARI (mevcut isimlendirme stiliyle)

| Kural adı | Reddettiği durum |
|---|---|
| `call_unknown_use` | `use` `calls` kataloğunda yok |
| `call_unused_catalog_entry` | Katalogda tanımlı ama hiçbir node/terminal referanslamıyor (autoexec'in ikizi) |
| `call_mode_placement` | `wait`/`detached` bir terminal'de, ya da `terminal` bir node'da (§2.4) |
| `call_input_missing` | Çağrılanın start ACT'inin bir `input.required` alanı WFC-IN'de yok |
| `call_input_unknown` | WFC-IN'de çağrılanın bildirmediği bir anahtar var |
| `call_input_source_undeclared` | `$ctx.<yol>` kaynağı çağıranın `context.properties`'inde yok |
| `call_input_type_mismatch` | Kaynak alanın tipi ile hedef ctx alanının tipi uyuşmuyor |
| `call_input_namespace` | WFC-IN'de geçersiz namespace: `$action.input.*` (§4), `$exec.result.*`, `$call.*`, `$node` |
| `call_version_not_published` | Pinlenen `wfd_id@version` yayınlanmış değil / bulunamıyor |
| `call_cross_tenant` | `wfd_id` başka ORGTNT'ye ait |
| **— alt akış (`wait`/`detached`) —** | |
| `call_wft_required` | `nodes.<k>.call.wft` yok |
| `call_result_unknown` | `$call.result.<k>` çağrılanın **hiçbir** terminal'inin `wfe_end_response`'unda yok |
| `call_result_in_detached` | `mode: detached`'ta `$call.result.*` kullanılmış (o modda daima boş) |
| `call_self_recursion` | `wfd_id` == çağıranın kendi id'si |
| `call_cycle` | Dolaylı yuvalanma döngüsü (A→B→A) — statik graf yürüyüşü (pin'li versiyonlarda tam, `version` yoksa "en son"a göre en iyi çaba + runtime WFC-DEPTH freni) |
| `call_node_has_action` | Bir `transitions[].from` WFC node'unu içeriyor |
| `call_node_forbidden_field` | WFC node'unda `escalation` / `claim_timeout` / `attachments` / `reassign` var |
| `call_node_is_start` | WFC node'u `start[].from` olarak kullanılmış |
| `call_effect_namespace` | `call.wfes_effects` içinde `$action.input.*` kullanılmış |
| **— ardıl (`terminal`) —** | |
| `call_next_cycle` | Ardıl döngüsü (A bitince B, B bitince A) **ve** `max_next` verilmemiş. `max_next` varsa döngü kabul edilir, runtime o sayıda durur |
| `call_next_self` | Terminal kendi WFD'sini ardıl yapıyor ve `max_next` yok |
| `call_next_start_actor` | SLA-3 / `Failed` / `Terminated` yoluyla da ulaşılabilen bir terminal'de `start_as: "actor"` — o yolda aktör yoktur, `"system"` şart |
| `call_next_forbidden_field` | Terminal `call`'ında `wfes_effects` / `wft` / `timeout` var (dönüş yok) |
| `call_next_result_ref` | Terminal `call`'ında `$call.*` kullanılmış — ardılda WFC-OUT yoktur |

Cross-WFD kurallar çağrılanın WFD'sini okumayı gerektirir → validator'a **opsiyonel bir
`WfdResolver` bağımlılığı** eklenir. Resolver verilmezse (saf `wfe-core` unit testleri)
yalnız yerel kurallar koşar, cross-WFD olanlar `skipped` işaretlenir. Upload yolunda
(`wfd` crate) resolver **daima** verilir — üretimde tam kontrol.

---

## 6. ENGINE / RUNTIME DEĞİŞİKLİKLERİ (crate bazında)

### `wfe-core`
- `types/wfd_v22.rs`: `Wfd.calls: BTreeMap<String, CallDef>`, `NodeDef.call:
  Option<NodeCall>`, `Terminal.call: Option<NextCall>`, `CallMode` enum + roundtrip testleri
- `v22/eval.rs`: `$call.*` namespace'i (yalnız WFC-RETURN bağlamında bind edilir)
- `v22/effects.rs`: `$call.result.*` / `$call.status` / `$call.wfe_id` çözümü
- `v22/pipeline.rs`:
  - WFT hedefi bir WFC node'u ise commit'e bir çağrı satırı stage edilir — WFE-P'nin
    **tek tx'i içinde**
  - `mode: detached` → aynı tx'te `$call.status = "started"` ile WFC-RETURN'ü **anında** çöz
  - **Ardıl:** `CommitOutcome::Terminal` üretilirken terminal'in `call`'ı varsa aynı tx'e
    çağrı satırı stage edilir. `Failed` / `Terminated`'da **stage edilmez**
  - yeni `Engine::fire_call_return(...)` — `fire_claim_timeout`/escalation yollarıyla aynı
    desen: system aktörü, `call:<key>` WFAH marker'ı, effects, `wft`, `TransitionCommit`
- `v22/ports.rs`:
  - `TransitionCommit`/`NewWfe`'ye `staged_calls: Vec<StagedCall>`; `NewWfe`'ye
    `caller: Option<CallLink>`
  - `WfeStore`'a: `start_pending_call`, `pending_call_returns`, `mark_call_returned`,
    `cancel_calls_of`
- `error.rs`: 4 yeni `WFD.Call*` varyantı

### `wfe`
- `WfeAdapter`: yeni store metodları + `create`/`commit` tx'lerine `wf.wfe_call` yazımı
  (aynı transaction — atomiklik korunur)
- `WfeExecutor`:
  - `start_pending_calls()` — `queued` satırlar için çağrılanı başlatır (idempotent:
    `UNIQUE(caller_wfe_id, site_kind, site_key)`)
  - `resume_returned_calls()` — `returned` satırlar için `fire_call_return`
  - ikisi de `tick_timers()` içinde; ayrıca çağrılanın terminal commit'inde
    **opportunistic nudge** (aynı istekte hemen çağır → 60s beklenmez)
  - WFE-P terminal/failed/terminated → `cancel_calls_of` **yalnız `wait`/`detached`
    satırları** (WFC-CASCADE ardılı kapsamaz)
  - `call.timeout` geçmiş `running` satırlar → WFE-C iptal + `$call.status="timeout"` ile dönüş
  - **İki ayrı derinlik sayacı:**
    - `depth` (WFC-DEPTH) — yuvalanma, cap **8**; `call_cycle` statik kaçarsa freni
    - `next_depth` (WFC-NDEPTH) — ardıl uzunluğu, cap **16** ya da `max_next` (küçük olan).
      Aşılırsa WFE-N **başlatılmaz**, WFE-P `completed` kalır, WFAH'a
      `call:next_depth_exceeded` marker'ı yazılır (Handoff Isolation)
  - `start_as` çözümü: `"actor"` → terminal'e getiren ACT'in aktörü (WFAH son kaydı),
    `"system"` → sistem aktörü. Aktör çağrılanın start node c_a'sıyla eşleşmezse
    `WFD.CallUnauthorized` → çağrı satırı `failed` + WFAH marker; WFE-P etkilenmez
- `sim`: WFC node'u simülasyonda **stub**lanır — kullanıcı `$call.result.*` değerlerini
  elle girer (SimInputFields deseni). Simülasyon gerçek WFE yaratmaz; ardıl çağrısı
  yalnız "burada <akış> başlayacak" notu olarak gösterilir

### `wfd`
- Upload/fetch kapısına `calls` desteği + validator'a resolver enjeksiyonu
- `call_version_not_published` için `wfd_meta` sorgusu
- **Yayın kilidi:** pin'siz çağrıda yeni versiyon yayınlamak koşan WFE'leri etkilemez
  (çağrılan start anında pinlenir) — `decisions.md`'ye madde olarak yazılacak

### `server`
- `GET /wfe/:id` yanıtına `calls: [{site_kind, site_key, call_key, mode, callee_wfe_id,
  status}]` ve çağrılanda `caller: {wfe_id, site_kind, site_key, call_key, mode}`
- Portal: WFE-P detayında alt akışa tıklanabilir bağlantı; **`mode: terminal` bitişinde
  "Bu akış tamamlandı → ardıl akış: \<link\>" kartı (Faz 2'de zorunlu)**
- `GET /wfe/:id/tree` (Faz 3) — yuvalanma **dikey**, ardıl **yatay** gösterilir; ikisi
  aynı ağaçta karıştırılmaz
- OpenAPI güncellemesi

### Migration — `migrations/wf/2026xxxx_wfe_call.sql`

Tek tablo üç modu da taşır — outbox, sweeper ve idempotency mantığı ortak kalsın.

```sql
CREATE TABLE wf.wfe_call (
  id             uuid PRIMARY KEY,
  orgtnt_id      uuid NOT NULL,
  caller_wfe_id  uuid NOT NULL REFERENCES wf.wfe(id),
  site_kind      text NOT NULL,          -- 'node' | 'terminal'
  site_key       text NOT NULL,          -- node slug'ı | terminal id'si
  call_key       text NOT NULL,          -- calls katalog key'i
  mode           text NOT NULL,          -- wait | detached | terminal
  callee_wfe_id  uuid NULL REFERENCES wf.wfe(id),
  -- queued → running → returned → consumed | cancelled | failed | skipped
  status         text NOT NULL,
  deadline       timestamptz NULL,
  end_response   jsonb NULL,             -- WFC-OUT (yalnız mode='wait')
  call_status    text NULL,              -- completed|failed|terminated|timeout|started
  depth          int  NOT NULL DEFAULT 0,   -- yuvalanma (cap 8)
  next_depth     int  NOT NULL DEFAULT 0,   -- ardıl uzunluğu (cap 16 / max_next)
  created_at     timestamptz NOT NULL DEFAULT now(),
  returned_at    timestamptz NULL,
  UNIQUE (caller_wfe_id, site_kind, site_key)   -- idempotent start
);
CREATE INDEX ON wf.wfe_call (status, deadline);
CREATE INDEX ON wf.wfe_call (callee_wfe_id);
CREATE INDEX ON wf.wfe_call (caller_wfe_id);
```

- `UNIQUE(caller_wfe_id, site_kind, site_key)` — çift start koruması. Terminal'de doğal
  olarak tekil (bir WFE bir kez biter). Node'da aynı WFC node'una ikinci giriş bu kısıt
  yüzünden engellenir; yeniden girişe izin verilecekse `attempt` kolonu eklenip UNIQUE
  dörtlüye çıkarılır. **Açık soru §9.2.**
- `depth`/`next_depth` yeni satıra çağıranın satırından **+1** ile taşınır (`wait`/
  `detached` `depth`'i, `terminal` `next_depth`'i artırır); kök WFE'de ikisi de 0.

---

## 7. FRONTEND (agnoflow-frontend)

### Palette — ayrı bölüm + ayrı renk

`i18n editor.json → palette`: `sectionHuman / sectionOto / **sectionWorkflow** /
sectionTerminal / sectionSwitch / sectionParallel`.

```json
"sectionWorkflow": "İş Akışı",
"itemCallTitle": "İş Akışı Çağır",
"itemCallDesc": "Başka bir akışı çalıştırır, sonucunu bekler",
"itemNextTitle": "Ardıl İş Akışı",
"itemNextDesc": "Bu akış bitince sıradaki akışı başlatır"
```

İki kalem, **tek bölüm**, aynı renk. "Ardıl İş Akışı" yalnız bir Terminal adımının
üzerine/çıkışına bırakılabilir; boş canvas'a bırakılırsa reddedilir + ipucu gösterilir.

Renk tokenı — mevcut palet: human violet `#9d7bff`, auto yeşil `#34d399`, terminal
kırmızı, switch amber, parallel cyan. Boş kalan ayırt edici hue **fuchsia**:

```css
/* dark */  --node-call: #e879f9;  --node-call-soft: rgba(232,121,249,0.13);
/* light */ --node-call: #c026d3;  --node-call-soft: rgba(192,38,211,0.10);
```
(Alternatif: mavi `#60a5fa` / `#2563eb`. Kontrast + violet'ten ayrışma göz kontrolü
uygulama sırasında yapılacak.)

**Görsel dil — kesikli kenar = yeni WFE sınırı.** Aynı WFE içinde akan her kenar düz,
başka bir WFE örneği doğuran her kenar **kesiklidir**: WFC node'undan çağrılana giden ok
kesikli (WFC-RETURN çıkışı düz kalır), terminal'den ardıla giden ok kesikli. Kullanıcı
"buradan sonrası ayrı bir akış örneği" bilgisini renk okumadan alır.

### Dosya bazında iş listesi

| Dosya | Değişiklik |
|---|---|
| `src/types/wfd.types.ts` | `CallStep` arayüzü (`type:'call'`, `mode:'wait'\|'detached'`, `catalogKey`, `wfdId`, `version?`, `startRuleId?`, `inputMap`, `wfes_effects?`, `timeout?`) + `WfdStep` union'a ekle. **Ardıl ayrı step tipi DEĞİL** — `TerminalStep.call?: NextCallMeta` + görselde türetilmiş ghost node (ParallelStep'in `__pjoin` ghost deseni) |
| `src/types/wfd-v22.types.ts` | `calls` katalogu + `nodeDef.call` + `terminalDef.call` tipleri (engine şemasının aynası) |
| `src/theme.css` | `--node-call` / `-soft` (dark + light) |
| `src/components/graph/CallStepNode.tsx` | **YENİ** — `AutoexecStepNode.tsx` (101 satır) şablonundan; ikon ⧉, çağrılan akış adı + versiyon rozeti. `variant: 'call' \| 'next'` — `next` varyantı kesikli çerçeve + "ardıl akış" etiketi, giriş handle'ı yok (terminal'e bağlı) |
| `src/components/graph/GraphTab.tsx` | `nodeTypes` + renk haritasına `callStepNode`; hover/handle CSS |
| `src/components/graph/LeftPanel.tsx` | Yeni palette bölümü + 2 kalem + arama filtresi + sürükle-bırak kaydı (ardıl kalemi için terminal drop-target kısıtı) |
| `src/hooks/useGraphNodes.ts` | `case 'call': return 'callStepNode'` + ardıl ghost node üretimi + `displayKind` |
| `src/components/graph/PropertiesPanel.tsx` | **Ana iş** — `CallStepContent`: WFD seçici (list API), versiyon seçici (*en son* / pinli), start kuralı seçici, `mode` seçici (`wait`/`detached`), **WFC-IN mapping tablosu** (her girdi için: mevcut ctx alanı seç / literal yaz / **"Otomatik oluştur"**), `$call.result.*` effects editörü, WFC-RETURN akış hedefi. `TerminalStepContent` içine **"Bitişte sıradaki akışı başlat"** bölümü: aynı seçici + mapping tablosu, `mode`/effects/wft yok; yerine `start_as` + `max_next` |
| `src/components/shared/CallConfigModal.tsx` | **YENİ** — mapping tablosu geniş; `AutoexecConfigModal.tsx` (708 satır) deseni. `mode`'a göre alan kümesi değişir |
| `src/hooks/useExport.ts` | `buildCallCatalog` (paylaşılan `catalogKey` → tek katalog kaydı; `buildAutoexecCatalog` ikizi) + `nodes.<k>.call` **ve** `terminals[].call` yazımı |
| `src/utils/wfdImport.ts` | `calls` + `nodes.<k>.call` + `terminals[].call` → step/ghost geri dönüşümü |
| `src/utils/validation.ts` | §5 kurallarının editör-tarafı aynası (canlı uyarı; upload'da engine son söz) |
| `src/store/wfd.store.ts` | Step CRUD + WFC-PROJ alan üretimi (context.properties'e yazma) |
| `src/api` | WFD listeleme + seçilen WFD'nin start ACT girdi şemasını ve terminal `wfe_end_response` anahtarlarını çekme (mapping tablosu + `$call.result.*` önerileri için) |
| `src/book/` (agobook) | Yeni bölümler: "İş Akışı Çağırma" + "Ardıl Akış" + WFC terminoloji sözlüğü |
| Portal (`work-pool-portal`) | WFE-P detayında "Alt akış çalışıyor" durumu + link; ardıl bitişte ardıl akış kartı |

---

## 8. TEST PLANI

- `wfe-core/tests/`: `call_types.rs` (roundtrip), `call_validator.rs` (§5'in her kuralı
  için bir pozitif + bir negatif), `call_pipeline.rs`
  (`#[tokio::test(start_paused = true)]`: girme → outbox → dönüş → wft; `detached`;
  timeout; cascade; WFC-DEPTH sınırı)
- `call_next.rs`: terminal → ardıl start; `Failed`/`Terminated` ardıl tetiklemez; Handoff
  Isolation (ardıl start hatası WFE-P'yi `completed` bırakır); `next_depth` sınırı;
  `max_next` ile izinli döngü tam N'de durur; cascade ardılı **iptal etmez**;
  `start_as: system` yolu
- Yeni fixture'lar: `docs/spec/examples/akis-cagrisi.golden.json` +
  `docs/spec/examples/ardil-akis.golden.json` + `wfe-core/tests/fixtures/` kopyaları.
  **`kredi-basvuru.golden.json` DEĞİŞTİRİLMEZ** — kural yürürlükte.
- `wfe` entegrasyon: sweeper'ı gerçek adapter ile (yerel psql yok → atılabilir
  `sqlx::raw_sql` binary'si ile şema uygula)
- Frontend: `roundtrip.test.ts`'e her iki mod için senaryo; `useExport` + `wfdImport`
  birim testleri; `validation.test.ts`'e mapping + mod/yerleşim kuralları

---

## 9. AÇIK SORULAR (uygulamaya geçmeden karar gerekiyor)

1. **Çağrılanı hangi Actor başlatır?** Öneri: `wait`/`detached`'ta WFC node'una girişi
   tetikleyen ACT'in aktörü; `terminal`'de `start_as`. Eşleşmezse
   `WFD.CallUnauthorized` (statik doğrulanamaz — org resolve runtime'dır).
2. **WFC node'una yeniden giriş** (döngüsel graf) olacak mı? `UNIQUE(caller_wfe_id,
   site_kind, site_key)` buna kapalı. Öneri: Faz 2'de kapalı, `attempt` kolonu ile
   sonradan açılır.
3. **`orgtnt_id`**: çağrılan her zaman çağıranın tenant'ında mı? Öneri: **evet**,
   cross-tenant yasak (validator `call_cross_tenant` + runtime).
4. **WFE-C iptal edilirse** WFE-P ne yapar? Öneri: `$call.status = "terminated"` ile
   WFC-RETURN normal işler (akış karar verir), WFE-P çökmez.
5. **Görünürlük:** çağrılanın DynCtx'i çağıranın aktörlerine görünür mü? Öneri:
   **hayır** — çağrılan kendi `x-visibility` kurallarıyla korunur; çağıran yalnız
   `wfe_end_response`'u (WFC-OUT) görür. V/authorization ayrımıyla tutarlı.
6. **`mode` adlandırması:** `"terminal"` mi, `"handoff"` / `"next"` mi? Kullanıcı
   `"terminal"` dedi; plan onu kullanıyor. Yerleşimden çıkarılabildiği için redundant —
   **açıkça yazılması bilinçli** (§2.3).
7. **Ardıl hangi bitişlerde tetiklenir?** Öneri: yalnız `Terminal` (başarılı bitiş).
   Alternatif: `call.on: ["completed","terminated","failed"]` — "iptal olduysa telafi
   akışını başlat" senaryosu isteniyorsa gerekli. **Öneri: Faz 2'de yalnız `completed`;
   `on` alanı Faz 3'e.**
8. **Ardılda ctx devri.** Şu an yalnız WFC-IN ile alan alan taşınıyor (açık ve
   denetlenebilir). "Tüm ctx'i ardıla kopyala" kısayolu (`inherit_ctx: true`) istenir mi?
   **Öneri: hayır** — WFD'ler arası şema bağımlılığını gizler ve validator'ın tip
   kontrolünü etkisizleştirir. Editördeki "Otomatik oluştur" zaten toplu eşleme sağlıyor.
9. **Ardılın kaynağı görünsün mü?** WFE-N'nin DynCtx'ine `_called_from` (WFE-P id +
   terminal id) yazılsın mı, yoksa yalnız `wf.wfe_call` satırında mı kalsın? **Öneri:
   yalnız tabloda** — DynCtx'e engine metadata sızdırmamak için
   (`context_field_never_written` ile de çakışır).
10. **`max_next` sayacı nerede sıfırlanır?** Öneri: **global `next_depth`** (basit ve
    güvenli); `max_next` bu global sayaca uygulanan yerel bir üst sınırdır.

---

## 10. FAZLAR

| Faz | Kapsam | Çıktı |
|---|---|---|
| **0** | Bu plan onayı + §9'daki 10 kararın verilmesi + terminology/runtime-semantics/schema.json'a spec yazımı | Spec commit'i |
| **1** | Ortak yüzey: tipler, `calls` katalogu, `nodes.<k>.call`, `terminals[].call`, **tüm validator kuralları**, resolver, 2 fixture; frontend palette bölümü + 2 kalem + node/ghost + PropertiesPanel + mapping tablosu + WFC-PROJ + export/import | WFD yazılabilir & doğrulanabilir (henüz koşmaz) |
| **2** | Runtime **üç mod birlikte**: `wf.wfe_call` migration, outbox, `fire_call_return`, terminal ardıl tetiği, sweeper, cascade (yalnız alt akış), `depth`/`next_depth` frenleri, `start_as`, API alanları, portal linki + **ardıl kartı**, sim stub | Çalışan özellik |
| **3** | Opsiyonel: `call.on[]`, `GET /wfe/:id/tree`, WFC node'una yeniden giriş, agobook bölümleri, (gerekirse) `mode:"sync"` | Cila |

---

## 11. DURUM (2026-07-30)

### Tamamlandı — Faz 1 engine tarafı

| Ne | Nerede |
|---|---|
| Tipler: `Wfd.calls`, `NodeDef.call`, `Terminal.call`, `CallDef`/`CallRef`/`CallMode`/`StartAs` | `crates/wfe-core/src/types/wfd_v22.rs` |
| `$call.result.* / $call.status / $call.wfe_id` namespace'i | `v22/eval.rs` (`CallOutcome`), `v22/effects.rs` |
| Hata taksonomisi: `WFD.CallNotFound / CallUnauthorized / CallFailed / CallTimeout` | `error.rs` |
| 24 validator kuralı + `WfdProvider` trait + `validate_with()` | `validator.rs` |
| WFC'nin mevcut kurallara entegrasyonu: cross_ref, BFS reachability, `no_exit` muafiyeti, wft koşulları, zen parse, effect yolları, effect yazar kümesi | `validator.rs` |
| 42 test (her kural için pozitif + negatif; sahte `WfdProvider`) | `crates/wfe-core/tests/calls.rs` |
| 3 fixture (çağıran + iki çağrılan), JSON Schema ile doğrulanmış | `docs/spec/examples/`, `crates/wfe-core/tests/fixtures/` |
| `calls` / `nodeDef.call` / `terminalDef.call` / `callDef` / `callRef` şeması | `docs/spec/schema.json` |
| Kanonik terminoloji + karar kaydı | `terminology.md`, `decisions.md` |

`cargo test --workspace`: tüm binary'ler temiz. **`kredi-basvuru.golden.json`
DEĞİŞTİRİLMEDİ** ve WFC'siz WFD'de hiçbir WFC kuralı tetiklenmiyor (regresyon testi:
`wfd_without_calls_triggers_no_call_rules`).

### Uygulama sırasında bulunan boşluk → Faz 2 girdisi

**`wf.wfd_meta` doküman kimliğini indekslemiyor.** Tablo WFD'yi
`(orgtnt_id, name, integer version)` ile saklar; `CallDef.wfd_id` dokümanın `id` alanına,
`CallDef.version` dokümanın semver `version`'ına atıfta bulunur. DB üzerinden çözüm için
`wf.wfd_meta`'ya indeksli `doc_id` (+ semver) kolonu gerekir — aksi halde her upload'da
tenant'ın tüm WFD JSON'larını okumak gerekirdi.

Sonuç: **DB-destekli `WfdProvider` Faz 2'ye alındı** (migration fazı). Faz 1'de resolver
trait'i ve cross-WFD kuralların tamamı hazır ve sahte katalogla test edilmiş durumdadır.
Kullanıcının çekirdek talebi olan *"çağrılanın girdileri çağıranın context'inde bulunmalı"*
kuralı **yerel**dir (`call_input_source_undeclared`) — resolver gerektirmez, şimdiden
çalışıyor.

### Tamamlandı — Faz 1 frontend (agnoflow-frontend)

| Ne | Nerede |
|---|---|
| `CallStep` + `TerminalStep.call` + `CallDefMeta`/`NextCallMeta` + `WfdParsed.calls` | `src/types/wfd.types.ts` |
| Fuchsia renk tokenı (dark+light) | `src/theme.css` (`--node-call`) |
| `CallStepNode` — **kesikli çerçeve** ("kesikli = yeni WFE sınırı") | `src/components/graph/CallStepNode.tsx` |
| Palette'te ayrı **"İş Akışı"** bölümü + ray kalemi + ardıl ipucu satırı | `src/components/graph/LeftPanel.tsx` |
| `CallStepContent` (akış/sürüm/start seçici, mod radyoları, WFC-IN tablosu, dönüş effects'i, c_a) + terminal ardıl bölümü (`start_as`, `max_next`, kök-timeout uyarısı) | `src/components/graph/PropertiesPanel.tsx` |
| `calls` katalogu + `nodes.<k>.call` + `terminals[].call` export/import | `src/hooks/useExport.ts`, `src/utils/wfdImport.ts` |
| Editör-tarafı ayna kurallar (9 kural) | `src/utils/validation.ts` |
| `upsertCallDef` / `removeCallDef` / `projectCallInput` (WFC-PROJ) | `src/store/wfd.store.ts` |
| Round-trip + sabit nokta + validasyon testleri (14) | `src/tests/calls.roundtrip.test.ts` |

`npx vitest run`: **959 test / 108 dosya** temiz. Yeni tip hatası yok.

**Frontend'de bulunan 4 gerçek hata** (planda öngörülmemişti):

1. `resolveWftTarget` CallStep'i tanımıyordu → bir çağrı node'una giden start/transition
   export'ta **sessizce düşüyordu** (wft `null` → kural atlanır).
2. ZEN ters ayrıştırıcı `$call.*`'ı ctx alanı sanıp `$ctx.call.*` yapıyordu →
   `RESERVED_ROOTS`'a `call` eklendi.
3. Editörün `allEffectTargets`'ı WFC-RETURN'ü yazar saymıyordu → yalnız çağrı
   sonucundan dolan alanlar "hiç yazılmıyor" hatası alıyordu (engine tarafındaki
   `collect_effect_targets` düzeltmesinin editör aynası eksikti).
4. `resolve_wft` (engine) döngü tespiti kökü çözemediği için A→B→A'yı kaçırıyordu.

### Tamamlandı — Faz 2 runtime (engine)

| Ne | Nerede |
|---|---|
| `wf.wfe_call` (outbox + çağıran↔çağrılan bağı) ve `wfd_meta.doc_id`/`doc_version` | `migrations/wf/20260730000001_wfe_call.sql` |
| `StagedCall`/`CallSite`/`CallLink`/`PendingCall`/`CallView` + 8 yeni `WfeStore` metodu + `WfdStore::resolve_doc` (hepsi varsayılan gövdeli → mevcut store'lar bozulmadı) | `wfe-core/src/v22/ports.rs` |
| `stage_calls` (outbox üretimi) + `fire_call_return` (WFC-RETURN) + `resolve_wft`'in 4. dönüş değeri `Option<CallSite>` | `wfe-core/src/v22/pipeline.rs` |
| `wf.wfe_call` repo'su (stage/tarama/durum/cascade/görünüm) | `wfe/src/repo/call.rs` |
| Outbox yazımı `create`/`commit` tx'i İÇİNDE + WFC store metodları | `wfe/src/wfe_adapter.rs` |
| `run_pending_calls` / `run_call_returns` / `expire_overdue_calls` / `after_wfe_settled` (cascade) + derinlik frenleri | `wfe/src/executor.rs` |
| Sweeper'a WFC taramaları (SLA tick'lerinden ÖNCE) | `wfe/src/timer.rs` |
| DB-destekli `WfdProvider`: geçişli çağrılan ön-yüklemesi + upload'da `validate_with` | `wfd/src/adapter.rs`, `wfd/src/repo.rs` |
| `GET /wfe/:id` → `calls` + `caller` alanları | `wfe/src/executor.rs` (`WfeView`) |
| 9 runtime testi (outbox, WFC-RETURN, timeout/failed/terminated dönüşü, ardıl, SLA-3'te ardıl yok) | `wfe-core/tests/call_runtime.rs` |

`cargo test --workspace`: 21 binary temiz.

**Faz 2'de bulunan 1 gerçek hata:** `detached` modda `call.timeout` yine mutlak
deadline'a çevriliyordu (guard `mode.is_node_site()` idi, `mode == Wait` olmalıydı) —
sonucu hiç beklenmeyen bir çağrı için süre sınırı hesaplanıyordu.

### Uygulama sırasında verilen iki karar (§9'un ötesinde)

1. **`start_as: "system"` = akışın BAŞLATICISI.** Nil bir sistem aktörü hiçbir `c_a` ile
   eşleşmez, yani ardıl asla başlayamazdı. Onun yerine WFAH'ın İLK kaydındaki aktör
   kullanılır: gerçek bir kullanıcıdır ve denetim izini anlamlı tutar. (`actor` modu
   WFAH'ın SON kaydını kullanır — "bu noktaya getiren kişi".)
2. **Çağrı ön-yüklemesi `MAX_PREFETCH = 64` ile sınırlı.** Döngü tespiti geçişli okuma
   gerektirir; bozuk/çok derin bir graf upload'ı kilitlemesin. Sınır aşılırsa uyarı
   loglanır ve runtime derinlik freni (cap 8 / cap 16) devreye girer.

### Tamamlandı — staging migration (2026-07-30)

`migrations/wf/20260730000001_wfe_call.sql` **staging'de uygulandı**
(`agnoflow-staging` / `agnoflow-postgres-0`, tek transaction, `ON_ERROR_STOP=1`):
`wf.wfe_call` (19 kolon, 6 CHECK, 2 FK, 5 indeks) + `wfd_meta.doc_id`/`doc_version`
+ `wfd_meta_doc_idx`.

**Geri doldurma gerekmedi:** o anda `wfd_meta`'da tek satır vardı ve `status='draft'`
("aa" adlı deneme taslağı). `resolve_doc` yalnız `status='published'` satır döndürdüğü
için o satır zaten çağrılabilir değildi. Genel kural yürürlükte: `doc_id`'si NULL kalan
eski satırlar yeniden yayınlanana kadar çağrılamaz (sessiz yanlış eşleşmeye yeğ tutulur).

Kod henüz staging'e **deploy EDİLMEDİ** — bu ayrı bir adımdır (`gitlab main:staging`) ve
açık "deploy" talimatı gerektirir. Şema geriye dönük uyumlu olduğu için mevcut image
sorunsuz koşmaya devam eder (yeni tablo/kolonlar okunmuyor).

### Tamamlandı — portal ardıl kartı (work-pool-portal)

| Ne | Nerede |
|---|---|
| `WfeCall` tipi + `WfeView.calls`/`caller` | `src/features/instances/api.ts` |
| **"Bağlı iş akışları"** kartı: ardıl EN ÜSTTE ve kesikli-fuchsia vurgulu ("Bu akış tamamlandı; sıradaki akış buradan devam ediyor" + "İşi aç" linki), altında alt akışlar durum etiketiyle | `src/features/instances/InstanceDetail.tsx` |
| Geriye bağlantı kartı: "Bu iş, tamamlanan bir akışın ardılı olarak başladı" / "…alt akışı olarak başladı" → çağıran işe link | aynı dosya |
| `callStatusLabel` — iç durum adlarını (`consumed`, `skipped`…) kullanıcı diline çevirir | aynı dosya |

`npx vitest run`: 79 test temiz, tsc temiz.

### Tamamlandı — executor entegrasyon testleri

`crates/wfe/tests/call_executor.rs` — 7 test, in-memory store `WfeAdapter`'ın WFC
semantiğini (outbox, mode'a göre `returned`/`consumed`, cascade'in ardılı atlaması)
taklit eder:

1. **Uçtan uca `wait`** — çağrı başlar, WFC-IN çağrılanın ctx'ine geçer, çağıran node'da
   bekler, çağrılan biter, dönüş işlenir, çağıran skoru yazıp müdür havuzuna ilerler.
2. **Ardıl** — çağıran `completed` KALIR, ardıl AYRI bir WFE olarak başlar, `next_depth`
   1 olur ama `depth` 0 kalır (yuvalanma artmaz).
3. **Handoff Isolation** — ardıl çözülemediğinde satır `failed`, çağıran `completed`.
4. **Ardıl derinlik sınırı** (`max_next`) → satır `skipped`, çağıran etkilenmez.
5. **Yuvalanma sınırı** (cap 8) → alt akış çağrısı `skipped`.
6. **WFC-CASCADE** — çağıranın kök süresi dolunca koşan alt akış `cancelled` ve
   çağrılanın WFE'si de sonlandırılır.
7. **`start_as: system`** — ardılı akışın BAŞLATICISI başlatır (son ACT'i alan müdür
   değil), yani nil sistem aktörü sorunundan kaçınılır.

`cargo test --workspace`: 22 binary temiz.

### Sırada

1. **Deploy** (açık talimat gerektirir): `git push gitlab main:staging` → CI image
   build → Flux. Şema hazır; kod deploy edilince WFC staging'de canlı olur.
2. **Faz 3 (opsiyonel):** `chain.on[]` (telafi akışı: `Failed`/`Terminated`'da da ardıl),
   `GET /wfe/:id/tree`, çağrı node'una yeniden giriş (`attempt` kolonu),
   agobook bölümleri ("İş Akışı Çağırma" + "Ardıl Akış").
3. **Editörde WFC-PROJ butonu** — `projectCallInput` store'da hazır ama panelde tetikleyici
   yok: çağrılanın girdi listesini API'den çekmek gerekiyor (`resolve_doc` artık var, bu
   yüzden Faz 3'te yapılabilir).


---

## 12. KAPANIŞ — spec + simülatör (2026-07-30)

Faz 0'ın ve Faz 2'nin son iki boşluğu kapatıldı.

### `runtime-semantics.md` §10 — WFC runtime semantiği yazıldı

Kanonik dosyaya 8 alt bölüm eklendi (CLAUDE.md: "spec ile kod çelişirse SPEC kazanır" —
o yüzden bu bir doküman borcu değil, sözleşme borcuydu):

| § | İçerik |
|---|---|
| 10 | Katalog ↔ referans ayrımı, mod ↔ yerleşim eşlemesi |
| 10a | **Outbox** — çağrı niyeti commit ile atomik; neden çağrılan tx İÇİNDE yaratılmaz; çift start koruması; gecikmenin neden anlık olduğu (ve `sync`'e neden gerek olmadığı) |
| 10b | WFC node'u bekleme HAVUZU değildir; `c_a`'nın daralan anlamı; yasaklar; graf kuralları |
| 10c | WFC-RETURN insan ACT'i olmayan kenardır; `$call.*`; "hata da bir dönüştür"; eskimiş dönüş |
| 10d | Ardılın **üç sert kuralı** (Handoff Isolation / cascade kapsamı / yalnız `Terminal`) + sıralama + `start_as` tablosu |
| 10e | İki ayrı döngü sayacı + statik/runtime fren matrisi + kenar-üzerinde tespit |
| 10f | WFC-IN sözleşmesi + `$action.input.*` yasağının iki gerekçesi |
| 10g | Versiyon çözümü (`doc_id` indeksi, pin'siz çağrı, NULL `doc_id` sonucu) |
| 10h | Validator'ın iki katmanı + `MAX_PREFETCH` |

### Simülatör artık çıkmaz sokak değil

**Sorun:** `staged_calls` sim'de sessizce yok sayılıyordu → akış çağrı node'una girip
orada takılı kalıyordu (çökmüyordu, ama ilerletmenin yolu yoktu).

**Çözüm — çağrılan akış simülasyonda KOŞTURULMAZ.** Bilinçli: çağrılanın kendi
aktörleri, kendi SLA'sı, kendi org çözümü olurdu; bu simülasyonun kapsamı değil. Onun
yerine çağrı "bekliyor" olarak durur ve kullanıcı sonucu ELLE girer.

| Ne | Nerede |
|---|---|
| `SimState.pending_calls: Vec<SimCall>` (`#[serde(default)]` — eski blob'lar bozulmaz) + `awaited_call()` / `clear_awaited_call()` | `wfe/src/sim.rs` |
| `POST /wfe/simulate/call-return` — `{status, result}` alır, `fire_call_return` koşar; bekleyen çağrı yoksa **409** | `server/src/routes/simulate.rs` |
| Test: çağrı görünür → WFC-IN çözülmüş sunulur → elle sonuç girilir → akış ilerler → satır listeden düşer | `wfe/tests/call_executor.rs` |

`SimCall` çözülmüş WFC-IN'i taşır: editör "çağrılana şu değerler gidecek" diyebilir.
`awaited` yalnız `mode: wait` için true — `detached`/`terminal` satırları akışı bloklamaz,
geçmiş kaydı olarak durur.

### Güncel durum

| Faz | Durum | Kalan |
|---|---|---|
| Faz 0 | **%100** | — |
| Faz 1 engine | **%100** | — |
| Faz 1 frontend | ~%80 | `wfd-v22.types.ts` aynası, `src/api` çağrılan-şeması, agobook, WFC-PROJ butonu |
| Faz 2 | ~%97 | OpenAPI açıklamaları (rotalar kayıtlı, şema alanları eksik) |
| Faz 3 | %0 | opsiyonel |

`cargo test --workspace`: 22 binary temiz (8'i WFC executor + simülasyon).


---

## 13. Akış seçici dropdown (2026-07-30)

**Sorun:** editörde çağrılacak akışın kimliği ELLE yazılıyordu. `calls.<key>.wfd_id`
dokümanın `id` alanına atıfta bulunuyor (DB uuid'sine değil) ve motor onu
`wfd_meta.doc_id` indeksinden çözüyor — elle yazım yayınlanmamış ya da hiç var olmayan
bir kimliğe kolayca sapıyordu ve hata ancak upload'da görülüyordu.

**Engine:** `wfd_meta.doc_id`/`doc_version` artık `WfdMeta` modelinde ve
`GET /wfd` listesinde. (Kolonlar Faz 2 migration'ında eklenmişti ama API'ye
açılmamıştı — dropdown'ın ihtiyacı tam buydu.)

**Frontend:** `src/hooks/useCallableWorkflows.ts`
- `useCallableWorkflows(excludeDocId)` — yayınlanmış akışları `doc_id`'ye göre gruplar.
  Filtreler bilinçli: `status='published'` + `is_active` + `doc_id != null` (motorun
  `resolve_doc`'unun gördüğü kümenin AYNISI — kullanıcıya çalışmayacak seçenek sunulmaz)
  ve **düzenlenen akışın kendisi hariç** (`call_self_recursion`).
- `useCalleeContract(target)` — çağrılanın dokümanını çekip start kurallarını ve girdi
  sözleşmesini çıkarır (WOR-70 zinciri: `start[]` → ACT → `input.required/optional`).

**Panelde:**
| Alan | Önce | Şimdi |
|---|---|---|
| Akış | metin girdisi | **dropdown** (`ad · doc_id`); katalogda olmayan seçili değer "listede yok" etiketiyle görünür kalır |
| Sürüm | metin girdisi | **dropdown** — "En son yayınlanan" + pinlenebilir semver'ler |
| Start kuralı | metin girdisi | **dropdown**, yalnız çağrılanın ≥2 kuralı varsa SORULUR (tek kural varsa motor kendi seçer) |
| Eksik zorunlu girdiler | görünmezdi | uyarı kartı + **"Eksik alanları otomatik oluştur"** (WFC-PROJ) |

Akış değişince sürüm ve start kuralı birlikte sıfırlanır — aksi halde başka akışın
kuralı seçili kalırdı.

**WFC-PROJ butonu bağlandı** (§11'de "panelde tetikleyici yok" olarak borç kalmıştı):
eksik girdiler için `projectCallInput` çağrılır. Buton yanındaki not, üretilen alanın
bir aksiyon tarafından doldurulması gerektiğini söyler — `context_field_never_written`
kilidi bilinçlidir, kullanıcı bunu bilerek devam eder.

10 yeni test (`hooks/__tests__/useCallableWorkflows.test.ts`): draft/`doc_id`-null/
`is_active=false` elenmesi, kendini eleme, sürüm gruplaması ("en son" = en yüksek DB
versiyonu, liste sırasına güvenilmez), start kuralı çıkarımı, bozuk dokümanda çökmeme.

Frontend: **969 test / 109 dosya** temiz. Faz 1 frontend ~%80 → **~%95**
(kalan: `wfd-v22.types.ts` pasif aynası, agobook bölümleri).
