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
ve doğruluğu dört **tasarım-zamanı** kuralıyla korunur — çalışma anında ayrı bir ctx doluluk
denetimi YOKTUR:

| Kod | Kural |
|---|---|
| `context_required_removed` | `context.required` ya da `context.properties.*` altında `required` varsa WFD REDDEDİLİR (kökte de, iç içe de). Şema düzeyinde de yasak: `contextSchemaNode.not.required`. |
| `context_field_never_written` | Her context **yaprağı** en az bir `wfes_effects.set` hedefi tarafından kapsanmalı. Kapsama iki yönlüdür: `applicant` yazımı `applicant.name`'i kapsar, `initiated_by.role` yazımı opak `initiated_by` yaprağını kapsar. Hiç yazılmayan alan = hiç dolmayacak alan → hata. |
| `unused_action_input` | Bir kuralın (`start[]` / `transitions[]`) aksiyonunun bildirdiği her input yolu (`required ∪ optional` — opsiyonel olması muaf tutmaz), o kuralın effects'inde `$action.input.<yol>` ile tüketilmeli. Tüketici olarak kuralın kendi `wfes_effects`'i, `trigger[].catch.wfes_effects`'i ve tetiklediği `autoexec.<ad>.wfes_effects` sayılır. |
| `effect_type_mismatch` | `wfes_effects.set` hedefinin şema tipi ile yazılan değerin tipi uyuşmalı. Kaynak tipi BİLİNEN değerler: `$actor` → **object** (`{orgu_id, user_id, role}`), `$timestamp`/`$wfe_id`/`$node`/`$call.status`/`$call.wfe_id` → string, `$ctx.<yol>` → o yolun şeması, sabitler → JSON tipi. `$action.input.*` / `$exec.result.*` / `$call.result.*` TİPSİZDİR (şema WFD'de durmaz) → kural sessiz kalır. Motor yazmayı reddetmediği için hata yayında görünmez: `$actor`'ü string alana yazan akışta o alanı okuyan koşullar sessizce hep-false olur. |
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
   | `{"collapse": {"terminal"…}}` | `sla_terminal_target` | akışı bitirir → SLA-3'ün işi |

   **İSTİSNA (2026-08-03):** node hedefli `{"collapse": {"node": …}}` SLA-2'de GEÇERLİDİR
   — bkz. 8a-1. Kardeş kolları düşürmek bir dallanma kararı değildir: hedef tektir.

   Böylece **dolaylı** yollar da kapanır: SLA bir switch'i hedefleyip onun bir kolundan
   terminal'e inemez, çünkü switch hedefi zaten yasaktır. Autoexec hiçbir zaman bir `wft`
   hedefi değildi (wire formatında öyle bir varyant yok).

Sonuç: bir SLA-2 adımının tek olası runtime sonucu `MoveTo` (paralel modda
`BranchMoveTo` / join hedefiyse `BranchArrived`) — `Terminated`, `CollapseTo` ya da
`ForkTo` üretmesi imkânsızdır.

### 8a-1. SLA-1 ve SLA-2 paraleli SONLANDIRABİLİR — tasarımcının tercihiyle (2026-08-03)

Her iki SLA da "süre dolunca paraleli kapat" yetkisini tasarımcının açık tercihiyle
alır. Kol bağlamında tetiklendiğinde hedef `{collapse:{node}}` olarak çözülür → kardeş
kollar `cancelled`, paralel mod kapanır, WFE hedefe gider (`CommitOutcome::CollapseTo`,
`_collapse` özeti + `_branch_cancelled` detayları — aksiyon collapse'ıyla BİREBİR aynı
yol, tetikleyicisi system aktörü). Audit'te SLA marker'ının input'una `collapse: true`
yazılır (bayrak yokken anahtar hiç görünmez).

Wire biçimi ikisinde FARKLIDIR, sebebi hedef alanının tipidir:

| | Wire | Neden |
|---|---|---|
| **SLA-1** | `claim_timeout.collapses_parallel: true` (ayrı bayrak) | `claim_timeout.wft` çıplak bir node key STRING'idir; `{collapse:{…}}` objesi oraya sığmaz. Alanı `Wft` union'ına çevirmek wire'ı kırardı. |
| **SLA-2** | `escalation[].wft = {collapse:{node}}` | `escalation[].wft` zaten bir `Wft` union'ı; yeni alan gerekmez, form yeterli. |

Sözleşme (yukarıdaki 8a'yı DARALTMAZ):

| Kural | Kod |
|---|---|
| **Node bir paralel KOLUN İÇİNDE olmalı** — fork ile join arasında | `claim_timeout_collapse_outside_parallel` / `escalation_collapse_outside_parallel` |
| SLA-1: `wft` ZORUNLU — hedefsiz collapse gidilecek yer bırakmaz | `claim_timeout_collapse_requires_wft` |
| SLA-2: `wft` zaten zorunluydu | `escalation_wft_required` (değişmedi) |
| Hedef hâlâ yalnız NODE — collapse paraleli bitirir, AKIŞI bitirmez | `sla_terminal_target` (değişmedi) |

**Kol içinde olmak** = `parallel_interior_nodes`: fork'un `branches` giriş node'larından
başlayıp transition kenarlarıyla BFS, join node'unda dur. Yani kol GİRİŞİ olmak şart
değil, kolun İÇİNDE kalmak şart — kolun 2., 3. adımı da collapse edebilir. Yürüyüş
`check_parallel`'in branch subgraph BFS'iyle aynıdır (collapse ve iç içe parallel
kenarları izlenmez; SLA kenarları da izlenmez — SLA hedefi kolun parçası olmaz).

Paralel akışa BAĞLI OLMAYAN bir node collapse edemez: süresi dolduğunda düşürülecek
kardeş kol yoktur, ayar sessizce hiçbir şey yapmaz. Bu yüzden uyarı değil HATA —
doküman yayınlanamaz. (Dokümanda hiç fork yoksa interior kümesi boştur, aynı hataya
düşer.)

**Runtime savunması.** Authoring kuralı yukarıda kapatıldığı hâlde, kol içi bir node
grafın BAŞKA bir yerinden de erişilebilir (kol dışı bir transition oraya gidebilir) — o
çağrıda WFE paralel modda olmaz. Böyle bir tetiklemede bayrak/form yok sayılır ve düz
`{node}` devri uygulanır; hata vermek WFE'yi zaman aşımında kilitlerdi. Bu bir savunma
yolu, normal yol değil.

Bayrak bir DALLANMA kararı değildir — hedef tektir ve tasarım anında sabittir; SLA yine
"kim karar verecek" sorusunu değiştirir, "hangi yol" sorusunu değiştirmez.

Aksiyon tarafındaki collapse (`transitions[].wft = {collapse:{…}}`) DEĞİŞMEDİ; tek fark
onun terminal hedefi de alabilmesidir — bir insan kararı akışı bitirebilir, bir
zamanlayıcı bitiremez.

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

## 5b. Parallel Fork/Join Validation (WOR-31, WOR-72, WOR-73)

`wft`'in 4. formu: `{"parallel": {"branches": [...], "join": {node|terminal}}}` (bkz.
decisions.md WOR-31 — eski `WftRule::Parallel`'in WOR-25'te kaldırılışını
yeniden tasarlayarak supersede eder; `join_when` YOK, join deklaratif bir hedef).
`wfe-core/src/validator.rs::check_parallel`:

- Parallel wft `start[].wft`'te YASAK.
- `branches` ≥2 ve distinct; `join` kollardan biriyle aynı olamaz.
- **WOR-72 `join_mode`:** `and` (varsayılan, WOR-31) tüm kolları bekler; `or`
  K-of-N quorum'dur. `join_threshold` YALNIZ `or` ile verilebilir
  (`parallel_join_threshold`), 1 ≤ K < kol sayısı olmalıdır ve verilmezse 1'dir
  (ilk varan kazanır). K = kol sayısı AND'in ikinci yazımı olurdu → reddedilir.
  Kol subgraph kuralları OR'da da AYNEN geçerlidir (quorum dolmadan iptal
  edilmeyecek kolların da bir çıkışı olmalı).
