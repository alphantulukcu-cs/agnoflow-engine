# WFD Custom Validator & Runtime Semantics — Named Nodes Model v2.2

`wfd_schema_v2_2.json`'ın yakalayamadığı kuralları tanımlar; önceki tüm sürümlerin yerini alır. Referans implementasyon: `wfd_types_v2_2.rs` (slug + matcher + kabul testleri).

---

## 1. Cross-Reference Validation

v2.1 ile aynı: `from`→nodes, `action`→actions, `trigger[].use`→autoexec, tüm `wft.node/terminal` (conditions, default, escalation dahil)→nodes/terminals. Unique: node key'leri, `start[].id`, `transitions[].id`, `terminals[].id`, action/autoexec key'leri; node ve terminal id'leri global namespace'te çakışmaz.

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

## 7. Transition Runtime Pipeline

```text
1. WFE assigned mi? Actor owner mi? Degilse ACT reddedilir.
   (Unassigned'da once claim; claim = current node c_a match'i, §3 semantigi.)
2. transition.c_a varsa: owner bu EK kurala da match etmeli (§3).
3. current_node ∈ transition.from? Degilse aday degildir.
4. Adaylar array sirasiyla; when'i true olan ILK transition secilir.
5. Action input validate edilir.
6. transition.wfes_effects STAGED.
7. trigger[] sirayla: when -> execute (timeout_seconds) -> fail'de retry
   (bekleme = interval * backoff^attempt, max_delay ile kirpilir)
   -> catch match: effects STAGED, handled, devam -> yoksa required davranisi.
   Basarili autoexec: wfes_effects STAGED.
8. transition.wft staged DynCtx uzerinden evaluate edilir.
9. COMMIT (atomik): diff'ler + WFAH + node degisimi + assignment reset (yeni node'a UNASSIGNED).
Unhandled fail'de hicbir sey commit edilmez.
```

## 8. Escalation / Timeout Runtime

v2.1 ile aynı: escalation zamanlayıcısı node-giriş anından başlar (WFAH'tan türetilir), sıralı adımlar birer kez tetiklenir, assigned WFE'de de çalışır, taşımada assignment temizlenir, WFAH'a system actor yazılır, adım tek transaction'dır. `autoexec.timeout_seconds` aşımı `WFD.Timeout`; root `timeout` aşımı engine-defined fail + WFAH kaydı.

## 9. WFD Yükleme

- Tanınmayan `wfd_version` = yükleme reddi. Root'ta bilinmeyen alan yasak.
- Çalışan WFE'ler başladıkları WFD (id+version)'a sabitlenir; kural değişikliği yeni WFD versiyonu doğurur — node slug'ları bu sayede WFE ömrü boyunca kararlıdır. Versiyon-aşırı metrikler `label` üzerinden agregat edilmelidir.

## Symmetric start (v2.2)

`start[]` artık `transitions[]` ile simetriktir: `{ id, from, action:"start", wfes_effects?, trigger?, wft }`. `c_a` startRule'dan kaldırılmıştır; `start[].from` ile referans edilen `nodes` girdisinin `c_a`'sı taşır. Start-node kimliği türetilmiştir — node'un kendisinde `kind` alanı YOKTUR; bir node, sadece bir `start[].from` tarafından referans edilerek start node olur.

| # | Kural |
|---|------|
| V1 | `start[].from`, `nodes` içinde var olan bir node'a referans vermelidir. |
| V2 | Herhangi bir `start[].from` tarafından referans edilen node, HİÇBİR transition veya start'ın `wft` hedefi OLMAMALIDIR — start havuzları yalnızca giriş noktasıdır, yeniden girilemez. |
| V3 | Bir start node `escalation` TAŞIYAMAZ (orada bekleyen yoktur). |
| V4 | `start[].action`, rezerve sabit `"start"` olmalıdır. |
| V5 | Rezerve `"start"` aksiyonu `actions{}` içinde TANIMLANMAMALIDIR. |
| V6 | En az bir `start` girdisi olmalıdır (mevcut `minItems: 1`). |

**Runtime resolution:** Actor rezerve `start` aksiyonunu çağırır → her aday start node'un `c_a`'sına karşı eşleştirilir → eşleşen node efektif `from` olur → o start rule'ın `wfes_effects`/`trigger`'ı çalışır → WFE `wft`'e iner. Transition seçimiyle (`from` + `action`) birebir aynı mekanik.

Lifecycle notu: transition'larda `node.c_a` WFE'yi o an elinde tutan owner'dır; bir start node'da `c_a` kimin *başlatabileceğidir*. Aynı eşleştirme mekaniği, farklı lifecycle anlamı (henüz WFE yok).
