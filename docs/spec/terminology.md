# TERMINOLOGY — WFD Named Nodes Model v2.2

Bu doküman WFD domain kavramlarının kanonik referansıdır ve önceki tüm Terminology sürümlerinin yerini alır.

v2.2'nin iki ana kararı, ilk tasarım kararlarına dönüştür:
1. **C_A tek kuraldır** — "Candidate Actor for an ACT (C_A): Union of C_ORGU, C_R and C_U". OR'lu kural listesi (array) sonradan sızmış bir drift'ti, kaldırılmıştır.
2. **Her C_A bir node'dur** — "Each C_A is a node." Node kimliği c_a'dan deterministik türetilir; `label` sadece görünümdür.

> **Bu doküman kendi kendine yeter (2026-07-28).** Daha önce ORGANIZATION,
> USER/ROLE/ACTOR, ORGTRVLANG ve DynCtx/WFAH/WFES tanımları için ayrı bir
> `Terminology.MD` dosyasına bağımlıydı. O dosya kaldırıldı; ilgili bölümler
> aşağıya, **çalışan koddan doğrulanarak** taşındı. Projede başka terminoloji
> dokümanı YOKTUR.

---

## ORGANIZATION

**Organization Tree (ORGT):** Hiyerarşik ağaç. ORGT dışında her node'un tam olarak bir
parent'ı vardır. Her ORGT'nin bir veya daha fazla organizasyon tenant'ı (**ORGTNT**)
vardır; ORGTNT o organizasyonun köküdür.

**Organization Tenant (ORGTNT):** Bir organizasyonun kökü.

**Organization Unit (ORGU):** ORGT'deki bir node. Birden fazla ORGTNT altında
bulunabilir (`org.orgt_orgu` üyelik tablosu; ağaç üyeliği ORGU kimliğinden ayrıdır).

**ORGU Type (ORGU_T):** ORGU'nun `orgu_type` JSONB etiketi — `{"key": value}` biçimi;
boş olabilir. Filtreler bu harita üzerinde çalışır (bkz. ORGTRVLANG filtre semantiği).
Özel durum: `orgu_type` içinde **`*` anahtarı varsa ORGU her filtreyle eşleşir**
(joker birim).

**Aktiflik:** Traversal yalnız aktif satırları görür — hem `orgu.is_active` hem
`orgt_orgu.is_active` true olmalıdır.

### Depolama — PostgreSQL ltree

ORGT, **ltree** eklentisiyle saklanır; her ORGU kökten kendine olan tam yolu
nokta ayrılmış label dizisi olarak (`orgt_orgu.path`) tutar. `path` üzerinde
**GiST**, `orgu_type` üzerinde JSONB indeksi kullanılır.

```
ORGTNT root  →  path: '1'
Division     →  path: '1.10'
Branch       →  path: '1.10.100'
Credit Dept  →  path: '1.10.100.1001'
```

| Operatör | Anlam |
|---|---|
| `@>` | ancestor'ıdır |
| `<@` | descendant'ıdır |
| `subpath(p, offset, len)` | alt yol |
| `nlevel(p)` | derinlik |

---

## ORGTRVLANG — ORGT TRAVERSAL DİLİ

C_A'nın `c_orgu` alanı bu dille yazılır. Referans implementasyon:
`crates/org/src/traversal/parser.rs` (sözdizimi) + `executor.rs` (SQL semantiği).
**Kod ile bu bölüm çelişirse kod kazanır** — bölüm koddan türetilmiştir.

### Sözdizimi

Bir ifade bir **anchor** ve nokta ile zincirlenen **adım**lardan oluşur:

```
self[.adım][.adım]...
*:[filtre][.adım]...
```

- **`self` öneki zorunludur.** Çıplak `siblings` GEÇERSİZDİR (`ParseError::MissingSelf`);
  doğrusu `self.siblings`. Tek başına `self` = anchor ORGU'nun kendisi (adım yok).
