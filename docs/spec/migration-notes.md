# WFD Migration Notes — v2 / v2.1 → v2.2

Bu doküman, mevcut engine/editor kodunun v2.2 modeline taşınması için delta'yı listeler ve önceki tüm migration notlarının yerini alır. Kanonik referanslar: `schema.json`, `terminology.md`, `runtime-semantics.md`, golden fixture `examples/kredi-basvuru.golden.json`, referans Rust modeli `reference-types.rs`.

Not: v2.1 hiç deploy edilmediyse bu doküman tek adımda v2 → v2.2 geçişi olarak okunur; M1–M9 v2.1'den taşınan maddelerdir, M10–M14 v2.2'nin ekleridir.

---

## M1. Root `nodes` Kataloğu

State = isimli bekleme havuzu. `$ctx.status` konvansiyonu ve onu set eden tüm effects kodu silinir. Engine `current_node` tutar, expression'lara `$node` olarak sunar.

## M2. Transition: `from` zorunlu, `when` opsiyonel guard

```json
{ "id": "t_x", "from": "self__creditAnalyst", "action": "approve", ... }
```

`from` string veya array. Seçim: aynı (node, action) için array sırasında `when`'i true olan İLK transition.

## M3. WFT formları: `{node}` / `{terminal}` / `{conditions, default?}`

Inline `wft.c_a` YOK. `default` yoksa match'sizlik = `WFD.NoConditionMatched`.

## M4. Trigger `retry[]` + `catch`

ASL semantiği. Catch effects uygular, handled sayar, devam eder; routing yapmaz.

## M5. Timeout'lar

`autoexec.*.timeout_seconds` (default 60) → `WFD.Timeout`; root `timeout` (ISO 8601).

## M6. Escalation (Node SLA)

`nodes.*.escalation[]`; assigned WFE'de de çalışır, taşımada assignment temizlenir.

## M7. Tek exec namespace: `$exec.result.*`

`$exec.response.*` her yerde hataya çevrilir.

## M8. Atomik pipeline

Diff'ler ancak WFT çözülünce commit; başarılı transition sonrası yeni node'a UNASSIGNED giriş.

## M9. Küçükler (v2.1'den)

`actions.*.name` → `label`; `terminal.wfes_effects` opsiyonel; `wfahAnchor.occurrence` (default last); hata taksonomisi `WFD.*`.

---

## M10. C_A TEK KURAL (v2.2 — EN KRİTİK)

Array formu ve kurallar-arası OR semantiği KALDIRILDI (orijinal karara dönüş).

Eski (v2/v2.1):

```json
"c_a": [
  { "c_orgu": "self", "c_r": ["creditAnalyst"] },
  { "c_orgu": "parent", "c_r": ["branchManager"] }
]
```

Yeni (v2.2):

```json
"c_a": { "c_orgu": "self", "c_r": ["creditAnalyst"] }
```

- Tek elemanlı array → objeye indirgenir (mekanik dönüşüm).
- ÇOK elemanlı array → tasarım kararı gerektirir: iki ayrı node'a bölünür veya ikincisi `listable` kaydına iner. Otomatik dönüştürülmez; migration aracı bu durumda İNSAN ONAYI istemelidir.
- Uygulandığı yerler: `nodes.*.c_a` (start node dahil — bkz. M15), `transitions[].c_a` (ek kısıt), `listable[].c_a`.
- Engine matcher: `resolved(c_orgu) AND (rol_match OR user_match)`; yok = false; c_u rol-agnostik. `any()` döngüsü silinir.

## M11. Node Kimliği = slug(c_a), İsim = `label` (v2.2) — **GEÇERSİZ, bkz. M18(a)**

> 2026-08-12'de kaldırıldı: node kimliğini TASARIMCI verir, `slug(c_a)` kimlik değildir.
> Aşağıdaki metin tarihsel kayıttır.

- Node key elle yazılmaz; runtime-semantics §2a algoritmasıyla türetilir (`self__creditAnalyst`, `parent__creditDeptManager`, `wfah_submit_parent__branchManager`, c_u-only: `self__u_user_ayse`).
- `nodeDef.label` eklendi (UI ismi).
- Validator: key == slug kontrolü + canonical c_a uniqueness.
- Editör: c_a düzenlenince slug yeniden üretilir, tüm referanslar otomatik yeniden bağlanır; label korunur.