- **WOR-73 `join_mode: expr` + `join_when`:** join koşulu ZEN ifadesidir —
  "(finans VE hukuk) YA DA GM" gibi SAYIYLA ifade edilemeyen kurallar için.
  `join_when` yalnız `expr` ile verilebilir ve o modda ZORUNLUDUR
  (`parallel_join_when`); ifade parse edilebilmeli ve `$branches.<x>` referansları
  BU fork'un kolları olmalıdır (`parallel_join_when_unknown_branch` — yazım hatası
  runtime'da sessizce `false` dönen bir alan olur ve join hiç dolmaz).
  `join_threshold` `expr` ile birlikte verilemez (sayıyı ifade kendisi anlatır:
  `len($arrived) >= k`).
- Branch subgraph'ları (fork'tan join/terminal'e kadar transition `wft` kenarları
  izlenerek BFS) pairwise AYRIK olmalı; içlerinde nested Parallel YASAK; her biri
  join node'a veya bir terminal'e ulaşabilmeli.
- `check_graph` (§5) Parallel'i kenar kaynağı sayar: fork node → her branch +
  fork → join hedefi (aksi halde branch/join node'ları hatalı biçimde
  `WFD.Unreachable` görünür).

Runtime yürütme semantiği (branch token, AND-join, iptal/SLA davranışı) T2 işinde
kodlanmıştır.