- **`*:[filtre]`** anchor'dan BAĞIMSIZ global kaynak kümedir: ORGTNT genelinde filtreye
  uyan tüm ORGU'lar. Yalnız ilk adım olarak gelir, sonrasında normal adımlarla
  zincirlenir (`*:[type:sube].parent` = tüm şubelerin parent'ları).

### Adımlar

| Adım | Anlam |
|---|---|
| `parent` | Doğrudan parent |
| `siblings` | Aynı parent'ın diğer çocukları (kendisi hariç) |
| `siblings[F]` | Filtreye uyan kardeşler |
| `children` | Doğrudan çocuklar |
| `children[F]` | Filtreye uyan doğrudan çocuklar |
| `up[F]` | Yukarı doğru **en yakın** eşleşen ancestor (anchor başına tek) |
| `down[F]` | Aşağı doğru **en yakın** eşleşen descendant |
| `ancestors` | TÜM ancestor'lar (kendisi hariç) |
| `ancestors[F]` | Filtreye uyan tüm ancestor'lar |
| `*:[F]` | Global tip selektörü (yalnız ilk adım) |

`up`/`down` "en yakın"dır (`up` derinliğe göre DESC + anchor başına DISTINCT ON);
`ancestors` ise sınırsız kümedir. Zincir serbesttir — 9 sabit kalıpla sınırlı
DEĞİLDİR (`self.siblings.children[kredi]`, `self.up[bolge].children[il].children[sube]`
geçerlidir). Her adım bir öncekinin çıktısını anchor kümesi olarak alır; sonuçlar
`orgu_id`'ye göre tekilleştirilir.

### Filtre dili (`[...]` içi)

| Biçim | Anlam |
|---|---|
| `[sube]` | Kısayol — `type:sube` demektir (anahtar verilmezse `type`) |
| `[key:val]` | `orgu_type` JSONB'de `key` → `val` |
| `[role:analyst]` | **İlişkisel** — birimin aktif `org.orgu_r` grant'ı var mı (zaman aralığı `valid_from`/`valid_until` ile kontrol edilir) |
| `[a,b]` | Aynı anahtar için virgül = OR (`[role:doviz,kredi]`) |
| `&&` `\|\|` `!` `( )` | Boolean birleşim (`[role:doviz,kredi && type:sube]`, `[!type:pasif]`) |

Tip yaprağı hem skaler hem dizi değerle eşleşir
(`orgu_type->>key = val` OR `orgu_type->key @> val`). Boş filtre (`[]`) hatadır.

```
self.children[role:doviz,kredi && type:sube]
self.up[bolge].children[il]
*:[type:sube]
self.ancestors[!type:pasif]
```

---

## USER / ROLE / ACTOR

**User (U):** ORGT içinde tekil ID'si olan kullanıcı. Birden fazla ORGU'ya bağlı olabilir.

**Role (R):** ORGT içinde tekil ID'si olan anahtar kelime.

**User Role (UR):** `(U, [{ORGU:type, timeslice}, R]..)` — ORGU kapsamı ORGTRVLANG ile
veya doğrudan ORGU:type ile verilebilir. Bir U; ORGU'dan bağımsız, belirli ORGU'lar
için veya belirli ORGU_T'ler için, zaman aralıklı ya da aralıksız birden fazla R
taşıyabilir.

**Actor (A):** Bir WFE üzerinde ACT uygulayan **(ORGU, (U, R))** üçlüsü. Tam eşleşen
bir tuple'dır — C_A eşleşmesi bu üçlü üzerinden yapılır.

---

## DynCtx / WFAH / WFES

**Workflow Definition (WFD):** Bir iş akışının tamamını tanımlayan JSON.

**Workflow Execution (WFE):** Bir WFD'nin tekil örneği (instance).

**Dynamic Context (DynCtx):** WFE'nin değişken durumunu tutan JSON.
**Değişmezdir (immutable)** — ya tümü yeni bir örneğe kopyalanır ya da diff'ler
saklanıp yeni DynCtx merge ile elde edilir.

**WFE Starting Context (WFE-SDynCtx):** WFE başlatılırken sahip olduğu bağlam.
WFE'yi başlatan **A** gibi zorunlu parçalar içerir; ek zorunlu parçalar WFD'de
tanımlanabilir.

**Workflow Actions History (WFAH):** WFE'ye uygulanmış `(ACT, A)` tuple'larının geçmişi.

**Workflow Execution State (WFES):** WFE'nin tam durumu = DynCtx ∪ WFAH.

**Action (ACT):** Uygulandığında WFES'i değiştirebilen eylem.

**Permission (P):** `P(WFES, A, ACT) → true | false`. Tam değerdir.

**Workflow Transition (WFT):** `WFT(WFES, A, ACT) → (yeni WFES, yeni C_A)`.

**Possible Actions (P_ACT):** `P_ACT(WFES) → {A, ACT}`.

**Possible Actions for an Actor (P_ACT_A):** `P_ACT_A(WFES, A) → {ACT}`.

---

## MODEL ÖZETİ

```text
nodes       = isimli bekleme havuzu katalogu; key = slug(c_a), label = display
actions     = human ACT katalogu
autoexec    = reusable sistem executable katalogu
calls       = reusable WFD cagrisi katalogu (WFC: ne + hangi girdi)
transitions = (from node + ACT) -> effects + trigger + WFT kenari
trigger     = autoexec invocation listesi (retry/catch destekli)
nodes.<k>.call    = alt akis cagrisi   (mode: wait | detached; donuslu)
terminals[].call  = ardil akis cagrisi (mode: terminal; bitis = ardilin baslangici)
wft         = tek routing authority; hedef node id veya terminal id
```

---

## C_A — CANDIDATE ACTOR (TEK KURAL)

```json
"c_a": { "c_orgu": "self", "c_r": ["creditAnalyst"], "c_u": ["user_ali"] }
```

| Alan | Zorunlu | Anlam |
|---|---:|---|
| `c_orgu` | Evet | Scope çapası: ORGTRVLANG token, static selector veya anchor object |
| `c_r` | c_r/c_u'dan en az biri | Rol kanalı |
| `c_u` | c_r/c_u'dan en az biri | Kişi kanalı (istisna izni) |

**Eşleşme semantiği (kanonik):**

```text
match = (actor.orgu ∈ resolve(c_orgu)) AND (rol_match OR user_match)

rol_match  = c_r varsa actor.role ∈ c_r,  yoksa false
user_match = c_u varsa actor.user ∈ c_u,  yoksa false
```

Kritik kurallar:

- **Yok = false, wildcard değil.** `c_r` yazılmadıysa rol kanalı kapalıdır; kural "ORGU'daki herkes"e dönüşmez.
- **`c_orgu` her zaman AND'lenen çapadır.** Rol ve kişi, çapadan asla kopmaz (Actor exact tuple felsefesi).
- **`c_u` match'i rol-agnostiktir.** Kişi, anchor ORGU'daki herhangi bir rol kaydıyla havuza girer; ACT yine somut `(ORGU,(U,R))` tuple'ıyla uygulanır ve WFAH'a o tuple yazılır.
- **Sadece-kişi havuzu:** `{ "c_orgu": "self", "c_u": ["user_ayse"] }` — c_r hiç yazılmaz.
- **Alternatif havuz ("analist VEYA üst müdür")** tek kuralla İFADE EDİLEMEZ; iki ayrı node veya `listable` kaydı olarak modellenir. Bu bilinçli bir kısıttır: bir c_a = bir node.

---

## NODE

```json
"self__creditAnalyst": {
  "label": "Analist Havuzu",
  "description": "Başvuru analist havuzunda bekliyor.",
  "c_a": { "c_orgu": "self", "c_r": ["creditAnalyst"] },
  "escalation": [
    { "after": "P3D", "wft": { "node": "self__branchManager" } }
  ]
}
```

- **Key = slug(c_a):** editör üretir, kullanıcı yazmaz; validator yeniden hesaplayıp karşılaştırır (algoritma: runtime-semantics §2a).
- **Label:** UI'da görünen serbest metin; kimlik DEĞİLDİR. Versiyon-aşırı metrikler label üzerinden agregat edilmelidir (kural değişince slug değişir).
- **Uniqueness:** Aynı canonical c_a workflow'da en fazla bir node'da bulunur.
- **Node vs assignment:** claim node'u değiştirmez; assignment runtime metadata'dır.

```text
WFES = current_node + assignment + DynCtx + WFAH
```

WFD dokümanı versiyonludur; kural değişikliği yeni WFD versiyonu doğurur, çalışan WFE'ler başladıkları versiyona sabittir (slug kararlılığı bu şekilde sağlanır).

---

## TRANSITION

```json
{
  "id": "t_manager_decide",
  "from": ["self__branchManager", "parent__creditDeptManager"],
  "action": "manager_decide",
  "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
  "wfes_effects": {
    "set": {
      "manager_decision": "$action.input.manager_decision",
      "manager_reviewed_at": "$timestamp"
    }
  },
  "trigger": [{ "use": "audit_log", "required": false }],
  "wft": {
    "conditions": [
      { "when": "$action.input.manager_decision == 'approve'", "terminal": "terminal_approved" }
    ],
    "default": { "terminal": "terminal_rejected" }
  }
}
```

- `from`: kaynak node slug'ı (string/array). `when` sadece ek veri guard'ıdır.
- `c_a` (opsiyonel, TEK kural): EK kısıt — owner ayrıca bu kurala da match etmelidir.
- Aynı (node, action) için birden fazla transition: array sırasında `when`'i true olan İLK seçilir.
- **WOR-70:** aksiyonun bildirdiği her input, bu kuralın `wfes_effects`'inde
  `$action.input.<yol>` ile ctx'e yazılmak ZORUNDADIR — girdi kendiliğinden ctx'e
  geçmez (`unused_action_input`). Yukarıdaki örnekte `manager_decide`'ın
  `input.required = ["manager_decision"]` bildirimi bu yüzden bir `set` satırıyla
  eşleşir.

---

## START (v2.2 — TRANSITION İLE SİMETRİK)

```json
{
  "id": "start__type_branch__branchClerk",
  "from": "type_branch__branchClerk",
  "action": "Akışı Hazırla",
  "wfes_effects": { "set": { "initiated_by": "$actor" } },
  "wft": { "node": "self__creditAnalyst" }
}
```

- `from`: WFE'yi başlatabilecek adayların havuzunu tutan bir **start node** slug'ı (tekil, array değil) — `nodes` kataloğundaki normal bir node.
- `action`: start aksiyonunun gerçek adı — rezerve sabit değildir, `actions{}` içinde normal bir ACT olarak tanımlanır (transition'lardaki `action` ile aynı kural).
- **Start node = referans ile türetilmiş kimlik.** Node'un kendisinde `kind` alanı YOKTUR; bir node sadece `start[].from` tarafından işaret edildiği için start node'dur. `c_a` (kim başlatabilir) o node'un üzerinde durur — TRANSITION'daki `node.c_a` (kim owner) ile aynı yerdedir, artık startRule üzerinde inline DEĞİLDİR.
- Start node, hiçbir transition/start'ın `wft` hedefi olamaz (giriş-only, yeniden girilemez) ve `escalation` taşıyamaz.
- `wfes_effects`/`trigger`/`wft` start EDGE'inde kalır (node'da asla).

---

## WFT / TRIGGER / AUTOEXEC / PIPELINE

v2.1 ile aynıdır:

- WFT formları: `{node}`, `{terminal}`, `{conditions[], default?}`; ilk-match; default yoksa `WFD.NoConditionMatched`.
- Trigger: `use` + `when?` + `required?`(default true) + `retry[]?` + `catch?`. Fail akışı: retry → catch (effects, handled, devam) → required davranışı.
- Autoexec: root katalog, `timeout_seconds` (default 60), tek çıktı namespace `$exec.result.*`; routing alanları yasak.
- Pipeline atomiktir: diff'ler ancak WFT çözülünce commit edilir; başarılı transition sonrası WFE yeni node'a UNASSIGNED girer.
- Hata taksonomisi: `WFD.ALL`, `WFD.Timeout`, `WFD.AutoexecFailed`, `WFD.NoConditionMatched`.

---

## EXPRESSION NAMESPACE

```text
$ctx  $wfah  $prev  $first  $node  $actor  $timestamp  $wfe_id
$action.input.*  $exec.result.*
$call.result.*  $call.status  $call.wfe_id      (yalniz WFC-RETURN baglami)
$branches.*  $arrived                           (yalniz join_when baglami)
```

Geçersiz: `$status` gibi top-level DynCtx, `$exec.response.*`, `$ctx.status` ile state guard'ı.

### `$wfah` ve uç girdi kısayolları (WOR-84)

`$wfah` append-only aksiyon geçmişidir; her girdi şu alanları taşır:

| alan | anlam |
|---|---|
| `seq` | WFE başına monotonik sıra (1'den başlar) |
| `action` | aksiyon adı |
| `actor` | `{orgu_id, user_id, role}` |
| `input` | aksiyonun ham girdisi — ctx'e yazılmamış olsa da geçmişte durur |
| `at` | Zaman damgası METNİ — UTC `yyyyMMddHHmmss`, 14 rakam (`"20260115103000"`). Karşılaştırmaları **string temellidir**, `d()` sarmalı yok: leksikografik sıra kronolojik sıraya eşit olduğu için `startsWith(#.at, "20260115")` = "o gün", `"202601"` = "o ay". Eşitlik TAM damga ister. Sıralama (`>` `<`) motorda metinde runtime hatası verir — `seq` kullanın. Aynı biçim `$timestamp` için de geçerlidir (`wfe-core/src/timestamp.rs` tek kaynak). |

`$prev` = geçmişin **son** girdisi, `$first` = **ilk** girdi; aynı alan kümesini taşırlar.
Geçmiş boşsa her alan `null` döner ve ifade **patlamaz** (`$call` ile aynı kabuk gerekçesi).

**`$wfah`'ı doğrudan indekslemeyin.** `$wfah[len($wfah) - 1].action` geçmiş boşken
indeks `-1`'e düşer ve VM patlar (`Fetch: Failed to convert to usize`); parse aşaması
bunu yakalamaz. Validator `wfah_index_unguarded` uyarısı basar. Negatif indeks
(`$wfah[-1]`) zen'de yoktur — parse edilir, runtime'da patlar; validator
`zen_negative_index` **hatası** verir.

**Dizi fonksiyonları İKİ argümanlıdır** (dizi + closure):
`count` `some` `all` `none` `one` `filter` `map` `flatMap`.

```text
count($wfah, #.action == "onay") >= 2     ✅
len(filter($wfah, #.action == "onay")) >= 2 ✅
count(filter($wfah, #.action == "onay")) >= 2  ❌ parse hatası (tek argümanlı count)
every($wfah, ...)                          ❌ böyle bir fonksiyon yok → `all`
```

Sıralama operatörleri (`>` `<` `>=` `<=`) `null` ile **hata** verir (`==`/`!=` vermez).
Bu yüzden `#.input.*` üzerinde sayısal karşılaştırma aksiyona kapılanmalıdır:
`some($wfah, #.action == "skor_gir" and #.input.tutar > 1000)` — kapısız hâli, girdisi
olmayan bir geçmiş satırında `null > 1000`'e düşer ve patlar.

`$call.*` `$exec.result.*` ile **birleştirilmez** — autoexec bir sistem çağrısıdır, WFC bir
WFE örneğidir. WFC-RETURN dışındaki bağlamlarda `$call` boş bir kabuktur (null döner).

---

## VISIBILITY / V — AUTHORIZATION'DAN FARKLI MANTIK

V, DynCtx field-level READ filtresidir; asla ACT/claim vermez. Gören ≠ eden.

**x-visibility kriterleri BAĞIMSIZDIR ve aralarında OR vardır** (orijinal karar):

```json
"x-visibility": {
  "c_r": ["creditDeptManager"],
  "c_u": ["user_denetci_ali"],
  "c_orgu": "*:[type:hq]",
  "c_a": { "c_orgu": "self", "c_r": ["branchManager"] }
}
```

Okunuşu: departman müdürü rolü olan herkes (her yerde) VEYA denetçi Ali (her yerde) VEYA HQ'daki herkes VEYA tam kuralla match edenler bu alanı görür. `c_r`/`c_u` burada scope'suzdur; scope'lu grant istenirse `c_a` (tek tam kural) kullanılır.

Gerekçe: authorization dar ve çapalı olmalıdır (sonucu ACT); görünürlük ekleyici izin listesidir (sonucu sadece okuma). İki matcher AYRI fonksiyondur, birleştirilmez.

---

## LISTABLE / L

L, WFE-level ek görünürlüktür; ACT/claim vermez. v2.2'de her kayıt TEK c_a kuralı taşır; çoklu grant = çoklu bağımsız kayıt:

```json
"listable": [
  { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] } },
  { "when": "$ctx.credit_info.amount_requested >= 100000",
    "c_a": { "c_orgu": "parent", "c_r": ["creditDeptManager"] } }
]
```

Claim/owner/ACT semantiği öncekiyle aynıdır (unassigned: C_A görür+claim eder; assigned: sadece owner ACT; escalation taşırsa assignment temizlenir).

---

## SCHEMA ANNOTATION UZANTILARI

`context` bir JSON Schema 2020-12 dokümanıdır; `context.properties` altındaki
field'lar WFD'ye özel bir uzantı taşıyabilir.

**`x-visibility`** *(obje)* — field seviyesinde görünürlük kuralı; şekli C_A ile
aynıdır (`c_orgu` / `c_r` / `c_u`). Kriterler **bağımsızdır ve aralarında OR
vardır**; kural sağlanmazsa Actor field'ı DynCtx'te göremez. Listable erişimi
olan Actor için de geçerlidir. Ayrıntı: **VISIBILITY / V** bölümü.

> **`x-wf-readonly` KALDIRILDI (WOR-71).** "Bu alanı yalnız engine yazar" artık
> ayrı bir flag ile değil, WFD'nin kendisinden okunur: alan hiçbir
> `actions.<ad>.input`'ta bildirilmemişse yalnız `wfes_effects` doldurabilir
> (WOR-70 — context'e tek yazma yolu effects'tir). Eski `_step_<action>`
> injection discriminator rolü de v2.2'de zaten GEÇERSİZDİ (`_step_*`
> kaldırıldı, bkz. DEPRECATED).

**`required` KULLANILAMAZ (WOR-70).** Ne `context.required` ne de bir field'ın
içindeki `required` listesi geçerlidir; ikisi de WFD'yi reddettirir
(`context_required_removed`). Zorunluluk **tek yerde** bildirilir:
`actions.<ad>.input.required`. Anlamı da tek: *"bu aksiyonu tetikleyen istekte şu
isimde parametreler bulunmak zorunda."* Değerin ctx'e taşınması ayrı bir adımdır ve
yalnız `wfes_effects` ile olur (runtime-semantics §7.5a); tutarlılığı
`context_field_never_written` + `unused_action_input` kuralları korur (§6b).

**`required` ↔ `optional` (WOR-70b).** İkisi de `wfes_effects` ile ctx'e eşlenmek
zorundadır; `optional` olmak muafiyet değildir. Fark yalnız değerde: `required`
gönderilmek zorunda ve `null` olamaz, `optional` gönderilmezse alan `null` kalır.
Ayrıntı: runtime-semantics §7.5a.

---

## CANVAS NODE MODELİ (yalnız editör)

> Bu kavramlar wfd-editor'ün görsel canvas'ına özgüdür. **WFD JSON'ında doğrudan
> karşılıkları yoktur**; editör export sırasında bunları standart WFD yapısına
> dönüştürür. Engine bu terimleri bilmez.

**CaGroup Node:** Bir veya daha fazla `ActionStep`'i gruplayan container node.
WFD JSON'ında bir `nodes.<key>` girdisine (yani bir `c_a`'ya) karşılık gelir.