## M12. Aynı c_a = tek node kuralı (v2.2) — **YÜRÜRLÜKTE, bkz. M18(b)**

"Duplicate c_a node meşrudur" yaklaşımı TERS DÖNDÜ: aynı canonical c_a ikinci node'da = HATA. UI'da bir havuz bir kez görünür; aksiyonlar node içinde slot'tur.

> Bu kural 2026-08-12'de UYARIYA (`shared_c_a`) çevrilmiş, 2026-08-14'te HATA olarak GERİ
> GETİRİLMİŞTİR (M18(b)). Kimlik ise M11'in aksine tasarımcınındır.

## M13. Visibility matcher OR + ayrı fonksiyon (v2.2)

`x-visibility` kriterleri bağımsız, aralarında OR; `c_r`/`c_u` scope'suz; `c_a` (tek kural) scope'lu grant. Authorization matcher'ı ile TEK fonksiyonda birleştirilmez. Eski `c_user` → `c_u`.

## M14. Root metadata

`wfd_version: "2.2"` zorunlu; tanınmayan versiyon = yükleme reddi. `expression_language` default `"zen@1"`.

---

## M15. Start Simetrik Hale Geldi — `from` + `action:"start"` (v2.2 — amended in place)

Öncesi: `startRule = { id, c_a(inline), wfes_effects?, trigger?, wft }` — `from`/`action` yoktu, `c_a` doğrudan startRule üzerinde (node'a bağlı değil).

Sonrası: `startRule = { id, from, action:"start", wfes_effects?, trigger?, wft }` — her start rule, `from` ile `nodes` kataloğundaki bir girdiye referans verir; o node `c_a`'yı taşır (transition'daki `node.c_a` ile aynı yer). `action` rezerve sabit `"start"`'tır ve `actions{}` içinde tanımlanamaz. Start node kimliği `start[].from` referansından türetilir; node'a `kind` alanı eklenmedi. `wfd_version` `"2.2"` kalır (amend in place, v2.3 yok).

## M16. Start Aksiyonu Artık Rezerve Kelime Değil — Gerçek Ad Taşır (v2.2 — amended in place)

Öncesi: `start[].action` her zaman rezerve sabit `"start"` yazılırdı (M15, custom validator V4/V5); editördeki başlama aksiyonunun gerçek adı (varsa) atılır, wire format'ta hiçbir yerde görünmezdi. WFAH anchor'ları da (`c_orgu.from.wfah`) bu yüzden literal `"start"` ile eşleştirilirdi.

Sonrası: Editörde "Başlama aksiyonu olsun" checkbox'ı (`isStart`) hangi ActionStep'in start rule'a gideceğini belirler — isim bu kararı hiç etkilemez (ad artık serbest, ör. `"Akışı Hazırla"`). Export, o adımın gerçek `action` adını `start[].action` alanına yazar ve bu ad `actions{}` içinde normal bir ACT olarak tanımlanır (eski V5 kuralı kaldırıldı). WFAH anchor'ları da aynı gerçek adı referans alır. Şema: `startRule.action` artık `const:"start"` değil, `transition.action` ile aynı `idName` tipindedir.

## M17. Node-level `listable` Eklendi (v2.2 — amended in place, 2026-08-13)

Öncesi: `listable[]` yalnız WFD KÖKÜNDE vardı (`start`/`transitions`/`nodes` ile aynı
seviye) — bir kural WFE'yi durumdan bağımsız, KALICI olarak ek listeye alırdı. "Bu işi
yalnız şu node'dayken göster" isteği motor seviyesinde ifade edilemiyordu.