**OR-join runtime (WOR-72).** Fork anında mod + eşik TEK sayıya indirgenir
(`ParallelSpec::quorum`) ve WFE satırına yazılır (`wf.wfe.join_threshold`;
NULL = AND). Bir kol join hedefine vardığında tamamlanma ölçütü:

| mod | tamamlandı | tamamlanmadı |
|---|---|---|
| AND (`NULL`) | başka aktif kol kalmadı → `JoinComplete` | `BranchArrived` |
| quorum (`k`) | varış sayısı ≥ k → `JoinComplete` | `BranchArrived` |
| expr (WOR-73) | `join_when` ZEN koşulu `true` → `JoinComplete` | `BranchArrived` |

**Kol kimliği (WOR-73).** `wfe_branch.branch_node` kol içinde aksiyon alındıkça
DEĞİŞİR (`BranchMoveTo`), dolayısıyla "hangi kol" sorusunun cevabı o değildir.
Kimlik `wfe_branch.entry_node`'dur — fork'taki giriş node'u, bir daha değişmez.
Join koşulu namespace'i bu kimlikle çalışır:

| ifade | anlamı |
|---|---|
| `$branches.<entry_node>` | o kol join'e vardı mı (DEĞERLENDİRİLEN varış dahil) — hiç varmamış kol `false` |
| `$arrived` | varmış kol kimliklerinin dizisi → `len($arrived) >= 2`, `'x' in $arrived` |