**ActionStep Node:** Bir `ACT` + `c_a` kombinasyonunun görsel temsili —
`transitions[]` veya `start[]` içindeki tek bir kural.

**AutoexecStepNode:** `autoexec` içeren bir transition'ın canvas temsili
(`rest`, `sql` veya `calc`).

**SwitchStepNode:** `wft.conditions` array'inin görsel temsili; her dal bir
`WftCondition`.

**TerminalStepNode:** `terminals[]` içindeki bir `TerminalDef`'in temsili.

---

## DEPRECATED (v2.2'de geçersiz)

```text
c_a array formu ("c_a": [ {...} ]) ve kurallar-arasi OR semantigi
elle yazilmis node isimleri (key slug'dan turetilir; insan ismi 'label'dadir)
x-visibility icinde alanlar-arasi AND varsayimi
startRule.c_a inline formu ve from/action eksikligi (c_a artik start node'da; amended v2.2 in place)
$ctx.status state konvansiyonu, wft.c_a inline form, $exec.response.*   (v2.1'den devam)
terminal:true, {ctx:...}, {ref:...}, _step_<action>, c_a[].from          (v1/v2'den devam)
```

## Attachment (Ek-belge)

| Terim | Anlam |
|---|---|
| **Attachment katalogu** | Root `attachments` — adlandırılmış grupların sözlüğü. Her grup `items[]` taşır. Bir kez tanımlanır, node'lardan adıyla referanslanır. |
| **Attachment grubu** | Katalogdaki bir kayıt: `{label?, description?, items[]}`. Aynı grup birden fazla node'dan referanslanabilir. |
| **Attachment item (dosya slotu)** | `{id, label?, description?, required?, formats?}`. `id` = "verilen dosya ismi", grup içinde tekil. `required` (default true) yüklenmeden gruba bağlı node'dan aksiyon alınamaz. |
| **Attachment format kuralı** | `formats[]` içindeki bir kayıt: `{accept: string[], max_size_mb?}`. Bir MIME grubu + o gruba ÖZEL boyut sınırı. Farklı formatlar farklı MB (örn. pdf/jpg→4MB, xml/zip→20MB). `formats` boş/yoksa: her tip, sınırsız. |
| **Node attachment referansı** | `nodes.<key>.attachments` — grup key'leri dizisi. WFE bu node'da beklerken referanslı grupların `required` dosyaları yüklenmeden aksiyon submit edilemez. |
| **AttachmentStore** | Server portal katmanının opendal store'u. Storage anahtarı `attachments/{wfe_id}/{grup}/{item}`. Engine core buna DEĞMEZ; dosya varlığı/yükleme yalnız edge'dedir. |

