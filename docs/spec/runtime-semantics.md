# WFD Custom Validator & Runtime Semantics — Named Nodes Model v2.2

`schema.json`'ın yakalayamadığı kuralları tanımlar; önceki tüm sürümlerin yerini alır. Referans implementasyon: `reference-types.rs` (slug + matcher + kabul testleri).

---

## 1. Cross-Reference Validation

v2.1 ile aynı: `from`→nodes, `action`→actions, `trigger[].use`→autoexec, tüm `wft.node/terminal` (conditions, default, escalation dahil)→nodes/terminals. Unique: node key'leri, `start[].id`, `transitions[].id`, `terminals[].id`, action/autoexec key'leri; node ve terminal id'leri global namespace'te çakışmaz.

`terminals[].id` ek olarak **case-insensitive unique** olmak zorunda ("Start" ile "sTaRT"
aynı isim sayılır — bkz. decisions.md "Terminal id = kullanıcı adı"). Editör terminal
id'yi kullanıcının girdiği isimden üretir (`assignTerminalKeys`), bu yüzden case-insensitive
uniqueness authoring-time'da zaten sağlanır; validator bunu editör-dışı üretilen dokümanlar
için de kesin kural olarak uygular. Runtime lookup (`wft.terminal` çözümü, §7 pipeline)
case-sensitive exact-match'tir — case-insensitivity SADECE bu uniqueness kuralı için geçerli.

## 2. Node Identity Validation (v2.2)

### 2a. Canonical Slug Algoritması

Node key, node'un `c_a`'sından şu şekilde türetilmelidir (editör üretir, validator yeniden hesaplayıp karşılaştırır):

```text
sanitize(s):  [A-Za-z0-9] korunur, diger karakterler '_' olur,
              ardisik '_' tekillestirilir, bas/son '_' kirpilir. Case korunur.

orgu_slug(c_orgu):
  string ise                  -> sanitize(s)            # "self", "*:[type:branch]" -> "type_branch"
  {from: "$ctx...", traverse} -> sanitize(from) + "_" + sanitize(traverse)
  {from: {wfah}, traverse}    -> "wfah_" + sanitize(wfah) + "_" + sanitize(traverse)

slug(c_a):
  parts = [ orgu_slug(c_orgu) ]
  c_r varsa: parts += [ sirali(sanitize(rol)).join("-") ]
  c_u varsa: parts += [ "u_" + sirali(sanitize(user)).join("-") ]
  slug = parts.join("__")
```

`u_` öneki rol/user ad çakışmasını ayırır. Sanitize sonrası iki FARKLI canonical c_a aynı slug'a düşerse (collision) editör ikinciye `_<fnv1a16(canonical)>` hex son eki ekler; validator collision'ı hata sayar, hash'li key'i kabul eder.

### 2b. Kurallar

