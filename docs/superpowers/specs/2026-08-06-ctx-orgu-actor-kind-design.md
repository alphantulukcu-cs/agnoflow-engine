# Context alan kind'ları (`orgu` / `actor`) + C_A'nın context'ten beslenmesi

**Tarih:** 2026-08-06
**Kapsam:** `agnoflow-engine` (spec + `wfe-core`) · `agnoflow-frontend` (Context Studio + C_A editörleri)
**İlgili görev:** T‑D1 ("ORGU context'ten beslenebilsin") — `gorevlendirme.md`

---

## 1. Sorun

`c_orgu`'nun ctx-anchor formu **motorda zaten var** ama kullanılamıyor:

```json
{ "c_orgu": { "from": "$ctx.musteri_sube", "traverse": "self.parent" }, "c_r": ["mudur"] }
```

`resolver::resolve_c_orgu` bu formu çözüyor (`crates/wfe-core/src/v22/resolver.rs`), `schema.json`
`cOrgu.from`'u `oneOf [string, wfahAnchor]` olarak tanımlıyor. Üç boşluk var:

1. **Editörde yazılamıyor.** Ana C_A editörü `OrgCaModal`'ın `FromMode`'u `'none' | 'wfah'`;
   `ctx` seçeneği yok. Dahası ctx-anchor'lı bir kural o modalda açılıp kaydedilirse
   `initFromMode` onu `'none'` sanıp **düz stringe çeviriyor** — sessiz veri kaybı.
   (Kompakt `CaRuleEditor` — `transition.c_a`, call node c_a, `listable` yüzeylerinde
   kullanılan form — ctx'i destekliyor ama `wfah`'ı desteklemiyor; simetrik boşluk,
   bu turda kapsam dışı.)
2. **Aday alanı bilinemiyor.** Editör "hangi context alanı bir ORGU tutar" sorusunu bugün
   yalnızca `wfes_effects.set: {alan: "$actor"}` yazan upstream adımlardan çıkarıyor
   (`cOrguUtils.getUpstreamActorFields().ctx`). Bir REST autoexec'in ctx'e yazdığı
   `musteri.sube_id` bu listeye giremiyor.
3. **Anchor çözülemezse yanlış kişiyi yetkilendiriyor.** `resolve_anchor(...)?.unwrap_or(default_anchor)`
   aktörün **kendi birimine** düşüyor — "ctx'teki şubenin müdürü" kuralı alan boşsa "aktörün
   kendi şubesinin müdürü"ne dönüşüyor. Bu davranış `resolver.rs`'in inline test modülünde
   `missing_anchor_falls_back_to_default` ile sabitlenmiş; ancak spec'in hiçbir yerinde
   (terminology / runtime-semantics / decisions) yazmıyor ve resolver'ın ilk yazıldığı
   commit'te (6e1d9dc) testiyle birlikte gelmiş — spec'ten türeyen bir sözleşme değil,
   uygulama tercihi. Belge kapsamı ise gerçekten test edilmemiş: hiçbir fixture/örnek
   ctx-anchor kullanmıyor.

## 2. Karar

Context şema düğümüne **`x-wf-kind`** uzantısı girer ve **spec seviyesinde** tanımlanır —
yalnız editörün bildiği bir kavram değil. Motor onu okur, validator onunla kural koşar,
editör aynı cevabı önden verir.

| kind | kanonik şekil | besleyebildiği kanal |
|---|---|---|
| `orgu` | `{orgu_id, name?}` | `c_orgu.from` |
| `actor` | `{user_id, orgu_id, role?, name?}` | `c_u` **ve** `c_orgu.from` |

`actor` kind'ı `orgu`'yu **kapsar**: `wfes_effects.set: {x: "$actor"}` alana
`{orgu_id, user_id, role}` yazıyor ve `resolver::extract_orgu_uuid` obje içinde
`orgu`/`orgu_id` anahtarını buluyor. Yani tek alan iki kanalı besler. İkisi simetrik
kavramlar değil; `orgu` kind'lı alan (ör. bir REST'in döndürdüğü şube) yalnız `c_orgu`'yu
besler, çünkü içinde kişi yoktur.

### Neden `x-wf-kind`, neden `$defs`/`format` değil

- **`x-` uzantısı bu spec'te yerleşik bir desen.** `x-visibility` `schema.json`'da tanımlı ve
  motor `v22/visibility.rs`'te okuyor. WOR‑71'in `x-wf-readonly`'yi kaldırma gerekçesi burada
  geçmez: o bilgi WFD'den türetilebiliyordu, "bu alan bir ORGU tutar" türetilemez.