## WFC — İŞ AKIŞI ÇAĞRISI (Workflow Call)

Bir WFE'nin başka bir WFD'yi örneklemesi. **Tek katalog (`calls`), tek eksen (`mode`),
üç mod.** Tam tasarım: `docs/plans/workflow-call.md`; kararlar: `decisions.md` → WFC.

| Terim | Kısaltma | Tanım |
|---|---|---|
| **Workflow Call** | **WFC** | Root `calls` kataloğundaki kayıt: **ne** çağrılacak (`wfd_id`, `version?`, `start?`) ve **hangi girdiyle** (`input`). *Nasıl* çağrıldığını `mode` söyler. Katalog↔referans ayrımı `autoexec`↔`trigger`'ın aynısıdır. |
| **Call Mode** | — | `wait` / `detached` / `terminal`. Çağrının TEK belirleyici eksenidir; yerleşimi de mod belirler. |
| **Caller** | **WFE-P** | Çağıran WFE. |
| **Sub Callee** | **WFE-C** | `wait`/`detached` ile yaratılan **alt** WFE. Çağıran yaşamaya devam eder. |
| **Successor Callee** | **WFE-N** | `terminal` ile yaratılan **ardıl** WFE. Çağıran zaten bitmiştir. **Ast değildir, ardıldır** — hiyerarşi değil sıra. |
| **Call Node** | **WFC node** | `nodes.<k>.call` taşıyan node. Bekleme *havuzu* DEĞİLDİR: insan ACT'i alınamaz. |
| **Handoff Terminal** | **WFC terminal** | `terminals[].call` taşıyan terminal. WFE-P burada NORMAL sonlanır (`completed`), ardından WFE-N başlar. |
| **Call Input Map** | **WFC-IN** | `calls.<k>.input`. Moddan bağımsızdır ( `$action.input.*` yasağı sayesinde). |
| **Projected Field** | **WFC-PROJ** | Eşlenmemiş bir girdi için çağıranın `context.properties`'ine editörün ürettiği alan. |
| **Call Result** | **WFC-OUT** | Çağrılanın `wfe_end_response`'unun çağırana taşınması (`$call.*`). **Yalnız `wait`.** |
| **Return Edge** | **WFC-RETURN** | `call.wfes_effects` + `call.wft`. Çağrılan bitince işleyen, insan ACT'i olmayan kenar — `escalation`/`claim_timeout` kenarlarıyla aynı sınıf. |
| **Cascade** | **WFC-CASCADE** | Çağıran sonlandığında koşan **WFE-C**'lerin `cancelled` edilmesi. **WFE-N'ye UYGULANMAZ.** |
| **Handoff Isolation** | — | Ardıl çağrı, çağıranın sonucunu ASLA değiştirmez. |