`$ctx`/`$wfah`/`$prev`/`$first`/`$actor`/`$action.input.*` da açıktır ("tutar 1M
üstündeyse GM de onaylasın"). Join bağlamı DIŞINDA `$branches` boş obje, `$arrived` boş
dizidir — ifade patlamaz (`$call` ile aynı gerekçe).

**Tatmin edilemeyen join (WOR-73).** `and` ve `quorum` bitişi garanti eder; ZEN
koşulu etmez. SON aktif kol da varıp ifade hâlâ `false` ise WFE paralel modda
sessizce kilitlenirdi — bunun yerine engine-defined fail üretilir:
`CommitOutcome::Failed`, `end_response = {reason: "WFD.JoinUnsatisfied", join_rule,
arrived}`. Validator tatmin edilebilirliği KANITLAYAMAZ (genel olarak karar
verilemez); yalnız bilinmeyen kol referansını yakalar.

Quorum eşiği dolarken geride aktif kol kalırsa (`quorum_collapse`):
kalan kollar `cancelled` olur, `_collapse` özeti + kol başına `_branch_cancelled`
marker'ları yazılır (`kind`/`reason` = `join_quorum`, `target` = join node'u). Eşiğin
ÜYESİ olan varmış kollar `superseded` İŞARETLENMEZ — onayları sayılmıştır. Kol
satırları AND yolunda silinir, quorum yolunda `cancelled` olarak KALIR (hangi kolun
neden düştüğü portalda görünsün). `_fork` marker'ı `join_threshold` taşır.
Yarış (WOR-73 ile genelleşti): engine kararını hangi VARIŞ KÜMESİ üzerinde verdiyse
onu outcome'a koyar (`arrived_entries`, acting kol dahil); adapter `FOR UPDATE`
altında DB'deki kümeyle karşılaştırır (`JoinState::arrival_matches`), uyuşmazsa
`Conflict(BranchArrival)` → executor reload + yeniden koşar. Sayı karşılaştırması
YETMEZ: ZEN koşulu sayıyla ifade edilemez, ama küme aynıysa saf engine'in kararı da
aynıdır — adapter ZEN ÇALIŞTIRMAZ (I/O katmanı motorun mantığını ikinci kez yazmaz).

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

**Start input doğrulaması (2026-07-14, WOR-70 ile güncellendi):** Start input'u, transition input'larıyla (§7.5) birebir aynı kurala tabidir — seçilen start rule'ın `action`'ına ait `input.required` yolları mevcut olmalı, `required ∪ optional` dışında kalan her leaf yol `WFD.InvalidInput` ile REDDEDİLİR (hard reject; sessiz düşürme yok). **Başlangıç ctx'i input'tan TOHUMLANMAZ** (WOR-70): input yalnız doğrulanır, ctx'e yalnız `wfes_effects` yazar — bkz. §7.5a. `context.required` KALDIRILDI; start sonrası ctx doluluk denetimi yoktur.

## 10. WFC — İş Akışı Çağrısı Runtime (2026-07-30)

Bir WFE'nin başka bir WFD'yi çalıştırması. Kanonik terimler: `terminology.md` → WFC;
tasarım gerekçeleri: `decisions.md` → WFC. Referans implementasyon:
`wfe-core/src/v22/pipeline.rs::stage_calls` / `fire_call_return`,
`wfe/src/executor.rs` (tarama döngüleri), `wfe/src/repo/call.rs` (`wf.wfe_call`).

**Katalog ↔ referans.** Root `calls` NE çağrılacağını + hangi girdiyle söyler
(`wfd_id`, `version?`, `start?`, `input`); referans NASIL çağrıldığını (`mode`).
`autoexec` ↔ `trigger` ayrımının aynısıdır — aynı katalog kaydı üç modda da kullanılabilir.

**Mod ↔ yerleşim.** Zorunlu eşleme (validator `call_mode_placement`):

```text
mode: wait      -> yalniz nodes.<k>.call    cagiran o node'da BEKLER, sonuc $call.* ile doner
mode: detached  -> yalniz nodes.<k>.call    cagrilan baslar, cagiran HEMEN devam eder
mode: terminal  -> yalniz terminals[].call  cagiran BITER, ardil akis baslar (donus yok)
```

### 10a. Outbox — çağrı niyeti commit ile atomiktir

Çağrılan WFE, çağıranın commit transaction'ı **içinde yaratılmaz**. Bunun yerine niyet
aynı tx'te `wf.wfe_call`'a `queued` olarak yazılır; gerçek start ayrı bir tx'te koşar.

Gerekçe: çağrılanı içeride yaratmak, çağıranın atomik transaction'ını başka bir WFE'nin
tüm start pipeline'ına (org resolve, trigger'lar, kendi commit'i) bağlardı. Outbox ile
çağıranın atomikliği korunur ve başlatma yeniden denenebilir olur.

