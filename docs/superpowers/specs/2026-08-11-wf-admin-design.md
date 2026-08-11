# WF Admin — akış-içi yetkili (T‑A5, T‑A6)

**Tarih:** 2026-08-11
**Kapsam:** `agnoflow-engine` (wfe-core + wfe + server + schema). Editör/portal yüzeyi ayrı iş.
**Görevlendirme:** T‑A5 ("WF Admin rolü"), T‑A6 ("doğrudan claim ettiren yetkili").

## 1. Amaç ve duruş

WF Admin, **bir akışın çalışması sırasında** bazı kararları uygulayabilen yetkilidir:
tıkanmış bir işi birinden alıp başkasına verir, escalation sayaçlarına müdahale eder.
Yetki WFD ayarlarında tanımlanır ve **havuz bazlıdır** — sabit bir kişi/rol listesi değil,
`c_a` gibi çalışma anında çözülen bir kural.

**agnoflow platform admini ile KARIŞTIRILMAMALIDIR.** Platform admini (`X-Admin-Key`,
`/org`, `/db`) sistemi yönetir. WF Admin tek bir akış örneğinin gidişatına müdahale eder;
yetkisi o akışın WFD'sinden ve o WFE'nin durumundan doğar.

Tipik kurgular:

- "Akışı başlatan memurun olduğu şubedeki genel müdür" — akıştan akışa DEĞİŞEN bir kişi
- "Akışı başlatan personelin kendisi"
- "100 bin üstü akışlarda bölge müdürü" (`when` guard'ı ile)

Bunlar statik bir rol grant'ıyla ifade EDİLEMEZ: yetkili, o akış örneğinin geçmişine
(kimin başlattığı, hangi birimde) göre belirlenir. Bu yüzden mekanizma tenant permission
havuzu (`org.p`, 2026-08-11 spec'i) değil, **C_A kuralıdır**. Görevlendirme T‑A5'i "rol
grant'ı" diye yazsa da doğru yer burasıdır.

## 2. Kural: WFD kökünde `wf_admin[]`

`listable[]`'ın birebir kardeşi — aynı şekil, aynı matcher, aynı `when` guard'ı.

```json
{
  "wfd_version": "2.2",
  "wf_admin": [
    { "c_a": { "c_orgu": { "from": {"wfah": "start", "field": "actor.orgu",
                                   "occurrence": "first"},
                           "traverse": "self" },
               "c_r": ["genel-mudur"] } },
    { "c_a": { "c_u": [{ "from": "$ctx.baslatan" }] },
      "when": "$ctx.tutar > 100000" }
  ]
}
```

### 2.1 Kararlar

**Dizi, tek kural değil.** `listable[]` deseni: "çoklu grant = çoklu kayıt". "Şube genel
müdürü VEYA başlatanın kendisi" tek `c_a` ile ifade edilemez — §3'ün "C_A TEK KURALDIR"
değişmezi bir kural içinde VEYA'ya izin vermez, iki kayıt olur.

**Yeni kural dili YOK.** `authorize()` aynen kullanılır; `c_orgu` / `c_r` / `c_u`
semantiği (çapasız biçim dahil) değişmez. Tasarımcı `c_a`'yı biliyorsa `wf_admin`'i de
biliyor.

**Node key'lere DOKUNMAZ.** Node key = `slug(node.c_a)`; `wf_admin` kökte durur, hiçbir
key değişmez. `#[serde(default)]` sayesinde alanı taşımayan belgeler birebir aynı
serileşir (golden fixture etkilenmez).

**Tip paylaşımı.** `wf_admin` ve `listable` kuralları aynı şekle sahiptir (`{c_a, when?}`).
İki özdeş struct yerine nötr adlı tek struct: `CaGrantRule`; `ListableRule` ona alias
olur. JSON değişmez, kural şekli tek yerde durur.

### 2.2 "Akışı başlatan kişinin kendisi" — bugünkü yol

`c_u` yalnız `$ctx` referansı okur (`resolve_cu_ident`); `$wfah` anchor'ı YOKTUR
(`c_orgu`'da vardır). Bu yüzden başlatanı kişi olarak hedeflemek için akışın başında
`wfes_effects` ile ctx'e yazmak gerekir:

```json
"wfes_effects": { "set": { "baslatan": "$actor" } }
```

sonra `c_a: { "c_u": [{ "from": "$ctx.baslatan" }] }`.

Simetrik bir `$wfah` anchor'ı (`{wfah: "start", field: "actor.user"}`) daha zarif olurdu
ama `c_u`, node key'lerini üreten `slug(c_a)`'nın girdisidir; oraya yeni bir biçim eklemek
key kararlılığı riski taşır. Faz 1'de ctx yolu kullanılır, sugar ayrı iş (§7).

## 3. Yetkiler

### 3.1 Reclaim / devir (T‑A6)

[pipeline.rs:845](crates/wfe-core/src/v22/pipeline.rs#L845) bugün "reassign kuralı VAR
olmalı VE reassigner uymalı" diyor. Kapı iki yollu olur:

```
yetkili = node.reassign eşleşir  VEYA  wf_admin[] içinden biri eşleşir
```

- Node'un kendi `reassign` kuralı olmasa bile WF Admin devredebilir — "tek yerde ayarla"
  gereksinimi budur.
- Mevcut belgelerin davranışı DEĞİŞMEZ: `reassign` yazılıysa aynen işler; `wf_admin` yoksa
  hiçbir kapı açılmaz (403).
- **Hedef kontrolü korunur:** hedef, o node'un `c_a`'sına uymak zorundadır
  (`TargetNotEligible`/400). Gerekçe: `apply_action` aksiyon anında `c_a`'yı yeniden
  sorar — uymayan kişiye iş verilirse claim'i tutar ama hiçbir aksiyon alamaz. İş görünür
  biçimde o kişide asılı kalır ve WF Admin sorunu çözdüğünü sanarak akışı kilitler.
- WFAH marker'ının `input`'una `"via": "wf_admin"` eklenir. Bugün `reassign` marker'ına
  bakıp "node amiri mi, akış admini mi" ayırt edilemiyor; denetimde bu fark gerekir.

T‑A6 ("amir işi bir kişiye direkt claim'letsin") bununla kapanır: hedefli `reassign` tam
olarak odur, `wf_admin` ile artık akış genelinde çalışır.

### 3.2 Görünürlük

`can_view`'a **(e)** kriteri eklenir: `wf_admin[]` kurallarından birine authorize VE
`when` (varsa) true. (d) — `listable[]` — ile yapısal ikizdir.

Neden ayrı kriter, "tasarımcı ayrıca `listable` yazsın" değil: yönettiği akışı göremeyen
admin işe yaramaz, ve aynı kuralı iki yere yazdırmak kaçınılmaz olarak birinin
güncellenip diğerinin unutulmasıyla biter.

### 3.3 Escalation müdahalesi

| Uç | İş |
|---|---|
| `POST /wfe/:id/escalation/fire` | Sıradaki escalation adımını şimdi uygula |
| `POST /wfe/:id/escalation/skip` | Sıradaki adımı atla (geçiş UYGULANMAZ) |

**Hedef adım İKİSİNDE DE "sıradaki ateşlenmemiş adım"dır** — `next_escalation`'ın
döndürdüğü `step_idx`. Adım numarası istemciden ALINMAZ: alınsaydı sıra dışı bir adımı
tetiklemek (0 beklerken 2'yi ateşlemek) mümkün olurdu ve escalation adımlarının sıralı
olma sözleşmesi kırılırdı. Vadenin gelmiş olması ŞART DEĞİL — erken tetikleme bu ucun
varlık sebebi.

Gövde `reassign`'ın konvansiyonunu izler; yanıt hangi adıma dokunulduğunu söyler:

```json
// istek (ikisi de aynı)
{ "node": "<kol node key>" }        // opsiyonel; yalnız paralel modda anlamlı

// yanıt
{ "step_idx": 0, "node": "onay", "marker": "escalate:onay:0" }
```

- Yetki: **yalnız `wf_admin`**. `node.reassign` bunları AÇMAZ — devir ile sayaç yönetimi
  farklı güçlerdir.
- Paralel mod: `node` ipucu escalation kol-bazlı ateşlendiği için gerekir (WOR-31);
  ipucu olmadan hangi kolun sayacına dokunulduğu belirsizdir. WFE paralel modda ve ipucu
  yoksa `400`. Paralel modda DEĞİLKEN gönderilen ipucu yok sayılır.
- Bekleyen adım yoksa `409` + `escalation.none_pending`.
- Terminal/settled WFE'de ikisi de `WfeTerminal`/409 — biten akışın sayacı yönetilmez.

**Öteleme YOK.** İki yolu da reddettik: adım başına offset yeni kalıcı durum ister ve
zamanlama mantığını iki kaynaktan besler; "saati sıfırla" ise WFAH tabanını örtük şekilde
kaydırır. "Henüz eskalasyon olmasın" ihtiyacı atlama ile karşılanır. İleride gerekirse
§7'deki nota bakılır.

### 3.4 `GET /wfe/:id` escalation tahminini döndürür

`WfeView` bugün `deadline` / `claimed_at` / `claim_deadline` taşıyor, sıradaki
escalation'ı TAŞIMIYOR. `next_escalation` zaten
`EscalationForecast { step_idx, entered_at, deadline, overdue }` üretiyor; yalnız dışarı
verilmiyor. Görmediği bir sayacı yönetmesini istemek admini kör karar vermeye zorlar.

## 4. Marker'lar ve denetim izi

**Elle tetikleme otomatik yolun AYNI marker'ını yazar** (`escalate:<node>:<idx>`).
Bilinçli: yayınlanmış akışlar `count($wfah, #.action == "escalate:...")` ile karar
verebiliyor; elle tetiklemeye ayrı ad vermek o sayımları bozar. Ayrım **aktörde** durur —
otomatik yolda system aktörü (nil uuid), elle tetiklemede adminin kendisi. (Yan etki:
admin WFAH katılımcısı olur ve `can_view` (b) ile akışı görmeye devam eder — tutarlı.)

**Atlama marker'ı `escalate:<node>:<idx>:skipped`.** Önekin `escalate:` olması ZORUNLUDUR:
`next_escalation` node giriş zamanını "son **escalation-dışı** WFAH kaydı"ndan hesaplıyor
([pipeline.rs:995](crates/wfe-core/src/v22/pipeline.rs#L995)), yani başka bir adla yazılan
atlama marker'ı tabanı kendine kaydırır ve **tüm sayaçları sessizce sıfırlar**. `fired`
kontrolü iki adı da kabul edecek şekilde genişler:

```rust
e.action == marker || e.action == format!("{marker}:skipped")
```

Atlama **adım başınadır**: 0. adım atlanınca 1. adımın sayacı aynı tabandan işlemeye
devam eder. "Escalation'ı komple kapat" diye bir şey yoktur; her adım tek tek atlanır.
WFAH append-only olduğundan kim ne zaman atladı kaydı kalıcıdır.

## 5. WF Admin'in YAPAMADIKLARI

Kapsamı sabitlemek için açıkça:

- **Akışı bitirmek/iptal etmek** — akışı yalnız aksiyonlar ve SLA-3 (root `timeout`)
  bitirir. Böyle bir yetki tasarımcının çizdiği grafiğin dışına çıkardı.
- **Rastgele bir node'a taşımak** — hedef yalnız escalation adımının kendi `wft`'sidir.
- **`$ctx`'e yazmak** — tek yazma yolu hâlâ `wfes_effects` (WOR-70).
- **Kendi adına aksiyon uygulamak** — bunun için node'un `c_a`'sına uyması gerekir.
  WF Admin olmak aksiyon yetkisi VERMEZ.

Son madde tasarımın özeti: **WF Admin işi yönetir, işi yapmaz.**

## 6. Kod yerleşimi ve testler

| Dosya | İş |
|---|---|
| `crates/wfe-core/src/types/wfd_v22.rs` | `Wfd.wf_admin: Vec<CaGrantRule>`; `ListableRule` alias |
| `crates/wfe-core/src/v22/visibility.rs` | (e) kriteri |
| `crates/wfe-core/src/v22/pipeline.rs` | `reassign` kapısı VEYA; `skip_escalation` → `WfahEntry` |
| `crates/wfe-core/src/validator.rs` | `wf_admin[]` → `listable`'ın aynı denetimleri |
| `crates/wfe/src/executor.rs` | Orkestrasyon + `WfeView.next_escalation` |
| `crates/server/src/routes/wfe.rs`, `routes/portal/wfe.rs` | İki uç, iki ağaçta |
| `docs/spec/schema.json` + `agnoflow-frontend/src/schema/wfd.schema.json` | Kök `wf_admin` (birlikte güncellenir) |
| `docs/spec/terminology.md`, `docs/spec/decisions.md` | Sözleşme metni |

`skip_escalation` `reassign`'ın desenini izler: core `WfahEntry` üretir, store append eder.

### 6.1 Saf testler (DB'siz — asıl güvence)

1. `wf_admin` eşleşen aktör, `node.reassign` OLMAYAN node'da devredebilir.
2. `wf_admin` eşleşmeyen aktör → `Unauthorized`.
3. `node.reassign` var, `wf_admin` yok → **eskisi gibi** çalışır (regresyon kapısı).
4. Hedef `node.c_a`'ya uymuyor → `TargetNotEligible` (wf_admin yolunda da).
5. Marker `via: "wf_admin"` taşır; `node.reassign` yolunda taşımaz.
6. `can_view`: wf_admin eşleşen viewer görür; `when` false ise görmez.
7. Atlanan adım bir daha ateşlenmez.
8. **Atlama tabanı KAYDIRMAZ** — atlamadan sonra sonraki adımın deadline'ı değişmemiştir
   (§4'teki tuzağın testi).
9. Elle tetikleme otomatik yolun aynı marker'ını yazar, aktör admin'dir.
10. Terminal WFE'de iki uç da reddedilir.
11. Paralel modda kol ipucu yoksa hata.
12. `wf_admin` içermeyen belge birebir aynı serileşir (golden fixture).

Zamana bağlı testler `#[tokio::test(start_paused = true)]` ile koşar.

### 6.2 DB gerektiren yollar

Rota kabukları ve commit yolları birim testiyle kapatılamaz (bu repoda DB'li test
koşulmuyor); canlı duman testiyle doğrulanır ve sonucu buraya eklenir.

## 7. Reddedilen alternatifler ve kapsam dışı

**Node başına `wf_admin`.** Reddedildi: `node.reassign` zaten o iş. İstenen, akış
ayarlarında TEK yerde tanımlamak.

**Yeni C_A kanalı (`c_admin`).** Reddedildi: "C_A TEK KURALDIR" değişmezini bozar ve
matcher'a dallanma ekler.

**WF Admin'i tenant permission havuzuna bağlamak** (`org.p` / `org.rp`). Reddedildi:
havuz tenant'ın STATİK iş yetkileridir; "başlatanın şubesindeki müdür" gibi akış örneğine
göre değişen bir yetki orada ifade edilemez. İki mekanizma iki farklı soruyu cevaplar —
havuz "bu kişi neye yetkili?", `wf_admin` "bu AKIŞTA kim müdahale edebilir?".

**Escalation öteleme.** §3.3.

**Akışı bitirme yetkisi.** §5.

**Kapsam dışı:** `c_u`'ya `$wfah` anchor sugar'ı (§2.2 — `slug` riski); T‑A4
(WFD‑Observer / WFD‑Admin, agnoflow'un kendi yetkileri ekseni); editör yüzeyi (WFD
ayarları sekmesinde `wf_admin` kural kurucusu) ve portal ekranları.