Sonrası: `$defs/nodeDef`'e opsiyonel `listable` alanı eklendi — `nodes` kataloğundaki
HER girdi (CaGroup node'u VE CallStep node'u) artık kendi `nodes.<key>.listable[]`
kaydını taşıyabilir. Şekil kök `listable[]` ile AYNIDIR (`{c_a, when?}`); fark yalnız
ÖMÜRDE: node listable **duruma bağlıdır** — WFE ilgili node'da (paralel modda aktif
kollardan biri o node'daysa) İKEN geçerlidir, WFE node'dan çıkınca (veya terminal'e
ulaşınca) görünürlük SONA ERER. Kök `listable[]` KALICI kalır — anlamı ve şekli
DEĞİŞMEDİ. Runtime karşılığı: `can_view` (f) kriteri (bkz. `runtime-semantics.md`
§4a), SQL projeksiyonu `wf.wfe.current_view_c_a` / `wf.wfe_branch.view_c_a` (bkz.
`decisions.md`).

`wfd_version` **"2.2" KALIR** — amend in place, M15/M16 precedent'i (v2.3 yok). Alan
**opsiyonel ve additive**: eski belgeler HİÇBİR DEĞİŞİKLİK yapılmadan yüklenir ve
çalışır, `nodes.<key>.listable` alanı yoksa (default `[]`) hiçbir davranış değişmez.
**Eski belgelerin kök `listable[]`'ı GLOBAL listable olarak okunmaya devam eder** —
okuyucu silinmez, migration yolu YOKTUR (bu tür bir davranış için bkz. proje hafızası
"Wire formatı değişince okuyucu kalır"). Golden fixture
(`docs/spec/examples/kredi-basvuru.golden.json`) DEĞİŞMEDİ — kök `listable[]` kullanır,
alan opsiyonel olduğu için değiştirilmesine gerek yoktur.

**Editör tarafı (etkisi bu dosyanın kapsamı dışında ama not edilir):**
`ActionStep.listable_for` (editor-only alan, hiçbir zaman WIRE'a çıkmamıştı — export
`serializeListable` ile hepsini kök `listable[]`'a düzleştiriyordu, node bağı export'ta
kaybolduğu için round-trip'te de geri gelmiyordu) KALDIRILDI. Bu alan hiç yayına
çıkmadığı için **yayınlanmış belgeler için veri kaybı YOK**; düzeltilen şey tasarımcının
"bu adımda görsün" niyetinin export'ta sessizce "her zaman görsün"e (global) dönüşmesiydi
— artık node listable ile doğrudan ifade edilebilir.

## M18. Node Kimliği Tasarımcının, ama Aynı c_a = Aynı Kimlik (2026-08-12 + 2026-08-14, **KIRICI**)

M11 ve M12 bu maddeyle güncellenir. İki ayrı hamle vardır, karıştırılmamalıdır:

**(a) 2026-08-12 — `node key == slug(c_a)` KALDIRILDI (M11 geçersiz).** Node kimliğini
TASARIMCI verir; `c_a` node'un bir ALANIDIR, kimliği değil. Sebep: kimlik ORGTRVLANG org
yolunu taşıyordu, "bu adımı kim yapar"ı değiştirmek adımın KİMLİĞİNİ bozuyordu. Anahtarın
BİÇİM kısıtı şemada durur (`nodes` `propertyNames: idName`, `^[A-Za-z_][A-Za-z0-9_-]*$`);
validator key'i yeniden hesaplayıp KARŞILAŞTIRMAZ. Bu hamle **geriye uyumluydu** (yalnız
kısıt kalktı): `self__creditAnalyst` gibi mevcut anahtarlar deseni zaten sağlar, veri
taşıması YOKTU.

**(b) 2026-08-14 — `duplicate_c_a` HATA olarak GERİ GETİRİLDİ (M12 yeniden yürürlükte).**
2026-08-12 aynı hamlede bu kuralı da kaldırıp UYARIYA (`shared_c_a`) çevirmişti; o kısım
geri alındı. Yeni değişmez: **aynı `c_a` = aynı kimlik, aynı kimlik = aynı `c_a`** → bir
canonical `c_a` belgede EN FAZLA BİR node'da bulunabilir. Validator kodu:
`duplicate_c_a`, seviye **HATA**. Gerekçe ve feda edilenler: `decisions.md`, 2026-08-14.