```text
1. resolve_wft "nereye varildi"yi da doner (Option<CallSite>) — CommitOutcome::Terminal
   terminal id'sini TASIMAZ, ardil cagriyi bulmak icin o id gerekir.
2. stage_calls: varilan sitedeki `call` blogu okunur, WFC-IN cozulur, `wait` icin
   `timeout` MUTLAK zamana cevrilir (her tick'te ISO parse etmemek icin — SLA-3 ile ayni).
3. COMMIT: cagiranin durumu + WFAH + outbox satiri TEK tx.
4. Sweeper (SLA tick'lerinden ONCE): queued'lari baslat -> suresi gecenleri kapat ->
   donusleri isle.
```

**Çift start koruması:** `UNIQUE (caller_wfe_id, site_kind, site_key)` +
`ON CONFLICT DO NOTHING`. Executor'ın conflict retry döngüsü (WOR-62) aynı transition'ı
ikinci kez koşarsa çağrı İKİ KEZ başlatılmaz.

**Gecikme:** çağrılan terminal'e ulaştığında `mark_callee_finished` satırı `returned`'e
çeker ve `nudge_timers` sweeper'ı hemen uyandırır — dönüş pratikte anlıktır, 60 sn'lik
güvenlik ağı beklenmez. Tam otomatik çağrılan zaten `Engine::start` içinde biter ve
aynı istekte işaretlenir. **Bu yüzden bloklayan bir `sync` moduna gerek yoktur** (bkz.
`decisions.md` → WFC).

### 10b. WFC node'u bir bekleme HAVUZU değildir

Bekleme bir **durum**tur, transition adımı değil: `WFES = current_node + assignment +
DynCtx + WFAH` değişmezi korunur, beklemenin kalıcı yeri `current_node`'dur.

- `c_a` HÂLÂ ZORUNLUDUR — "node key = slug(c_a)" ve "aynı canonical c_a ikinci node'da
  olamaz" değişmezlerine dokunulmadı. Anlamı daralır: *alt akış sürerken bu WFE'yi kim
  görür ve kim iptal edebilir*. ACT/claim VERMEZ.