- Her node key == slug(c_a) (veya collision hash'li hali). Uymayan key = HATA.
- Aynı canonical c_a (c_r/c_u sıraları normalize edilmiş) ikinci bir node'da bulunamaz = HATA.
- `label` serbesttir, kimlik değildir; validator dokunmaz.
- Editör, c_a düzenlendiğinde slug'ı yeniden üretir ve tüm `from` / `wft.node` / `escalation.wft.node` referanslarını otomatik yeniden bağlar.

## 3. C_A Matcher (Authorization) — Kanonik Semantik

```text
match(rule, actor, wfe) :=
  actor.orgu ∈ resolve(rule.c_orgu, wfe)
  AND ( (rule.c_r var ve actor.role ∈ rule.c_r)
        OR (rule.c_u var ve actor.user ∈ rule.c_u) )
```

- Verilmeyen alan false'dur (wildcard değil). Şema c_r/c_u'dan en az birini zorunlu kılar.
- c_u match'i rol-agnostiktir; ACT yine exact `(ORGU,(U,R))` tuple ile kaydedilir.
- Bu matcher node `c_a` (start node dahil — bkz. §"Symmetric start"), transition ek-kısıt `c_a` ve `listable[].c_a` için AYNIDIR.

## 4. Visibility Matcher — AYRI Fonksiyon, OR Semantiği

```text
visible(vis, actor, wfe) :=
     (vis.c_orgu var ve actor.orgu ∈ resolve(vis.c_orgu, wfe))
  OR (vis.c_r    var ve actor.role ∈ vis.c_r)          # scope'suz
  OR (vis.c_u    var ve actor.user ∈ vis.c_u)          # scope'suz
  OR (vis.c_a    var ve match(vis.c_a, actor, wfe))    # scope'lu tam kural
```

Authorization matcher'ı ile BİRLEŞTİRİLMEZ; iki ayrı fonksiyon olarak implemente edilir. V yalnızca field okunurluğunu filtreler; ACT/claim/listability üretmez. `x-visibility` yoksa field görünürdür; varsa match etmeyen actor'a field response'ta gizlenir. V, WFE'yi görebilen herkese uygulanır (owner, unassigned C_A, L observer).

## 5. Graf Validation

v2.1 ile aynı: start'tan BFS reachability (escalation kenarları DAHİL), erişilemeyen node/terminal = `WFD.Unreachable`; çıkışsız node (transition + escalation yok) = hata; aynı `(from, action)` için `when`'siz çoklu transition = hata, `when`'li = uyarı (runtime ilk-match).

## 6. Context / Expression / Retry Validation

v2.1 ile aynı: input path'leri, readonly yasağı, `wfes_effects.set` path+tip (catch ve escalation effects dahil), `$exec.response.*` = hata, ZEN parse + boolean sonuç, `WFD.ALL` tek başına ve son retrier'da, `catch.error_equals` default `["WFD.ALL"]`.

### 6b. Context yazma sözleşmesi (WOR-70)

`context.required` KALDIRILDI. Zorunluluk artık **tek yerde** bildirilir (`actions.<ad>.input.required`)
ve doğruluğu üç **tasarım-zamanı** kuralıyla korunur — çalışma anında ayrı bir ctx doluluk
denetimi YOKTUR:

| Kod | Kural |
|---|---|
| `context_required_removed` | `context.required` ya da `context.properties.*` altında `required` varsa WFD REDDEDİLİR (kökte de, iç içe de). Şema düzeyinde de yasak: `contextSchemaNode.not.required`. |
| `context_field_never_written` | Her context **yaprağı** en az bir `wfes_effects.set` hedefi tarafından kapsanmalı. Kapsama iki yönlüdür: `applicant` yazımı `applicant.name`'i kapsar, `initiated_by.role` yazımı opak `initiated_by` yaprağını kapsar. Hiç yazılmayan alan = hiç dolmayacak alan → hata. |
| `unused_action_input` | Bir kuralın (`start[]` / `transitions[]`) aksiyonunun bildirdiği her input yolu (`required ∪ optional` — opsiyonel olması muaf tutmaz), o kuralın effects'inde `$action.input.<yol>` ile tüketilmeli. Tüketici olarak kuralın kendi `wfes_effects`'i, `trigger[].catch.wfes_effects`'i ve tetiklediği `autoexec.<ad>.wfes_effects` sayılır. |
| `optional_input_nulls_other_writer` *(UYARI)* | Bir alanı hem opsiyonel girdi hem başka bir yazar yazıyorsa: girdi gönderilmezse diğerinin değeri `null`'a döner. Yayını engellemez (§7.5a). |

Taranan effect siteleri (yazar kümesi): `start[].wfes_effects`, `start[].trigger[].catch`,
`transitions[].wfes_effects`, `transitions[].trigger[].catch`, `nodes[].escalation[]`,
`nodes[].claim_timeout`, `terminals[].wfes_effects`, `autoexec[].wfes_effects`.

## 6c. Attachment (ek-belge) Validation

Root `attachments` katalogu + `nodes.<key>.attachments` referansları için custom validator (`check_attachments`):
- Grup içi `item.id` tekil olmalı — aksi `attachment_item_dup`.
- Node'un referansladığı her grup key'i katalogda VAR olmalı — aksi `attachment_ref`.
- Bir node aynı grubu birden fazla referanslayamaz — aksi `attachment_ref_dup`.

Alan opsiyoneldir; katalog boş/yoksa hiçbir kural tetiklenmez. Dosyaların KENDİSİ engine'de değildir — validator yalnız katalog+referans tutarlılığını dener. Runtime gate ve varlık kontrolü portal edge'indedir (bkz. DECISIONS Madde 8): `nodes.<key>.attachments` referanslı grupların `required` item'ları yüklenmeden o node'dan aksiyon submit edilemez (`422 attachment.missing`). Start node'unda attachment (henüz wfe_id yok) gate EDİLMEZ.

## 7. Transition Runtime Pipeline

```text
1. WFE assigned mi? Actor owner mi? Degilse ACT reddedilir.
   (Unassigned'da once claim; claim = current node c_a match'i, §3 semantigi.)
2. transition.c_a varsa: owner bu EK kurala da match etmeli (§3).
3. current_node ∈ transition.from? Degilse aday degildir.
4. Adaylar array sirasiyla; when'i true olan ILK transition secilir.
5. Action input validate edilir (SADECE dogrulama — ctx'e YAZILMAZ, §7.5a).
6. transition.wfes_effects STAGED.
7. trigger[] sirayla: when -> execute (timeout_seconds) -> fail'de retry
   (bekleme = interval * backoff^attempt, max_delay ile kirpilir)
   -> catch match: effects STAGED, handled, devam -> yoksa required davranisi.
   Basarili autoexec: wfes_effects STAGED.
8. transition.wft staged DynCtx uzerinden evaluate edilir.
9. COMMIT (atomik): diff'ler + WFAH + node degisimi + assignment reset (yeni node'a UNASSIGNED).
Unhandled fail'de hicbir sey commit edilmez.
```

### 7.5a. Context'e TEK yazma yolu: wfes_effects (WOR-70)

Aksiyon girdisi ctx'e **kendiliğinden yazılmaz**. Adım 5 yalnız sözleşmeyi doğrular
(`input.required` mevcut mu, bildirilmemiş leaf var mı); ctx'e yazan tek mekanizma
`wfes_effects.set`'tir. Girdiyi ctx'e taşımak için akış açıkça yazar:

```json
"wfes_effects": {
  "set": {
    "applicant": "$action.input.applicant",
    "credit_info.amount_requested": "$action.input.credit_info.amount_requested"
  }
}
```

Gerekçe: bir ctx alanının değeri nereden geldiği akışa bakılarak cevaplanabilsin
("iki yazma yolu" belirsizliği kalksın). Sözleşmenin bütünlüğü §6b'nin üç kuralıyla
korunur: yazılmayan alan da, tüketilmeyen input da WFD'yi reddettirir.

**`required` ↔ `optional`: tek fark değerdir (WOR-70b).** İkisi de `wfes_effects` ile
ctx'e eşlenmek ZORUNDADIR — `optional` olması bu zorunluluğu kaldırmaz (validator
`unused_action_input` her ikisini de denetler). Fark yalnız yazılan değerde:

| | İstek | Ctx'te sonuç |
|---|---|---|
| `required` | Gönderilmek zorunda, değeri `null` OLAMAZ (`zorunlu input 'x' null olamaz`) | Her zaman gerçek bir değer |
| `optional` | Gönderilmeyebilir | Gönderildiyse değeri, gönderilmediyse **`null`** |

Yani `ek_bilgi` context'te tanımlı, bir aksiyonun `optional` girdisi ve o aksiyonun
effects'inde `"ek_bilgi": "$action.input.ek_bilgi"` yazılıyorsa bu GEÇERLİ bir
kullanımdır; kullanıcı doldurmazsa alan `null` kalır. Alan "ölü" değildir — yazarı
vardır, değeri boştur.

Null denetimi YALNIZ bildirilen yolun kendisine bakar: `required: ["applicant"]` ile
`{"applicant": {"name": null}}` geçerlidir. `name`'in de dolu olması isteniyorsa
`applicant.name` ayrıca `input.required`'a yazılır.

**Yan etki ve uyarısı:** her `set` satırı koşulsuz uygulandığı için, bir alanı hem
opsiyonel girdi hem BAŞKA bir yazar (escalation / autoexec / terminal / başka bir kural)
yazıyorsa, girdi gönderilmediğinde diğerinin yazdığı değer `null`'a döner. Validator
bunu `optional_input_nulls_other_writer` UYARISI ile bildirir — yayını engellemez
(bilinçli tasarım olabilir), ama akış yazarı tuzağı tasarım anında görür. Golden
fixture bu durumun canlı örneğini taşır (`internal_notes`: iki aksiyonun opsiyonel
girdisi + analist havuzunun escalation'ı).

## 8. Escalation / Timeout Runtime

v2.1 ile aynı: escalation zamanlayıcısı node-giriş anından başlar (WFAH'tan türetilir), sıralı adımlar birer kez tetiklenir, assigned WFE'de de çalışır, taşımada assignment temizlenir, WFAH'a system actor yazılır, adım tek transaction'dır. `autoexec.timeout_seconds` aşımı `WFD.Timeout`; root `timeout` aşımı engine-defined fail + WFAH kaydı.

### 8a. SLA-1/SLA-2 akışı BİTİREMEZ (2026-07-28)

Zaman aşımıyla akışı sonlandırma yetkisi **yalnız SLA-3'e** (root `timeout`) aittir —
o, tüm işin mutlak son teslim süresidir. SLA-1 ve SLA-2 birer **sorumluluk devridir**;
bir adımın süresi dolmuş olması işin bittiği anlamına gelmez. İki kural:

1. **`escalation[].terminate` kaldırıldı** (`escalation_terminate_removed`). `wft`
   artık ZORUNLUDUR (`escalation_wft_required`). `SLA.Dwell` end_response'u üretilmez;
   `terminated` durumunu üreten tek zamanlayıcı yolu `timeout:deadline`
   (`SLA.Deadline`) kalır.
2. **SLA hedefi YALNIZ bir node olabilir.** SLA-1'in `claim_timeout.wft`'i zaten bare
   bir node key'idir; SLA-2'nin `wft`'i de yalnız `{"node": …}` formunu kabul eder
   (şemada `$ref: wftNode`). Diğer formların hepsi hata:

   | Form | Hata kodu | Neden |
   |---|---|---|
   | `{"terminal": …}` | `sla_terminal_target` | akışı bitirir → SLA-3'ün işi |
   | `{"conditions": […]}` | `sla_target_not_node` | dallanma bir karardır |
   | `{"parallel": …}` | `sla_target_not_node` | fork açmak bir karardır |
   | `{"collapse": …}` | `sla_target_not_node` | kardeş kolları düşürmek bir karardır |

   Böylece **dolaylı** yollar da kapanır: SLA bir switch'i hedefleyip onun bir kolundan
   terminal'e inemez, çünkü switch hedefi zaten yasaktır. Autoexec hiçbir zaman bir `wft`
   hedefi değildi (wire formatında öyle bir varyant yok).

Sonuç: bir SLA-2 adımının tek olası runtime sonucu `MoveTo` (paralel modda
`BranchMoveTo` / join hedefiyse `BranchArrived`) — `Terminated`, `CollapseTo` ya da
`ForkTo` üretmesi imkânsızdır. Kardeş kolları düşürmek isteyen akışlar bunu bir
AKSİYONUN `wft: {"collapse": …}` hedefiyle yapar.

### 8b. SLA effects (opsiyonel DynCtx yazımı)

SLA-1 ve SLA-2 adımları opsiyonel `wfes_effects` taşır (SLA-2'de zaten vardı; SLA-1'e
2026-07-28'de eklendi). Süre dolduğunda effect'ler **system aktörü** adına staged
DynCtx'e uygulanır ve aynı transaction'da persist edilir — SLA-1'in `wft`'siz
(havuza-dönüş) yolunda node/status değişmediği hâlde ctx satırı yazılır.

Kullanılabilir namespace: `$ctx.*`, `$actor` (= `{role: "system"}`), `$node` (SLA'nın
tetiklendiği node), `$timestamp`, `$wfe_id`. **`$action.input.*` ve `$exec.result.*`
YASAKTIR** — SLA'yı bir aksiyon ya da autoexec tetiklemez, bu yollar sessizce `null`
yazardı; validator reddeder (`sla_effect_namespace`). `wfes_effects` verilmezse hiçbir
şey yazılmaz (rastgele bir aksiyonun effect'leri devralınmaz).

## 9. WFD Yükleme

- Tanınmayan `wfd_version` = yükleme reddi. Root'ta bilinmeyen alan yasak.
- Çalışan WFE'ler başladıkları WFD (id+version)'a sabitlenir; kural değişikliği yeni WFD versiyonu doğurur — node slug'ları bu sayede WFE ömrü boyunca kararlıdır. Versiyon-aşırı metrikler `label` üzerinden agregat edilmelidir.

## 5b. Parallel Fork/Join Validation (WOR-31)

`wft`'in 4. formu: `{"parallel": {"branches": [...], "join": {node|terminal}}}` (bkz.
decisions.md WOR-31 — eski `WftRule::Parallel`'in WOR-25'te kaldırılışını
yeniden tasarlayarak supersede eder; `join_when` YOK, join deklaratif bir hedef).
`wfe-core/src/validator.rs::check_parallel`:

- Parallel wft `start[].wft`'te YASAK.
- `branches` ≥2 ve distinct; `join` kollardan biriyle aynı olamaz.
- Branch subgraph'ları (fork'tan join/terminal'e kadar transition `wft` kenarları
  izlenerek BFS) pairwise AYRIK olmalı; içlerinde nested Parallel YASAK; her biri
  join node'a veya bir terminal'e ulaşabilmeli.
- `check_graph` (§5) Parallel'i kenar kaynağı sayar: fork node → her branch +
  fork → join hedefi (aksi halde branch/join node'ları hatalı biçimde
  `WFD.Unreachable` görünür).

Runtime yürütme semantiği (branch token, AND-join, iptal/SLA davranışı) T2 işinde
kodlanacak; bu commit yalnızca model + validator + spec'i getirir.

## Symmetric start (v2.2)

`start[]` artık `transitions[]` ile simetriktir: `{ id, from, action, wfes_effects?, trigger?, wft }`. `action` start aksiyonunun gerçek adıdır (ör. `"Akışı Hazırla"`) — rezerve sabit değildir, `actions{}` içinde normal bir ACT olarak tanımlanır. `c_a` startRule'dan kaldırılmıştır; `start[].from` ile referans edilen `nodes` girdisinin `c_a`'sı taşır. Start-node kimliği türetilmiştir — node'un kendisinde `kind` alanı YOKTUR; bir node, sadece bir `start[].from` tarafından referans edilerek start node olur.

| # | Kural |
|---|------|
| V1 | `start[].from`, `nodes` içinde var olan bir node'a referans vermelidir. |
| V4 | `start[].action`, `actions{}` içinde tanımlı bir action key'e karşılık gelmelidir (transition'lardaki `action` ile aynı kural). |
| V5 | En az bir `start` girdisi olmalıdır (mevcut `minItems: 1`). |

**V2/V3 kaldırıldı (2026-07-16):** Start node artık yeniden girilebilir. Bir `start[].from` node'u başka bir transition/start/escalation'ın `wft` hedefi OLABİLİR ve kendi `escalation`'ını taşıyabilir — bir akışı hem müdür hem memur başlatabildiğinde (iki start node), biri diğerini `wft` ile hedefleyebilir (onay/kontrol simetrisi). Start kuralları yalnızca WFE **yaratılırken** devreye girer; node'a mid-flow bir `wft` ile girildiğinde WFE orada normal bir node gibi durur — escalation/claim_timeout zaten node-giriş WFAH zaman damgasından ölçülür, start anında WFE o node'da beklemediği için ayrım runtime'da halihazırda vardı; değişiklik sadece validasyon katmanındaydı.

**Runtime resolution:** Actor, `start[].action` ile adlandırılmış aksiyonu çağırır → her aday start node'un `c_a`'sına karşı eşleştirilir → eşleşen node efektif `from` olur → o start rule'ın `wfes_effects`/`trigger`'ı çalışır → WFE `wft`'e iner. Transition seçimiyle (`from` + `action`) birebir aynı mekanik.

Lifecycle notu: transition'larda `node.c_a` WFE'yi o an elinde tutan owner'dır; bir start node'da `c_a` kimin *başlatabileceğidir*. Aynı eşleştirme mekaniği, farklı lifecycle anlamı (henüz WFE yok).

**Start input doğrulaması (2026-07-14, WOR-70 ile güncellendi):** Start input'u, transition input'larıyla (§7.5) birebir aynı kurala tabidir — seçilen start rule'ın `action`'ına ait `input.required` yolları mevcut olmalı, `required ∪ optional` dışında kalan her leaf yol `WFD.InvalidInput` ile REDDEDİLİR (hard reject; sessiz düşürme yok). `x-wf-readonly` işaretli bir yol, bildirilmiş olsa bile start input'unda verilemez. **Başlangıç ctx'i input'tan TOHUMLANMAZ** (WOR-70): input yalnız doğrulanır, ctx'e yalnız `wfes_effects` yazar — bkz. §7.5a. `context.required` KALDIRILDI; start sonrası ctx doluluk denetimi yoktur.