**KIRICI — ne bozulur.** UYARI döneminde (2026-08-12 → 2026-08-14) aynı c_a'lı İKİ node
taşıyan bir belge yayınlanabiliyordu. Böyle bir belge **ARTIK YAYINLANAMAZ**: upload /
publish / submit / approve / `POST /wfd/validate` `duplicate_c_a` hatası döndürür. Koşan
WFE'ler kendi (id+version) belgesine sabit olduğu için ETKİLENMEZ — kapı yalnız YENİ
sürümü keser; otomatik veri taşıması YOKTUR ve yazılmamıştır (dönüşüm tasarım kararı
gerektirir, M10'daki "çok elemanlı c_a array'i" ile aynı gerekçe).

**Böyle bir belge nasıl düzeltilir:**

1. Aynı canonical `c_a`'yı taşıyan node'ları bul (validator hatası ikisinin de anahtarını
   yazar: `'<prev>' ve '<key>' AYNI c_a'yı taşıyor`).
2. **İkisini TEK node'a indir.** Hangisi kalacaksa (tercihen akışta önce geleni) onun
   anahtarı korunur; diğeri silinir.
3. Silinen node'a giden tüm referansları kalan node'a bağla: `transitions[].from`,
   `wft.node`, `wft.conditions[].node`, `wft.targets[].node`, `escalation[].wft.node`,
   `call.wft.node`, `start[].from`.
4. **Farkı aksiyonların `when`i ile ver.** İki node "aynı kişi, ardışık iki adım" içindi;
   artık ikisi tek node'un iki aksiyonudur ve ayrım `$wfah` üzerinden yazılır — ör.
   ikinci adımın transition'ına `when: count($wfah, #.action == "incele") >= 1`, birincinin
   transition'ına `when: count($wfah, #.action == "incele") == 0`. (Dizi fonksiyonları İKİ
   argümanlıdır — WOR-84.)
5. İki node'un `escalation` / `claim_timeout` / `attachments` / `listable` kayıtları
   birleşir; çakışan tanımlar için karar tasarımcınındır (mekanik birleştirme YOK).

**Düzeltilemeyen tek senaryo:** paralel kolda **aynı havuzdan iki kol**. Kol kimliği node
anahtarı olduğundan (WOR-73) aynı havuza bakan iki eşzamanlı kol iki node ister — bu artık
çizilemez. Bilinçli ve GEÇİCİ kısıt; gerekçesi ve ileride kaldırılma yolu `decisions.md`
2026-08-14 maddesindedir.

## M19. Adlandırılmış tip `format` + RUNTIME tip denetimi (2026-08-19, **KIRICI**)

Karar kayıtları: `decisions.md` → "Adlandırılmış tip: `format` → `$defs`" ve
"Runtime tip denetimi — engine bilir kişi".

### Ne değişti

1. **`$ref` KALDIRILDI (okuyucusu da yok).** Context şemasında bir alanı yeniden
   kullanılabilir bir tanıma bağlamanın tek yolu artık `format`tır:

   ```diff
    "context": {
      "$defs": { "Tarih": { "type": "string", "pattern": "^[0-9]{14}$" } },
      "properties": {
   -    "basvuru_tarihi": { "$ref": "#/$defs/Tarih" }
   +    "basvuru_tarihi": { "format": "Tarih" }
      }
    }
   ```

   `$ref` şemadan çıkarıldı (`contextSchemaNode`), motor/editör/portal çözücüleri onu
   TANIMAZ, validator `context_ref_removed` ile reddeder.

2. **`format` artık standart JSON Schema formatı DEĞİL**, `$defs` tanım adıdır.
   `format: "date-time"` = "benim `$defs.date-time` tanımım"; tanımlı değilse
   `context_format_unknown` ile yayın durur. Standart adların kütüphaneye gömülü anlamı
   KULLANILMAZ — kural belgede durur, motor onu çalışma anında da uygular.

3. **`format` yanında tip kuralı yazılamaz** (`context_format_with_type`): `type`, `enum`,
   `pattern`, sayı/uzunluk sınırları, `items`, `properties`, `x-wf-kind`. Tip tanımın
   İÇİNDEDİR; kullanım yerinde yalnız `title`/`description`/`x-visibility` ezilebilir.

4. **Yayınlanan belge DRY**: editör artık `properties`'i inline ETMEZ; `format` + `$defs`
   birlikte yayınlanır ve üç tüketici (motor `v22::ctx_types`, editör `utils/contextDefs`,
   portal `lib/contextTypes`) aynı çözümü yapar.