Üç mod, üç davranış:

```text
wait      cagiran node'da BEKLER  -> cagrilan bitince WFC-RETURN isler, $call.* gorunur
detached  cagrilan baslar, cagiran HEMEN devam eder ($call.result.* daima bos)
terminal  cagiran BITER (completed) -> ardil akis baslar; donus, bekleme, cascade YOK
```

Mod ↔ yerleşim eşlemesi zorunludur: `wait`/`detached` yalnız `nodes.<k>.call`,
`terminal` yalnız `terminals[].call`.

**WFC node'unda `c_a` hâlâ zorunludur** (slug/uniqueness değişmezleri korunur); anlamı
daralır: *alt akış sürerken bu WFE'yi kim görür ve kim iptal edebilir*. WFC node'u
`transitions[].from` içinde yer alamaz, `escalation`/`claim_timeout`/`attachments`/
`reassign` taşıyamaz, `start[].from` olamaz; çıkışı `call.wft`'dir (zorunlu).

**Ardıl sıralaması kesindir:** terminal `wfes_effects` → `wfe_end_response` →
çağıran `completed` commit → *ondan sonra* ardıl start. Yalnız başarılı `Terminal`
tetikler; `Failed`/`Terminated` tetiklemez. Ardıl döngüsü (`max_next` ile açıkça
istenmedikçe) reddedilir.