- Node'da `kind` alanı YOKTUR; node'u WFC node'u yapan şey `call` bloğunun varlığıdır
  (start node'un "referans ile türetilmiş kimlik" deseninin aynısı).
- Yasak: `transitions[].from` içinde yer almak, `escalation`, `claim_timeout`,
  `attachments`, `reassign`, `start[].from` olmak. Çıkışı `call.wft`'dir (zorunlu).
- Graf kuralları: `call.wft` normal bir wft kenarıdır (cross_ref + BFS reachability
  onu izler) ve WFC node'u `no_exit` kuralından muaftır — transition aramak yanlış olur.

### 10c. WFC-RETURN — insan ACT'i olmayan kenar

`fire_escalation` / `fire_claim_timeout` ile AYNI sınıftır: system aktörü tetikler,
WFAH'a `call:<key>` marker'ı düşer, tek transaction'dır. Tek farkı bağlamda `$call.*`
namespace'inin bağlı olmasıdır.

```text
$call.result.*   cagrilanin wfe_end_response'u  (detached'da daima null)
$call.status     completed | failed | terminated | timeout | started
$call.wfe_id     cagrilanin id'si
```

- `$exec.result.*` ile **birleştirilmez**: autoexec bir sistem çağrısıdır, WFC bir WFE
  örneğidir. WFC-RETURN dışındaki bağlamlarda `$call` boş bir kabuktur (null döner,
  ifade patlamaz — eksik ctx alanının null olması gibi).
- Bağlamda `$action.input.*` **YOKTUR** (validator `call_effect_namespace`) — SLA
  effects'teki `sla_effect_namespace` kuralının ikizi.
- `call.wft` hedefi node **veya** terminal olabilir. SLA'nın "terminal hedef yasak"
  kısıtı burada GEÇERLİ DEĞİLDİR: bu bir zamanlayıcının değil, çağrılanın sonucuna
  dayanan bir karardır.
- **Hata da bir dönüştür:** `failed` / `terminated` / `timeout` akışı çökertmez; WFC-RETURN
  normal işler ve akış `$call.status`'a bakarak karar verir. Sonuç yoksa effects hedefleri
  `null` yazar (sessizce eski değer KALMAZ).
- Çağıran o node'da artık beklemiyorsa (SLA devri, iptal, elle müdahale) dönüş
  UYGULANMAZ: satır `consumed` yapılıp geçilir — sessiz yanlış transition üretilmez.

### 10d. Ardıl akış (`mode: terminal`) — üç sert kural

*"Bir iş akışının bitişi başka bir iş akışının başlangıcı."* Alt akış DEĞİLDİR:
yuvalanma yok, dönüş yok, sahiplik yok. Sıradaki akış.

**Sıralama kesindir:** terminal `wfes_effects` → `wfe_end_response` üretimi → çağıran
`completed` olarak **commit** → *ondan sonra* ardıl start. Yani WFC-IN, terminal effects
uygulandıktan SONRAKİ ctx'e göre çözülür; ardıla taşınacak veri önce terminal effects'i
ile ctx'e yazılır (WOR-70 ile tam tutarlı).

1. **Handoff Isolation** — ardıl çağrı, çağıranın sonucunu ASLA değiştirmez. Ardıl
   başlatılamasa bile çağıran `completed` kalır; hata yalnız WFAH marker'ı
   (`call:next_failed`) + çağrı satırında görünür.
2. **WFC-CASCADE ardılı KAPSAMAZ.** Çağıran sonlandığında koşan alt akışlar
   (`wait`/`detached`) `cancelled` edilir; ardıl edilmez — ardıl, astın aksine çağıranın
   ömrüne bağlı değildir ve zaten çağıran bittikten sonra başlar.
3. **Yalnız başarılı `Terminal` tetikler.** `Failed` / `Terminated` (SLA-3 ihlali, engine
   hatası) ardıl TETİKLEMEZ — bunlar başarılı bitiş değildir.

**`start_as`** — ardılı hangi aktör başlatır:

| Değer | Aktör | Neden |
|---|---|---|
| `actor` (default) | Çağıranı bu noktaya getiren ACT'in aktörü (WFAH'ın SON kaydı) | Denetim izi doğal |
| `system` | Akışı BAŞLATAN aktör (WFAH'ın İLK kaydı) | Nil bir sistem aktörü hiçbir `c_a` ile eşleşmez, yani ardıl asla başlayamazdı; akışın başlatıcısı gerçek bir kullanıcıdır |

Kök `timeout` varsa terminal'e zaman aşımıyla da ulaşılabilir; o yolda aktör YOKTUR →
`start_as: "system"` şarttır (validator uyarısı `call_next_start_actor`).

Aktör çağrılanın start node c_a'sıyla eşleşmezse `WFD.CallUnauthorized`. Bu **statik
doğrulanamaz** (org resolve runtime'dır) — hata çağrı satırına yazılır, çağıran etkilenmez.

### 10e. Döngü frenleri — iki ayrı sayaç

Ardıl döngüsü (A bitince B, B bitince A) **sonsuz WFE üretir**; autoexec'te karşılığı
olmayan yeni bir başarısızlık sınıfıdır. Yuvalanma döngüsü ise sonsuz derinlik üretir.
Frenleri ayrıdır, sayaçları da:

| Sayaç | Neyi sayar | Statik fren | Runtime fren | Kaçış |
|---|---|---|---|---|
| `depth` | Alt akış yuvalanması (`wait`/`detached`) | `call_cycle` (reddet) | cap **8** | YOK |
| `next_depth` | Ardıl zinciri (`terminal`) | `call_next_cycle` (reddet) | cap **16** ya da `max_next` (küçük olan) | terminal'de `max_next: N` ile AÇIK izin |

Sınır aşılırsa çağrılan **hiç başlatılmaz**: satır `skipped` olur, çağıran bulunduğu
durumda kalır (Handoff Isolation).

**Statik döngü tespiti kenar üzerinde yapılır.** Kökün kendisi `WfdProvider`'dan
çözülemeyebilir (yayınlanmamış taslak); "hedefe git, orada kendini gör" yaklaşımı
A→B→A'yı kaçırırdı. Bu yüzden DFS yığınına giden bir kenar görüldüğünde döngü bildirilir.

### 10f. WFC-IN — girdi sözleşmesi

Çağrılanın girdi kümesi WOR-70 zinciriyle okunur: `start[]` → ACT →
`input.required`/`optional`. Tipler çağrılanın kendi `context` şemasından, start ACT'inin
`wfes_effects`'i (`$action.input.<x>` → `ctx.<y>`) izlenerek alınır.

İzinli kaynaklar: `$ctx.<yol>`, `$actor`, `$timestamp`, `$wfe_id`, literal.
**`$action.input.*` YASAKTIR** (`call_input_namespace`) — iki gerekçe:

1. **Moddan bağımsızlık:** `terminal` modunda ACT girdisi güvenilir biçimde mevcut değil
   (SLA-3 ile ulaşılan terminal'de hiç yok). Yasak olunca aynı katalog kaydı üç modda da
   aynı anlamı taşır.
2. **WOR-70 tutarlılığı:** ctx'e tek yazma yolu `wfes_effects`'tir. Bir ACT girdisini
   çağrılana geçirmek isteyen onu önce effects ile ctx'e yazar — böylece "çağrılana ne
   gitti" DynCtx'te **denetlenebilir** kalır, uçucu bir ara değer olmaz.

`$ctx.*` kaynağı çağıranın `context.properties`'inde **bildirilmiş** olmalıdır
(`call_input_source_undeclared`) — "çağrılanın girdileri çağıranın context'inde de
bulunmalı" kuralı budur ve resolver GEREKTİRMEZ, yereldir.

**WFC-RETURN effects bir context YAZARIDIR:** yalnız çağrı sonucundan dolan alan aksi
halde `context_field_never_written` ile yanlışlıkla reddedilirdi (§6b).

### 10g. Versiyon çözümü

`calls.<key>.wfd_id` bir DB uuid'si DEĞİL, çağrılan WFD'nin **doküman `id`**'sidir;
`version` ise doküman semver'i. Çözüm `wf.wfd_meta.doc_id`/`doc_version` indeksinden
yapılır (`WfdStore::resolve_doc`) ve yalnız `status='published'` + `is_active` satırları
döner — draft çağrılamaz.

- `version` verilmezse **en son yayınlanmış** sürüm (`version DESC`).
- Yaratılan WFE her hâlde start anında bir (wfd_id, version) çiftine sabitlenir → pin'siz
  çağrıda yeni sürüm yayınlamak **KOŞAN** WFE'leri etkilemez.
- `doc_id`'si NULL olan eski satırlar (migration öncesi) yeniden yayınlanana kadar
  çağrılamaz — sessiz yanlış eşleşmeye yeğ tutulur.

### 10h. Validator iki katmanlıdır

`validate()` yalnız YEREL kuralları koşar (saf `wfe-core`, I/O yok).
`validate_with(wfd, Some(&provider))` cross-WFD kurallarını da koşar: girdi kümesi
(`call_input_missing`/`unknown`), tip uyumu (`call_input_type_mismatch`),
`$call.result.*` anahtarları (`call_result_unknown`), döngü, versiyon.

Upload yolunda resolver DAİMA verilir: `wfd` crate çağrılanları **geçişli** olarak
ön-yükler (döngü tespiti çağrılanın kendi çağrılarını da görmeyi gerektirir), üst sınır
`MAX_PREFETCH = 64` — bozuk/çok derin bir graf upload'ı kilitlemesin. Sınır aşılırsa
uyarı loglanır ve runtime derinlik freni devreye girer.