5. **Çalışma anında TİP denetimi** (üç kapı):
   | Kapı | Nerede | Hata |
   |---|---|---|
   | A — girdi | `validate_action_input` (start + apply) | `422` `input.type_mismatch` + `items[]` |
   | B — ctx yazma | `pipeline::guard_written_ctx` (bu geçişin YAZDIĞI yollar) | `422` `ctx.type_mismatch` + `items[]` |
   | C — bozuk ctx | `executor::guard_stored_ctx` (apply · claim · escalation fire) | `422` `ctx.type_mismatch`; okuma SERBEST + `WfeView.ctx_violations` |

   Zorlanan kurallar: `type` · `enum` · `const` · sayı sınırları · `minLength`/`maxLength`/
   `pattern` · dizi kuralları · iç içe `properties`. **`null` HER tipte geçerlidir**
   (WOR-70b gönderilmeyen `optional` ctx'e `null` yazar).

### Kimi etkiler

- **Elle JSON yazan / API'ye doğrudan istek atan istemciler:** gevşek tipli değer
  (`"1000"` yerine `1000` bekleniyorsa) artık `422` alır. Alan bazında ne beklendiği
  `items[]`te yazar.
- **`$ref` kullanan belgeler:** ölçüldü — yayınlanmış hiçbir belgede `$ref`/`format`
  kullanımı YOK (`ctx_type_report`, 2026-08-19: 25 WFE / 0 `$ref`). Elde kalan bir
  taslak varsa `$ref` → `format` elle çevrilir.
- **`format: "date-time"` gibi standart ad taşıyan belgeler:** editör import'unda o değer
  DÜŞER (motorda zaten etkisizdi); `data-url` → `x-wf-document: true` çevrilir. Kesin
  kısıt isteniyorsa `$defs` tanımı + `pattern` yazılır.

### Düzeltme reçetesi

1. `context.$defs` altına tipleri tanımla (`type` + `pattern`/sınır/`enum` ile).
2. Alanlarda `$ref`i `format` ile değiştir, alanın üzerindeki tip kurallarını SİL
   (tanıma taşı).
3. `ctx_type_report` ile sahayı ölç: ihlal varsa akışın yeni sürümünü yayınla ya da
   bağlamı düzelt (motor bozuk bağlamda EYLEM kabul etmez, görüntülemeye izin verir).
4. Yanlış tip gönderen entegrasyonları düzelt; ret gövdesindeki `items[]` hangi yolun ne
   beklediğini söyler.

### Geriye uyum politikası (bu maddenin gerekçesi)

"Wire formatı değişince eski şeklin okuyucusu kalır" kuralı **production'dan SONRA**
geçerlidir. Ürün henüz production'da değil; mevcut tüm WFD/WFE'ler test verisi ve
production öncesi sıfırlanacak — bu yüzden `$ref` okuyucusu ve GLB `__gt__` anahtar
ailesi okuyucusu (`legacyGlobalAction.ts`) SİLİNDİ. Kalan geriye uyum okuyucularının
envanteri: `docs/2026-08-19-legacy-okuyucu-temizligi.md`.

## Editor (React Flow) Eşlemesi

```text
nodes katalogu     -> humanPool node (basligi = label, alt yazi = slug)
terminals[]        -> terminal node
start[]            -> start node
transitions[]      -> edge (conditional wft = when-label'li coklu edge)
escalation         -> kesikli edge ("after" label)
```

Sanal node üretme katmanı silinir; export ajv (draft 2020-12) ile `schema.json`'a valide edilir; `ui_*` alanları export'ta yoktur. c_a editörü tek-kural formudur: orgu seçici + rol çoklu-seçim + kişi çoklu-seçim.

## Doğrulama Sırası (CI kapıları)

1. JSON Schema validation (golden fixture geçmeli).
2. Custom validator: cross-ref + c_a tekilliği (`duplicate_c_a`) + context path + ZEN parse.
3. Graf analizi: BFS reachability (escalation kenarları DAHİL) + çıkışsız node.
4. Rust kabul testi: `reference-types.rs` fixture'ı parse eder, slug'ları doğrular.