- **`$defs` arkasına saklanamaz.** Motor `$ref`'i çözmüyor — `expr_types.rs`'te hiç `$ref`
  işlemesi yok, validator `$ref`'i "opak, yaprak sayılır" diye geçiyor. Kind alanın kendi
  düğümünde durursa hiçbir çözümleme gerekmeden görünür.
- **`format` yanlış eksen.** JSON Schema'da `format` metin biçimi içindir; obje kimliği için
  anlamsal olarak yanlış ve şekli anlatmaz.

> **Adlandırma düzeltmesi (aynı gün).** Kind ilk turda `user` diye adlandırılmıştı; `terminology.md`
> §USER/ROLE/ACTOR'a göre **User (U)** koltuksuz kişidir, `(ORGU,(U,R))` üçlüsünün adı ise **Actor**'dür
> — ve alana yazılan şey (`$actor`) tam olarak o üçlüdür. Kind `actor` olarak düzeltildi. Salt kişi tutan
> bir alan için ayrı bir `user` kind'ı gerekirse sonra eklenir; o yalnız `c_u`'yu besler.

## 3. Faz 1 — `orgu` kind

### 3.1 Spec

- `schema.json` → `contextSchemaNode.properties`'e
  `"x-wf-kind": {"type":"string","enum":["orgu","actor"]}`. **Enum ikisini birden tanımlar**
  ki Faz 2 şema değişikliği istemesin; Faz 1'de yalnız `orgu` tüketilir.
- `decisions.md`'ye karar maddesi (kavram + kurallar + §3.3 davranış değişikliği).

### 3.2 `$defs` çözümlemesi (zorunlu ön koşul)

Editör `$ref`'i çözüyor, motor çözmüyor. Bu asimetri kalırsa
`musteri: {"$ref":"#/$defs/Musteri"}` altındaki `orgu` kind'lı alanı motor göremez ve
**meşru bir belgeyi reddeder**. Validator'ın context-yolu yürüyüşüne sınırlı çözümleme
girer:

- yalnız `#/$defs/<Ad>` biçimi (editörün `REF_PREFIX` ile ürettiği tek biçim),
- döngü bekçili (`$defs.A → $defs.B → $defs.A` sonsuz dönmez),
- çözülemeyen `$ref` bugünkü gibi opak yaprak kalır.

### 3.3 Motor davranış düzeltmesi

`resolver.rs`'te `resolve_anchor(from, ...)?.unwrap_or(default_anchor)` → anchor
çözülemezse **boş küme** döner, hiç kimse eşleşmez. `COrgu::Selector` yolundaki
`default_anchor` (`self` = aktörün birimi) **aynen kalır** — orada doğru davranıştır.

Bu bir mantık hatasının düzeltilmesidir, tercih değişikliği değil. `traverse: "self"` olan
bir kuralda fallback ORGU kapısını **etkisiz** kılıyordu:

1. `{from: "$ctx.initiated_by", traverse: "self"}` = "talebin açıldığı birimin müdürü"
2. Alan o an yazılmamışsa anchor `default_anchor`'a düşer; matcher onu `actor.orgu_id`
   olarak geçer (`matcher.rs`)
3. `resolve(actor.orgu, "self")` = `{aktörün birimi}`
4. Kapı `resolved.any(|u| u.orgu_id == actor.orgu_id)` → **daima doğru**

Yani kural "o rolü taşıyan **herkes**"e dönüşüyordu; hata yok, log yok, WFAH'ta meşru
görünen bir ACT. Hatanın kaynağı, `Selector` dalı için doğru olan varsayılanın anlamı
tersine çeviren `Anchor` dalına kopyalanmasıydı. Aynı genişleme `x-visibility` için de
geçerliydi (`visibility.rs` de `actor.orgu_id`'yi default anchor geçiyor) — maskeli alan
görünür hale gelebiliyordu.

Boş küme = node görünür biçimde durur, `claim_timeout`/`escalation` devreye girer. Gürültülü
tıkanma, sessiz yetki genişlemesine yeğdir.

Geriye uyumluluk maliyeti sıfır: repoda ctx-anchor kullanan hiçbir belge yok (§1.3).

### 3.4 Validator kuralları

Kural `c_orgu.from`'un **string** olduğu her durumda koşar — `from` objeyse o `wfahAnchor`'dır,
ayrı yol. `anchor_from_ctx` `$ctx.` önekini `strip_prefix(...).unwrap_or(path)` ile soyduğu için
önek **opsiyoneldir**; validator da aynı normalizasyonu yapar (editör daima `$ctx.` yazar).

| kural | seviye | koşul |
|---|---|---|
| `c_orgu_anchor_unknown_field` | hata | yol context şemasında yok |
| `c_orgu_anchor_not_orgu_kind` | hata | yol `orgu`/`actor` kind'lı bir düğüme ya da bunlardan birinin `orgu_id`/`orgu` çocuğuna çözülmüyor |
| `c_orgu_anchor_kind_unverifiable` | **uyarı** | şema o derinliği kısıtlamıyor → kind doğrulanamıyor |

Üçüncü kural, uygulama sırasında ortaya çıkan bir boşluğu kapatıyor: `initiated_by: {"type":"object"}`
gibi property'siz bir alanın **alt yolu** (`$ctx.initiated_by.orgu`) şemada çözülemez. Bu biçim
meşru ve yaygın olduğu için hata olamaz; ama sessiz geçerse kuralı tümüyle atlatmanın yolu olur.
Uyarı, tasarımcıyı alanı `orgu` tipiyle bildirmeye yönlendirir.

Kural **tüm `c_orgu` yüzeylerinde** koşar — beşi:

1. `NodeDef.c_a`
2. `NodeDef.reassign` (Madde 7)
3. `Transition.c_a` (editördeki "ek yetki kuralı")
4. `ListableRule.c_a`
5. context şemasının içindeki `x-visibility.c_orgu`

Toplama tek bir gezinti fonksiyonunda toplanır (`validator::env_references`'ın deseni) —
yeni bir C_A taşıyıcısı eklendiğinde tek yerde güncellenir.

### 3.5 Test

`resolver.rs`'in inline modülünde 5 test var (selector, ctx-obje, wfah occurrence first/last,
eksik anchor) — belge seviyesinde ise hiç kapsam yok.

- `resolver` birim testleri: ham‑UUID ve `_id` fallback'i (kapsanmıyor), bozuk UUID → hata
  (kapsanmıyor). **`missing_anchor_falls_back_to_default` testi §3.3 gereği tersine çevrilir**
  (`missing_anchor_resolves_to_empty_set`).
- `validator` testleri: iki kural × beş yüzey, `$defs` arkasındaki alan, döngülü `$ref`.
- **Golden fixture DEĞİŞMEZ** (CLAUDE.md). Ctx-anchor'lı **yeni** bir örnek fixture
  eklenir; `crates/wfe-core/tests/fixtures/` kopyası senkron tutulur.

### 3.6 Editör

- **Context Studio tip açılırı** (`JsonSchemaEditorModal`): `string/number/boolean/object/array`
  + `$ref` yanına **`orgu`**. Seçilince alan
  `{"type":"object","x-wf-kind":"orgu","properties":{"orgu_id":{"type":"string"},"name":{"type":"string"}}}`
  olarak yazılır — kullanıcı şekli elle kurmaz.
- **`OrgCaModal`**: `FromMode`'a `'ctx'`; aday listesi kind'a göre filtrelenmiş context
  yolları (`getUpstreamActorFields`'ın `ctx` yarısı `$actor` sezgisinden kind temeline
  geçer); `wfah` modundakiyle aynı canlı `from:/traverse:` önizlemesi; kaydetme
  `{from:"$ctx.<yol>", traverse}` üretir.
- **İki sessiz veri kaybı kapanır**:
  - `OrgCaModal.initFromMode` string `from`'u tanır (bugün `'none'` sanıp düz stringe çevirir).
  - `CaRuleEditor`'ın "Mutlak" sekmesindeki metin kutusu wfah formunu `wfah:<ad>::<traverse>`
    diye gösteriyor; düzenlenirse `updateCOrgu` `from`'u `"wfah:<ad>"` yazıyor ve motor bunu
    ctx yolu sanıyor. Kutu wfah formunda **salt-okunur** olur.

### 3.7 Faz 1 kapsamı DIŞI

`actor` kind'ının `c_u` tarafından tüketilmesi · dinamik `c_u` · `CaRuleEditor`'a `wfah` anchor'ı ekleme.

## 4. Faz 2 — dinamik `c_u`

### 4.1 Gerekçe

`c_u` bugün düz `Vec<String>` (UUID **veya** kullanıcı adı, `matcher.rs` 3. adım). Aday
aktör havuzunu kişiyle daraltmak ileride birinci sınıf bir yetenek olacak, o yüzden `c_u`
adım adım büyüyecek bir alan. Büyüyecek bir alana sihirli önek konvansiyonu
(`"$ctx.x.user_id"` düz string olarak) koymak her yeni yeteneği şemadan denetlenemez ve
tip sistemine görünmez kılar — bu projenin defalarca temizlediği drift (C_A array'i,
`terminal_when`, `x-wf-readonly`).

### 4.2 Şekil — `COrgu`'nun aynası

```rust
#[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum CuItem {
    Literal(String),          // "ahmet.yilmaz" | "<uuid>"
    Ref { from: String },     // { "from": "$ctx.talep_sahibi.user_id" }
}
```

`COrgu` de `#[serde(untagged)]` bir birleşim (`Selector(String) | Anchor{from, traverse}`);
`c_u` aynı "literal ya da anchor" ikiliğini ve aynı `from` anahtar adını kullanır. Düz
string'ler **aynen** çalışır → migration yok, fixture değişmez.

### 4.3 Node key maliyeti: sıfır

`sanitize()` `$` ve `.` karakterlerini atıyor, dolayısıyla `Literal("$ctx.x.user_id")` ile
`Ref{from:"$ctx.x.user_id"}` **aynı** slug'ı üretir (`ctx_x_user_id`) — §2a node key
değişmezi zedelenmez. `slug()`: `Literal(s) => sanitize(s)`, `Ref{from} => sanitize(from)`.
`canonical()` `Debug` türetiminden variant'ı zaten ayırır; sıralama için `CuItem`'a `Ord`
türetilir.

### 4.4 Matcher

`authorize`'ın 3. adımı `CuItem::Ref`'i ctx'ten çözer, sonra bugünkü UUID/ad
karşılaştırmasını uygular. Değer çıkarımı `extract_orgu_uuid`'in simetriği olur
(`extract_user_ident`): ham string, ya da obje içinde `user_id`/`actor`. Böylece
`{from:"$ctx.talep_sahibi"}` (son ek olmadan) da çalışır.

**Çözülemeyen `Ref` → o kanal eşleşmez** (hata değil). `$ctx`'in "eksik = null" sözleşmesiyle
tutarlı; `$env`'in "eksik = hata" kuralı burada geçmez çünkü null bir domain üretmiyor,
yalnızca aday havuzunu daraltıyor.

### 4.5 ORGU kapısı — tasarımı belirleyen kısıt

`matcher::authorize` ORGU kanalını **erken çıkışla** uyguluyor: `actor.orgu ∈ resolve(c_orgu)`
sağlanmazsa `c_u`'ya hiç bakılmaz. Yani "talebi açan kişi onaylasın" tek başına `c_u` ile
kurulamaz — kişinin birimi `c_orgu`'nun çözdüğü kümede de olmalı. Doğru kullanım iki kanalı
**aynı alandan** beslemektir:

```json
{ "c_orgu": { "from": "$ctx.talep_sahibi", "traverse": "self" },
  "c_u":    [ { "from": "$ctx.talep_sahibi.user_id" } ] }
```

Bu kısıt spec'te ve editör yardım metninde açıkça yazılır. `matcher`'ın erken çıkışı
**değişmez** — C_A'nın `resolved(c_orgu) AND (rol OR c_u)` değişmezi korunur.

### 4.6 Portal havuzu

Havuz listelemesi denormalize `current_c_a` jsonb cache'i üzerinden SQL'de yapılıyor ve
cache **çözülmüş** adayları tutuyor (`routes/portal/pool.rs`). Dinamik `c_u`, node'a girişte
(ctx bilinirken) çözülüp cache'e yazılır → **havuz sorgusu değişmez.**

### 4.7 Validator (Faz 2)

| kural | koşul |
|---|---|
| `c_u_literal_dollar_prefix` | `Literal` öğesi `$` ile başlıyor (yazım hatası kullanıcı adı sanılmasın) |
| `c_u_ref_not_actor_kind` | `Ref.from` bir `actor` kind'lı alana ya da onun `user_id` çocuğuna çözülmüyor |

### 4.8 Editör (Faz 2)

`c_u` satırlarına "context'ten kişi" seçeneği
(`CaRuleEditor` ve `OrgCaModal`'ın rol/kişi bölümü).

### 4.9 Faz 2 kapsamı DIŞI

`c_u`'da `$wfah` anchor'ı — `wfes_effects` `$actor`'ü ctx'e yazabildiği için ihtiyaç yok.

## 5. Kabul kriterleri

**Faz 1**
1. `orgu` kind'lı context alanı Context Studio'dan tanımlanabiliyor.
2. `OrgCaModal`'da o alan seçilerek `c_orgu.from` kurulabiliyor; kaydet→aç→kaydet
   döngüsünde form korunuyor.
3. Kind'lı olmayan bir yolu işaret eden `c_orgu.from` publish'te **hata** veriyor; beş
   yüzeyin hepsinde.
4. Anchor çözülemediğinde hiç kimse yetkilenmiyor (aktörün kendi birimine düşmüyor).
5. `cargo test --workspace` geçiyor; golden fixture değişmemiş.

**Faz 2**
6. `c_u: [{from:"$ctx.<user alanı>.user_id"}]` yazılabiliyor, matcher onu çözüyor, havuz
   listelemesi kişiyi görüyor.
7. `Literal` öğede `$` öneki ve `actor` kind'lı olmayan `Ref` publish'te hata veriyor.
8. Eski düz-string `c_u` belgeleri değişmeden çalışıyor; node key'leri aynı kalıyor.
