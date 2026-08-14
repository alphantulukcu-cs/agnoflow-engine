# Rapor: Paralel kol izolasyonu — görünürlük ve veri

**Tarih:** 2026-08-14
**Kapsam:** `agnoflow-backend` (`wfe-core` runtime + `wfe` adapter + `server` görünürlük/havuz) · `agnoflow-frontend` (editör paralel yüzeyleri)
**Durum:** SADECE ARAŞTIRMA. Hiçbir kod değişmedi, hiçbir migration önerilmedi. Bu belge bir **sonraki konuşmanın zemini**dir; karar VERMEZ.
**Tetikleyen soru:** kullanıcı, paralel kolların birbirinin WFE'sini ve birbirinin ctx/wfah değişikliklerini görmemesini istedi; "bu kısım çok karmaşık" diyerek uygulamayı ertelledi ve rapor istedi.

---

## Yönetici özeti (10 satır)

1. **DynCtx bugün WFE SEVİYESİNDE TEK'tir.** `Wfes.dynctx` tek alan (`ports.rs:65`), `BranchState`in ctx alanı YOK (`ports.rs:33-51`), DB'de `wf.wfe_dynctx` yalnız `wfe_id`+`seq` ile anahtarlı (`20260521000001_initial.sql:33-41`) — kol kolonu YOK.
2. **Bir kolun `wfes_effects` yazması diğer kolun `when` ifadesinden GÖRÜNÜR.** `apply_parallel` ctx'i WFE satırından okur (`pipeline.rs:617`), üzerine yazar (`:709`), TAM SNAPSHOT olarak commit eder (`:799` → `wfe_adapter.rs:511`).
3. **WFAH da WFE seviyesinde TEK listedir.** `wf.wfah`'ta kol kolonu YOK (yalnız `from_node`/`to_node`, ikisi de nullable); `$wfah` izdüşümü hiçbir yerde süzülmez (`eval.rs:117-120`).
4. Sonuç: `count($wfah, ...)`, `$prev`, `$first` paralel modda **tüm kolların + sistem marker'larının** birleşik geçmişini görür. `$prev` çoğu zaman bir insan aksiyonu değil, `_branch_arrived` marker'ıdır.
5. **Join'de "birleştirme" diye bir adım YOK** — birleştirilecek iki şey hiç var olmadı. Join yalnız kol satırlarını kapatıp `current_node`u join hedefine taşır.
6. **Aynı ctx alanına iki kol yazarsa: son COMMIT eden kazanır, çakışma tespiti YOK.** Gerçek eşzamanlılık `UNIQUE (wfe_id, seq)` ile serileştirilir → `StaleRevision` → executor reload + yeniden koşma (`MAX_ATTEMPTS = 3`).
7. **Görünürlük bugün de WFE seviyesindedir ve bu BİLİNÇLİ yazılmıştır.** `visibility::sql`in kol EXISTS'i "herhangi bir aktif kol eşleşiyorsa WFE görünür" der (`visibility.rs:95-109`); havuzun kol sorgusu bunu WFE-seviyesi süzgeç olarak kullanır ve yorumu açıkça "WFE görünüyorsa AKTİF KOLLARININ HEPSİ listelenir" (`pool.rs:177-180`).
8. Yani kullanıcının tarif ettiği iki şey bugün **ikisi de yok**: kollar birbirinin satırını görür, birbirinin verisini görür.
9. Kullanıcının talebi **iki AYRI iştir** (B1 görünürlük, B2 veri) ve B2 kıyasla ~5-10 kat daha derindir: kol-yerel ctx + kol-yerel WFAH görünümü, `$wfah` sayım semantiğini ve `expr_types` tip çıkarımını doğrudan etkiler.
10. **Tavsiye:** B1'i (kol-bazlı satır süzgeci) ayrı ve küçük bir iş olarak değerlendir, B2'yi ŞİMDİ YAPMA — yerine mevcut semantiği belgele ve `docs/spec/decisions.md`'ye bilinçli karar olarak yaz (gerekçe D bölümünün sonunda).

---

## 0. Kullanıcının ifadesi (aynen korunmuştur)

> "Farklı kollar birbirinin wfe'sini listable'da yoklarsa göremezler. Yani görebilecekleri editörde listable içinde spesifik olarak söylenmediyse birbirlerinin wfe'lerini göremezler. Yani paralel akışın 3 kola böldüğünü düşünürsek, hepsi önce aynı wfe'yi görüyor tamam. Ama kollar kendi içinde uzuyorsa ve context'e ve wfah'da değişiklik yapılıyor ama kollar bu değişiklikleri sadece kendi kol yolunda görebilir. 2.'nin yaptığı değişikliği 1. koldaki göremez. Sonra join'lenince tekrar tek'e iner."

---

## A. Bugün ne oluyor? (ölçüm)

### A1 — `DynCtx` paralel modda: WFE seviyesinde TEK

**Runtime tipi.** `Wfes` tek bir ctx taşır; kol durumu ctx taşımaz:

```rust
// crates/wfe-core/src/v22/ports.rs:55-97
pub struct Wfes {
    ...
    pub dynctx: DynCtx,          // ← :65  TEK
    pub wfah: Wfah,              // ← :66  TEK
    ...
    pub branches: Vec<BranchState>,   // ← :80
    pub join_target: Option<WftTarget>,
    pub join_rule: JoinRule,
}

// crates/wfe-core/src/v22/ports.rs:32-51 — BranchState'in TAM alan listesi
pub struct BranchState {
    pub branch_node: String,   // kolun ŞU AN beklediği node (hareket ettikçe değişir)
    pub entry_node: String,    // WOR-73: kolun DEĞİŞMEZ kimliği = fork'taki giriş node'u
    pub status: BranchStatus,  // Active | Arrived | Cancelled
    pub claimed_by: Option<Uuid>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub entered_at: DateTime<Utc>,
}
```

`BranchState`te ctx yok, geçmiş yok, ctx'e/geçmişe işaret eden bir seq/versiyon alanı da yok.

**Şema.** DynCtx `wf.wfe` üzerinde bir kolon DEĞİL, ayrı append-only tablo:

```sql
-- migrations/wf/20260521000001_initial.sql:33-41
CREATE TABLE wf.wfe_dynctx (
    dynctx_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    ctx        jsonb       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)          -- ← anahtar (wfe_id, seq); BRANCH YOK
);
```

`wf.wfe_branch`ın TAM kolon kümesi — `branch_id, wfe_id, branch_node, status, claimed_by, claimed_at, entered_at, created_at, updated_at, entry_node, c_a, view_c_a` (`20260717000006_wfe_branch.sql:20-34` + `20260731000002_join_expr.sql:29` + `20260813000001_visibility_grants.sql:71-72` + `20260813000004_node_listable.sql:53-54`). Üç `jsonb` kolonu var (`claimed_by`, `c_a`, `view_c_a`) ve **üçü de aktör listesidir**; ctx/context/dynctx adında kolon YOKTUR.

**Okuma.** Kol hangisi olursa olsun aynı satır okunur:

```rust
// crates/wfe/src/repo/dynctx.rs:6-15
"SELECT ctx FROM wf.wfe_dynctx WHERE wfe_id = $1 ORDER BY seq DESC LIMIT 1"
```

**Kolda aksiyon uygulanırken.** `apply_parallel` ctx'i WFE satırından alır, `when` ifadelerini o ctx üzerinde koşar, effects'i o ctx'in üzerine yazar:

```rust
// crates/wfe-core/src/v22/pipeline.rs:617
let ctx = wfes.dynctx.as_value().clone();      // WFE-seviyesi ctx
...
// :630-636 — KOLUN when'i, WFE-seviyesi ctx ve WFE-seviyesi wfah ile
let env = EvalEnv::new(&ctx).with_wfah(&wfes.wfah).with_node(Some(&b.branch_node))...
...
// :691
let mut staged = ctx.clone();
// :698-710 — kolun wfes_effects'i AYNI ctx'in üzerine
staged = apply_effects(&staged, effects, &env)?;
...
// :799
new_dynctx: final_ctx,                          // TAM SNAPSHOT
```

`apply_effects` ctx'i bütün olarak klonlar ve yol yol üzerine yazar (`effects.rs:47-61`) — yani her commit **tüm ctx'in yeni bir sürümüdür**, delta değil.

**Yazma.** Commit tek `INSERT`, seq = o commit'in son WFAH seq'i:

```rust
// crates/wfe/src/wfe_adapter.rs:506-518
let dynctx_seq = commit.wfah_entries.last().map(|e| e.seq as i32).unwrap_or(1);
sqlx::query("INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, $2, $3)")
    ... .map_err(insert_err)?;     // 23505 → Conflict(StaleRevision)
```

> **Sorunun cevabı:** Bir kolda `wfes_effects` ile yazılan bir ctx alanı diğer kolun `when` ifadesinden **GÖRÜNÜR**. Aynı satır, aynı jsonb, tek zaman çizgisi. Bunu doğrulayan bir birim testi YOK (bkz. ÖLÇÜLEMEDİ), ama kod yolunda ayrıştırma imkânı da yok: kol başına ikinci bir ctx kaynağı hiç mevcut değil.

### A2 — `WFAH` paralel modda: WFE seviyesinde TEK liste

**Şema.** Kol kolonu yok:

```sql
-- migrations/wf/20260521000001_initial.sql:43-53
CREATE TABLE wf.wfah (
    wfah_id    uuid PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer NOT NULL,
    action     text NOT NULL,
    actor      jsonb NOT NULL,
    input      jsonb,
    applied_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);
-- migrations/wf/20260810000001_wfah_path.sql:22-24  (2026-08-10, K7)
ALTER TABLE wf.wfah ADD COLUMN from_node text, ADD COLUMN to_node text;
```

`from_node`/`to_node` **yalnız kayıt ve ekran içindir**; migration bunu açıkça yazıyor: `ForkTo` gibi çok-hedefli geçişlerde `to_node` **NULL**'dır çünkü hedefler `wf.wfe_branch`te satır satır durur (`20260810000001_wfah_path.sql:18-21`). Yani fork satırında "hangi kollara bölündü" WFAH'tan okunamaz.

**İzdüşüm.** `$wfah` sözleşmesi `{seq, action, actor, input, at}` ve süzgeç YOK:

```rust
// crates/wfe-core/src/v22/eval.rs:17-30
fn project_entry(e: &WfahEntry) -> Value { json!({ "seq":…, "action":…, "actor":…, "input":…, "at":… }) }
// crates/wfe-core/src/v22/eval.rs:117-120
pub fn with_wfah(mut self, wfah: &Wfah) -> Self {
    self.wfah = wfah.entries().iter().map(project_entry).collect();   // ← TÜMÜ
    self
}
```

Kolda koşan her ifade bu birleşik listeyi görür: kol `when`i (`pipeline.rs:631`), `possible_actions` (`:1260`), join koşulu (`:2663`), trigger `calc` (`ports.rs:610`).

**`$prev` / `$first`.** Uç girdi kısayolları listeyi süzmez (`eval.rs:172-178`): `$prev` = birleşik listenin SON kaydı. Paralel modda bu kayıt genellikle bir insan aksiyonu **değildir** — `apply_parallel` önce aksiyonu push eder (`pipeline.rs:712`), sonra trigger marker'larını, en sonra `stage_parallel_markers` ile `_branch_arrived` / `_branch_cancelled` / `_collapse` marker'larını yazar (`:779-790`). Yani bir kol join'e vardıktan sonra başka bir koldaki `$prev.action` `"_branch_arrived"` okur.

**Sistem marker'ları da aynı listede.** `_fork`, `_branch_arrived`, `_branch_cancelled`, `_branch_superseded`, `_collapse`, `_join` (`pipeline.rs:3075-3108` dokümantasyonu; `_join` istisnai olarak adapter tarafından yazılır).

> **Sorunun cevabı:** Bir koldaki aksiyon diğer kolun `$wfah` / `$prev` / `count($wfah, ...)` ifadelerini **ETKİLER**. Ayrıca dolaylı bir etki daha var: `rev` kolon değil, son WFAH seq'inden türetilir (`ports.rs:112-129`) → paralel modda **bir kardeş kolun aksiyonu diğer kolların `expected_rev` precondition'ını geçersizleştirir** (`executor.rs:1053` `check_rev`, retry döngüsüne girmez → 409 `conflict.stale_revision`).

### A3 — Join anında ne oluyor?

**Birleştirme adımı YOKTUR.** Birleştirilecek iki ctx hiç var olmadığı için join'de merge kodu da yoktur. Join yalnız üç şey yapar (`wfe_adapter.rs:716-742`): `FOR UPDATE` ile WFE satırını kilitler, varan kolu `arrived` işaretler, tamamlanma ölçütünü kilit altında yeniden doğrular; tuttuysa içteki `next` outcome'u (`MoveTo{join}` ya da `Terminal`) uygular.

**Tamamlanma ölçütü** üç kuraldan biri (`pipeline.rs:2658-2679`):

| Kural | Ölçüt | Kaynak |
|---|---|---|
| `All` (AND) | kardeş aktif kol kalmamalı (`others_active == 0`) | `pipeline.rs:2659` |
| `Quorum(k)` (OR/K-of-N) | `arrived_entries.len() >= k` | `pipeline.rs:2660` |
| `Expr(e)` | ZEN koşulu `$branches.<entry_node>` / `$arrived` ile `true` | `pipeline.rs:2661-2678` |

`$branches` her kol için **bool** taşır (varmış mı), `$arrived` varmış kol kimliklerinin dizisi (`eval.rs:71-85`). Kol kimliği `entry_node`dur, `branch_node` DEĞİL (kol içinde hareket ettikçe değişir — `ports.rs:37-45`).

**İki kol AYNI ctx alanına yazarsa:**

- **Son yazan kazanır.** `apply_effects` `set_path` ile alan bazında üzerine yazar; bir önceki değer korunmaz, sürüm tutulmaz (alan bazında; eski SNAPSHOT `wf.wfe_dynctx`te durur ama okunmaz).
- **Çakışma tespiti YOK.** Ne engine ne adapter "bu alanı başka bir kol da yazdı" sorusunu sormaz. `wf.wfe_dynctx`te kim yazdı bilgisi de yoktur.
- **Sıra deterministik DEĞİL** — commit sırasıdır, yani gerçek dünyadaki aksiyon sırası. Aynı senaryo iki kez koşulsa iki farklı sonuç verebilir.
- **Ama LOST UPDATE yok.** Gerçek eşzamanlılıkta `wf.wfe_dynctx`in `UNIQUE (wfe_id, seq)`i ihlal edilir → `insert_err` 23505'i `Conflict(StaleRevision)`a çevirir (`wfe_adapter.rs:57-64`), `is_retryable() == true` (`error.rs:25-35`), `WfeExecutor::apply` reload edip engine'i **yeniden koşar** (`executor.rs:1046-1091`, `MAX_ATTEMPTS = 3`). Yani kaybeden kolun effects'i kazananın ctx'i ÜZERİNE yeniden hesaplanır. "Silinmiş yazma" değil, "yeniden temellenmiş yazma".
- Bunun bir yan etkisi: kaybeden kolun `when` guard'ı da yeniden koşar. Kazananın yazdığı alan guard'ı değiştirdiyse aksiyon `TransitionNotFound` alabilir. Bu bugünkü, ölçülmüş davranıştır.

**Join sonrası tek WFE'nin gördüğü geçmiş:** hepsi. Tek liste olduğu için join'den sonraki node'un ifadeleri tüm kolların aksiyonlarını + tüm marker'ları görür.

### A4 — Görünürlük: bir kolun `c_a`'sına uyan aktör DİĞER kolun satırını görüyor mu?

**EVET, görüyor. Ve bu bilinçli yazılmış, testle kilitlenmiş bir karardır.**

Görünürlük tek `WHERE` parçasıdır (`crates/wfe/src/visibility.rs:72-112`). Kol kanalı bir **EXISTS**tir ve dış satırla korelasyon kurmaz:

```sql
-- crates/wfe/src/visibility.rs:95-109
OR EXISTS (
     SELECT 1 FROM wf.wfe_branch b
      WHERE b.wfe_id = e.wfe_id
        AND b.status = 'active'
        AND (   b.c_a      @> $role  OR b.c_a      @> $user  OR …
             OR b.view_c_a @> $role  OR b.view_c_a @> $user  OR …
             OR b.claimed_by @> $owner))
```

Yani soru "bu aktör **herhangi bir** aktif kolun adayı mı" — cevap evetse **WFE satırı** görünür hale gelir.

Havuz 2026-08-14'te bu parçaya bağlandı ve iki sorgu koşuyor: WFE-seviyesi (`pool.rs:139-169`) ve kol satırları (`pool.rs:181-205`). Kol sorgusunda `br` dıştan gelir, ama süzgeç yine WFE-seviyesi parçadır. Dosyadaki yorum bunu açıkça söylüyor:

```
// crates/server/src/routes/portal/pool.rs:177-180
// Satır süzgeci WFE-SEVİYESİDİR: WFE görünüyorsa AKTİF KOLLARININ HEPSİ listelenir.
// Kol bazında daraltmak ikinci bir görünürlük kuralı yazmak olurdu — 2026-08-13
// kararının yasakladığı şey tam bu. Kolu claim edebilmek ayrı bir sorudur ve
// `WfeExecutor::can_claim` node `c_a`'sını sorarak cevaplar.
```

Somut sonuç, `docs/spec/examples/paralel-onay.json` üzerinde: fork üç kola bölünür (`self__financeApprover`, `self__legalApprover`, `self__hrApprover`, join `self__resultCoordinator`). Bugün **finans onaylayıcısı havuzunda üç kol satırının hepsini görür** — hukuk ve İK kollarını da. Aksiyon alamaz (aşağıya bak) ama satırı ve WFE'yi görür.

**Görmek ≠ yapmak.** Bu ayrım bugün sağlam:

| Kapı | Neye bakar | Kaynak |
|---|---|---|
| Havuzda satır görünür mü | WFE-seviyesi `visibility::sql` | `visibility.rs:72` |
| Kol claim edilebilir mi | O KOLUN node `c_a`'sı | `executor.rs:1099-1122` (`can_claim`), projeksiyon kolonu okumaz |
| Kolda aksiyon alınabilir mi | Kolun `claimed_by` == aktör, sonra node/transition `c_a` | `pipeline.rs:659-681` |
| `possible-actions` ne döner | Kolun `claimed_by` aktör DEĞİLSE **boş** | `pipeline.rs:1247-1249` |

Yani bugünkü sızıntı **görünürlük sızıntısıdır, yetki sızıntısı değildir**: yabancı kolun satırı, başlığı, node etiketi, claim durumu ve `deadline`ı görünür; ayrıca detay ucu açıldığında (`GET /wfe/:id`) **ctx'in tamamı** (yalnız alan bazlı `x-visibility` süzgeciyle, `executor.rs:1445-1449`) ve **WFAH'ın tamamı** görünür.

### A5 — Node `listable` paralel modda hangi node'lara bakıyor?

**TÜM aktif kolların node'larına.** İki okuma var, ikisi de aynı kümeyi kullanır.

*Referans okuma (`can_view`, sim/testler)* — `crates/wfe-core/src/v22/visibility.rs:163-249`:

```rust
// :205-215 — "aktif node" kümesi paralel modda AKTİF KOLLARIN node'larıdır
let active_nodes: Vec<&str> = if wfes.join_target.is_some() {
    wfes.branches.iter().filter(|b| b.status == Active).map(|b| b.branch_node.as_str()).collect()
} else { wfes.current_node.as_deref().into_iter().collect() };
for node_key in active_nodes {
    …
    if authorize_or_delegated_anchored(&node.c_a, …) { return Ok(true); }    // (c)
    if matches_grant_rules(&node.listable, viewer, wfes, org).await? { return Ok(true); }  // (f)
}
```

Kriter (f) — node `listable[]` — kriter (c) ile **aynı** aktif-node kümesini paylaşır (yorumu `:157-161`'de). Yani üçüncü kolun node `listable`ına uyan aktör tüm WFE'yi görür.

*Üretim okuması (projeksiyon)* — kol satırının `view_c_a` kolonu. `fill_view_grants` her kol için node `listable`ını çözüp `wfe_branch.view_c_a`ya yazar (`executor.rs:1364-1408`). Dikkat çeken bir ayrıntı: kol projeksiyonunda `$node` guard'ı **`None`** verilir (`executor.rs:1400` + `:1390-1393` gerekçesi) çünkü paralel modda WFE-seviyesi `current_node` NULL'dır ve okuma anında `can_view` (f) guard'ı o NULL ile değerlendirir. Yani **`listable[].when` içinde `$node` kullanan bir kural paralel modda `$node == null` görür** — B1/B2 çalışılırsa bu, kural yazımını etkileyecek mevcut bir sınırdır.

`listable`/`wf_admin` guard'ında `$actor` YASAKTIR (`grant_when_actor_ref`, CLAUDE.md:267) — yani "kuralı soranın kim olduğuna göre" yazmak zaten mümkün değil; kural WFE'nin çapasına (`origin_orgu_id`) göre çözülür.

### A — Özet tablo

| Soru | Bugün | Kaynak |
|---|---|---|
| Ctx kol başına mı? | **Hayır — WFE'de tek** | `ports.rs:65`, `20260521000001_initial.sql:33-41` |
| WFAH kol başına mı? | **Hayır — WFE'de tek liste** | `ports.rs:66`, `20260521000001_initial.sql:43-53` |
| WFAH satırı hangi kola ait, biliniyor mu? | Kolon YOK; `from_node`/`to_node`dan + WFD topolojisinden ÇIKARILABİLİR (kol alt grafları ayrık) | `20260810000001_wfah_path.sql:22`, `validator.rs:2014-2100` |
| `when` başka kolun ctx yazmasını görür mü? | **Görür** | `pipeline.rs:617,630,709,799` |
| `count($wfah,…)` başka kolun aksiyonunu sayar mı? | **Sayar** | `eval.rs:117-120` |
| `$prev` kolun kendi son aksiyonu mu? | **Değil** — birleşik listenin sonu, çoğu zaman sistem marker'ı | `eval.rs:172-178`, `pipeline.rs:779` |
| Join ctx merge yapar mı? | Yapmaz (merge edilecek iki şey yok) | `wfe_adapter.rs:716-742` |
| İki kol aynı alana yazarsa? | Son commit kazanır, tespit YOK, sıra = gerçek zaman | `effects.rs:47-61`, `wfe_adapter.rs:57-64` |
| Kol A'nın adayı kol B'nin satırını görür mü? | **Görür** (bilinçli) | `visibility.rs:95-109`, `pool.rs:177-180` |
| Node `listable` hangi node'lara bakar? | Tüm aktif kolların node'larına | `visibility.rs:205-215` |
| Yabancı kolun adayı aksiyon alabilir mi? | **Alamaz** (claim + `c_a` kapıları ayrı) | `pipeline.rs:659-681,1247-1249` |
| Kol satırı bir kola ait `rev` taşır mı? | Hayır, `rev` WFE-seviyesi (son WFAH seq) | `ports.rs:112-129` |

---

## B. Kullanıcının istediği model ne demek?

İfadede **iki ayrı talep** var. Karıştırılmamaları kritik: biri süzgeç işi, diğeri motor semantiği işi.

### B1 — Görünürlük izolasyonu

> "Farklı kollar birbirinin wfe'sini listable'da yoklarsa göremezler."

**Anlamı:** kol satırının görünürlüğü kolun KENDİ `c_a`/`view_c_a`sıyla belirlenir. Bir aktör yalnız (a) kendi eşleştiği kolları, (b) kök `listable`/`wf_admin` ile açıkça kendisine verilmiş WFE'leri görür.

Bunun bugünkü koddaki karşılığı **hazır** duruyor: `wf.wfe_branch.c_a` ve `.view_c_a` kolonları kol başına yazılıyor (`executor.rs:1364-1408`), GIN index'leri var (`20260813000001:85`, `20260813000004:63`). Eksik olan tek şey `branch_pool_sql()`in kendi `br` satırını da süzmesi — yani `visibility::sql`in kol EXISTS'inin dış satıra **korelasyonlu** bir varyantı.

**Ama küçük değil.** İki yapısal soru doğuyor:

1. **"Bir WFE'yi görmek" ile "bir kolu görmek" ayrı sorulara ayrılır.** Bugün tek fonksiyon (`visibility::sql`) iki tüketiciye hizmet ediyor ve 2026-08-13 kararının ÖZÜ "ikinci bir görünürlük kuralı yazmayacağız"dı (`pool.rs:178-180`). B1 tanım gereği ikinci bir kural açar. Doğru biçimi "ikinci kural" değil, **aynı parçanın kol-korelasyonlu ikinci imzası** olurdu (`sql_for_branch(offset, branch_alias)`), ve `visibility_report` kontrat testinin bu ikinci imzayı da kapsaması gerekir.
2. **`GET /wfe/:id` detay ucu ne döner?** Kolları göremeyen ama WFE'yi gören biri `branches[]`in tamamını mı görecek? Bugün `WfeView.branches` süzülmüyor (`executor.rs:352`). Süzülürse istemci "3 koldan 1'ini görüyorum" durumunu anlamlandırmak zorunda; süzülmezse B1 detay ucunda delinir.

**Etkilenen yerler (ölçülen):** `crates/wfe/src/visibility.rs` (yeni imza) · `crates/server/src/routes/portal/pool.rs:181-205` · `crates/wfe-core/src/v22/visibility.rs:205-215` (referans okumanın da ayrışması) · `crates/wfe/src/executor.rs` `query`/`possible_actions_for` · `crates/server/src/bin/visibility_report.rs` (kontrat) · portal kol listesi UI.

### B2 — VERİ izolasyonu

> "Kollar bu değişiklikleri sadece kendi kol yolunda görebilir. 2.'nin yaptığı değişikliği 1. koldaki göremez. Sonra join'lenince tekrar tek'e iner."

Bu, motorun **durum modelini** değiştirir. Açık açık ne demek olduğunu yazıyorum.

#### B2.1 — Kol-yerel ctx (branch-local DynCtx + join'de merge)

Fiilen: fork'ta ctx **dallanır**, her kol kendi overlay'ini yazar, join'de overlay'ler tek ctx'e katlanır.

- **Depolama:** `wf.wfe_dynctx`e kol boyutu gerekir (`branch_key` kolonu + `UNIQUE (wfe_id, branch_key, seq)`), ya da ayrı bir `wf.wfe_branch_dynctx` tablosu. Mevcut `UNIQUE (wfe_id, seq)` **aynı zamanda tek yarış korumasıdır** (`wfe_adapter.rs:46-64`) — kol boyutu eklenince o koruma kol-içine iner ve "iki kol aynı anda commit edebilir" hale gelir. Yani `StaleRevision` yolunun yerine yeni bir eşzamanlılık modeli kurulmalıdır.
- **Okuma:** `Wfes.dynctx` tek alan olmaktan çıkar. Her ifade değerlendirme noktasının "hangi kolun gözünden" sorusunu cevaplaması gerekir. Bugün ctx'i okuyan noktalar: `apply` / `apply_parallel` / `possible_actions` / `next_escalation` / `fire_escalation` / `fire_claim_timeout` / `resolve_wft` / `terminal_outcome` / `stage_calls` / `candidates_at` / `view_grants` / `node_view_grants` / `resolve_node_c_a` / `filter_dynctx` / `matcher` (`MatchEnv.ctx`) / `AutoexecRunner` (`ExecEnv.ctx`) / sim. **Hepsine kol bağlamı taşınmalı.** Bunlardan bazıları paralel moda özgü DEĞİL (`view_grants`, `filter_dynctx`) — yani ya kol-agnostik bir "birleşik görünüm" tanımlanır ya da bu yüzeyler de kolluk kazanır.
- **Join'de merge:** yeni bir kural KATEGORİSİ. En az şunlar cevaplanmalı: aynı alanı iki kol yazdıysa hangi değer? Kol sırası mı, `entry_node` alfabetik mi, açık bir `merge` politikası mı (`{strategy: "last-write"|"error"|"per-branch-namespace"}`)? İptal edilen kolun (collapse/quorum) yazdıkları merge'e girer mi? Bunlar **ürün kararlarıdır**, E bölümünde soruluyor.
- **Alternatif ve daha ucuz bir şekil:** merge hiç yapılmaz, kol yazmaları ctx'te **ayrı ad alanına** düşer (`$ctx.branches.<entry_node>.*`), ortak ctx yalnız fork ÖNCESİ yazılanları taşır. O zaman "izolasyon" bir yol kuralıdır, motor semantiği değil — ve bugün bile `wfes_effects` yollarını tasarımcı seçtiği için **belge disipliniyle yaklaşık olarak sağlanabilir** (motor zorlamaz). Bu, D2'deki en hafif seçenek.
- **`$env` ile karışmaz** (secret'lar ZEN'e hiç girmez, `env.rs`/CLAUDE.md:558-561) — ortam kollanmaz, bu iyi haber.

#### B2.2 — Kol-yerel WFAH görünümü

- **`count($wfah, ...)` semantiği DEĞİŞİR.** Bugün `count` tüm kolları + marker'ları sayar. Kol-yerel görünümde aynı ifade daha küçük bir sayı verir. Bu, C bölümünün 1. maddesi — CLAUDE.md'de kritik değişmez olarak yazılı.
- **`$prev` / `$first` anlam kazanır** (bugünkü hâlleri paralel modda pratik olarak kullanılamaz durumdadır: `$prev` sistem marker'ı okur). Bu, B2'nin en net faydası.
- **Kol atfı ÇIKARILABİLİR, saklanmıyor.** `validator.rs:2014-2100` kol alt graflarının **ayrık** olmasını garanti eder (`parallel_disjoint`) → bir `(from_node, to_node)` çifti en fazla bir kola aittir. Ayrıca kol kimliği `entry_node`dur (`ports.rs:37-45`) ve BFS ile alt graf hesaplanabilir; editör tarafında bu BFS **zaten var** (`agnoflow-frontend/src/utils/parallelScope.ts`, 140 satır, `useExport.ts`'in BFS'inin aynısı). Yani "depolama tek, okuma süzgeçli" mümkündür ve golden fixture'a dokunmadan yapılabilir — bu D3'ün temeli.
- **Ama sistem marker'ları kola atfedilemez.** `_fork`'un `to_node`u NULL (`20260810000001_wfah_path.sql:18-21`); `_collapse` özeti hiçbir kolun değil; `_join` WFE'nin. Kol görünümü bunları ya hepsine gösterir ya hiçbirine — ve `count($wfah, #.action == "escalate:...")` gibi sayımlar bundan etkilenir (`escalate:<node>:<idx>` marker'ının node'u kolun node'udur, dolayısıyla atfedilebilir; `_collapse` değildir).
- **`$wfah` izdüşümü sözleşmesi (`{seq, action, actor, input, at}`) dokunulmaz sayılıyor** (CLAUDE.md:45, :419). Kol bilgisi izdüşüme **eklenmek zorunda değil**: kol-yerel görünüm ifadenin GÖRDÜĞÜ LİSTEYİ süzmekle ifade edilir, izdüşümün alan kümesini büyütmekle değil. `expr_types.rs:40-58` `WFAH_FIELDS` ve editördeki aynası (`whenFields.ts:8,340`) böylece değişmez. Alan eklenirse golden fixture (`WfahEntry` serileştirmesi) ve `zen_wfah_field_unknown` kuralı birlikte kırılır — bu yüzden **eklememek** tercih edilmelidir.

#### B2.3 — Etkilenen diğer yüzeyler

| Yüzey | B2'nin etkisi | Kaynak |
|---|---|---|
| `expr_types` tip çıkarımı | `#.input.<yol>` tipi `wfes_effects`ten çıkarılır. Kol-yerel ctx'te "hangi kolun effects'i" sorusu doğar; iki kol aynı yola farklı tip yazarsa bugün çakışma tespit edilmez | `expr_types.rs`, CLAUDE.md:48 |
| Projeksiyon kolonları | `view_c_a`/`current_view_c_a`/`branch_view_c_a` `listable[].when` guard'ını ctx üzerinde koşar → hangi ctx? | `executor.rs:1328-1408` |
| Simülasyon | `SimState` tek `dynctx: Value` taşır (`sim.rs:23`), kol durumunu `branches` ile modelliyor (`:32`) ama ctx'i kollamıyor. B2 sim'i de dallandırır | `crates/wfe/src/sim.rs:23,32,100,172` |
| `AutoexecRunner` / `calc` | `ExecEnv { ctx, wfah, … }` tek görünüm alır | `ports.rs:598-618` |
| `filter_dynctx` (`x-visibility`) | Alan bazlı gizlilik hangi kol görünümü üzerinde koşar? | `visibility.rs:105-140` |
| WFC (`calls[].input`) | Kolda başlatılan alt akışa hangi ctx aktarılır? | `pipeline.rs:793` `stage_calls` |
| `wf.wfe_dynctx` yarış koruması | `UNIQUE (wfe_id, seq)` kol boyutuyla zayıflar; yerine yeni model gerekir | `wfe_adapter.rs:46-64` |

---

## C. Ne kırılır?

### C1 — `count($wfah, ...)` eşikleri

CLAUDE.md dört ayrı yerde bunu kritik değişmez sayıyor (:46, :294, :419, :475) ve `docs/spec/decisions.md` en az beş yerde aynı gerekçeyle karar veriyor (:1350, :1673, :1718, :1842). Somut kırılma senaryoları:

| Senaryo | Bugün | Kol-yerel WFAH görünümünde |
|---|---|---|
| `count($wfah, #.action == "onay") >= 2` — üç kol, her kol "onay" veriyor, join sonrası node bunu sayıyor | Join SONRASI node paralel modda değil → kol görünümü kavramı yok → 3 sayar (bugünkü davranış korunur) | Aynı, **eğer** join sonrası görünüm birleşik kalırsa. Kol görünümü join sonrasına sızarsa 1 sayar ve akış kilitlenir |
| Kol İÇİNDE `count($wfah, #.action == "duzelt") >= 1` (kolun kendi döngüsü) | Kardeş kolun `duzelt`i de sayılır → **bugün yanlış** | Doğru sayar → **davranış değişir, yayınlanmış akış farklı karar verir** |
| `escalate:<node>:<idx>` sayımı (`next_escalation` tabanı) | Node adı kola özgü olduğu için pratikte doğru | Değişmez (marker atfedilebilir) |
| `_branch_arrived` / `_collapse` sayan bir ifade | Hepsini görür | Marker atfı belirsiz → kararsız |

**Kritik nokta:** kırılma yalnız "sayı küçülür" değil, **ters yönde de var**: bugün YANLIŞ sayan bir akış düzeltilirse davranışı değişir. Yayınlanmış bir belgenin davranışını "düzeltmek" de kırıcı bir değişikliktir — `(wfd_id, version)` immutable olduğu için (`MEMORY.md`, CLAUDE.md:14) yayınlanmış sürüm zaten yeniden yazılamaz; ama **koşan WFE'ler kendi belgeleriyle çalışmaya devam eder** ve motor semantiği altlarından değişirse yarı yolda davranış değiştirirler. Bu, D bölümündeki her seçenekte açıkça ele alınmalı: semantik değişikliği **belge sürümüne kapılamak** (`wfd_version` ya da yeni bir `parallel.isolation` bayrağı) gerekir mi?

### C2 — `wfes_effects` = context'e TEK yazma yolu (WOR-70)

Kural: CLAUDE.md:58. Kol-yerel ctx bu kuralı **delmez** ama anlamını genişletir: "tek yazma yolu" hâlâ `wfes_effects`tir, değişen şey **nereye yazdığı**. İki risk:

- **Sessiz kayıp riski.** Kol-yerel yazma join'de merge edilmezse (ya da merge kuralı o alanı düşürürse) tasarımcının yazdığı bir alan join sonrası **yok** olur. Bugün böyle bir sınıf hata yok. WOR-70'in ruhu "yazma açık olsun"du; yazmanın **kaybolabilir** hale gelmesi o ruhla çelişir → merge kuralı ya hiç kayıp üretmemeli ya da kaybı **validator'da** yakalayabilmeli.
- **`unknown_dollar_ref` kapısıyla ilişki.** Motor çözemediği `$`-string'i düz metin yazar, kapı bunu yayında yakalar (CLAUDE.md:50, `v22/dollar.rs`). Kol-yerel ctx yeni bir namespace getirirse (`$branch_ctx.*` gibi) `dollar::EXACT`/`PREFIXES` genişletilmeli, aksi halde yazım hatası sessiz metne düşer.

### C3 — Golden fixture ve `$wfah` izdüşümü sözleşmesi

`docs/spec/examples/kredi-basvuru.golden.json` DEĞİŞTİRİLMEZ (CLAUDE.md:57; tek istisna WOR-70, kullanıcı onayıyla). Ölçüm: fixture'da `parallel` **hiç geçmiyor** (`grep -c parallel` → 0), yani paralel semantiği doğrudan fixture'a dokunmaz. Fixture'ı kıracak olan tek şey **`WfahEntry` tipine alan eklemek**tir.

> **Kol bilgisini izdüşüme eklemek GEREKMEZ.** Kol-yerel görünüm "ifadeye verilen liste" seviyesinde ifade edilebilir: `EvalEnv::with_wfah` çağrısına bir süzgeç parametresi (`with_wfah_scoped(&wfah, Some(branch_scope))`) eklenir, `project_entry` ve `WFAH_FIELDS` **hiç değişmez**. Bu, 2026-08-10'daki `from_node`/`to_node` kararının aynı deseni: bilgi adapter/executor seviyesinde türetilir, core tipine girmez (`20260810000001_wfah_path.sql:10-16`, CLAUDE.md:300-304).

Kol atfının SQL karşılığı da tabloya dokunmadan kurulabilir: `wf.wfah.from_node`/`to_node` + WFD'den BFS ile hesaplanan kol alt grafı. Maliyeti: her okuma WFD'yi ister (bugün `load` istemez) ve BFS'i önbelleklemek gerekir.

### C4 — Editör tarafı

Ölçüm (`agnoflow-frontend`):

| Dosya | Satır | Bugün paralel/kol farkındalığı | B1/B2 etkisi |
|---|---|---|---|
| `src/utils/validateParallelRules.ts` | 274 | Motorun `check_parallel`ının 1:1 aynası; `$branches.<x>` referanslarını doğrular (`:214-217`), `join_mode ∈ {and,or,expr}`, `join_threshold` yalnız `or` ile, alt graf ayrıklığı/nested/dead-end (`:1-23` sözleşme yorumu) | B1'de değişmez. B2'de motora eklenen her yeni kural (merge politikası, izolasyon bayrağı) buraya da yazılır — ayna sözleşmesi |
| `src/utils/parallelScope.ts` | 140 | **Kol interior kümesini BFS ile ZATEN hesaplıyor** (`useExport.ts`in BFS'inin 1:1'i, `:1-13`) | **Varlık, borç değil**: B2'nin editör tarafındaki kol-kapsam altyapısı burada hazır. `collapsesParallel` için yazılmış ama kapsam sorusu aynı |
| `src/utils/whenFields.ts` | 1641 | `parallel`/`branch` kelimesi **HİÇ geçmiyor** (grep: 0 eşleşme). `WFAH_FIELDS` tek kaynaktan gelir (`:8,340`), `collectActionInputCtxMap` (`:203`) `wfes_effects`ten `#.input.*` tipini çıkarır | B2'de **en büyük editör işi**: tip çıkarımı "hangi kolun görüşü" sorusunu hiç bilmiyor. İki kol aynı input yolunu farklı tiple yazarsa bugün de tespit edilmiyor |
| `src/components/graph/ParallelStepNode.tsx` / `ParallelJoinStepNode.tsx` | — | Fork/join çizimi, join modu seçimi | B2'de merge politikası UI'sı buraya iner |

`$branches.*` / `$arrived` join koşulu kurucusu B1'den **etkilenmez** (görünürlük join kuralına girmez) ve B2'den de doğrudan etkilenmez — ama B2 kol kimliğini (`entry_node`) daha çok yere yayacağı için o kimliğin editörde tek kaynaktan gelmesi önemi artar.

### C5 — Diğer ölçülen kırılganlıklar

- **`duplicate_c_a` kısıtı B1'i sınırlıyor.** "Aynı havuzdan iki kol" bugün YASAK (2026-08-14 kararı, `decisions.md:2044-2050`; `validator.rs:1261-1264`). Yani B1 uygulanırsa "üç kolun üçü de aynı havuza bakıyor, ama her biri yalnız kendisini görmeli" senaryosu **çizilemez**. Kararın kendisi "kol kimliğini node anahtarından ayırmak" işini erteledi — B1'in ürün değeri kısmen o işe bağlı.
- **`rev` paralel modda kollara ait değil** (`ports.rs:112-129`): kardeş kolun aksiyonu diğer kolların `expected_rev`ini geçersizleştirir → portal kullanıcısı sebepsiz `409 conflict.stale_revision` görebilir. Bu, B1/B2'den bağımsız **mevcut** bir devex sorunudur ve B2 kol-yerel seq getirirse kendiliğinden düzelir.
- **`listable[].when` içinde `$node` paralel modda `null`dır** (`executor.rs:1390-1400`). B1 kol-bazlı süzme getirirse "kol node'una göre listable" yazmak isteyen tasarımcı bugün bunu yapamaz.
- **`visibility_report` kontratı**: `can_view` (belge) ile `visibility::sql` (projeksiyon) eşitliğini ölçüyor. B1 ikisini birden değiştirmek zorundadır, yoksa "KONTRAT SAĞLAM" hedefi düşer.

---

## D. Seçenekler

Her seçenek için: ne yapar · maliyet · neyi kırar · geriye uyumluluk.

### D0 — Hiçbir şey yapma, mevcut semantiği BELGELE

**Ne yapar.** Kod değişmez. `docs/spec/decisions.md`'ye bilinçli bir karar kaydı girer: "paralel modda ctx ve WFAH WFE-seviyesidir; kollar birbirinin verisini ve satırını görür; kol izolasyonu YOKTUR." `docs/spec/terminology.md`'ye paralel modda `$prev`in sistem marker'ı okuduğu ve `count($wfah,…)`ın kardeş kolları saydığı yazılır. Editörde fork adımına bir bilgi notu eklenir.

**Maliyet.** Küçük — 2 belge + 1 editör metni. Yarım gün.

**Neyi kırar.** Hiçbir şey.

**Geriye uyumluluk.** Tam.

**Neden meşru:** bugünkü davranış bir arıza değil, **belgelenmemiş bir tasarım**. Belgelenmemişliğin somut maliyeti var: tasarımcı kol içinde `count($wfah, #.action == "duzelt") >= 1` yazdığında kardeş kolun `duzelt`inin de sayıldığını bilmiyor ve akış sessizce yanlış karar veriyor. Bu tuzağın **yazılı olması**, izolasyonun kendisinden çok daha ucuz bir kazançtır. **Maliyeti:** tuzak durur; tasarımcı disipliniyle (kol başına ayrı aksiyon adı, kol başına ayrı ctx ad alanı) dolanılır.

### D1 — Yalnız B1: kol-bazlı SATIR süzgeci (veriye hiç dokunmadan)

**Ne yapar.** `visibility::sql`in kol-korelasyonlu ikinci imzası (`sql_for_branch(offset, "br")`): kol satırı yalnız `br.c_a` / `br.view_c_a` / `br.claimed_by` viewer'a uyuyorsa VEYA kök `view_c_a` uyuyorsa listelenir. `branch_pool_sql()` onu kullanır. `can_view` (c)/(f) referans okuması kol bazında ayrışır. `WfeView.branches` süzülür. `visibility_report` ikinci imzayı da ölçer.

**Maliyet.** Orta-küçük, tek katman: `crates/wfe/src/visibility.rs` (+~40 satır ve testleri) · `routes/portal/pool.rs:181-205` · `wfe-core/src/v22/visibility.rs:205-215` (kol-kapsamlı varyant) · `executor.rs` `query`/`possible_actions_for` · `bin/visibility_report.rs` · portal kol listesi. **Migration YOK** (kolonlar mevcut, index'ler mevcut). Tahmini 1 PR, 1-2 gün.

**Neyi kırar.** (1) 2026-08-13 kararının "tek kural" ilkesini gevşetir — mitigasyon: ikinci KURAL değil, aynı parçanın ikinci İMZASI, ve kontrat testi ikisini birden ölçer. (2) Bugün üç kol satırı gören aktör bir satır görmeye başlar → **portal davranışı değişir**, kullanıcı eğitimi gerekir. (3) `duplicate_c_a` kısıtı yüzünden "aynı havuzdan üç kol" senaryosu hâlâ çizilemez, yani B1'in değeri bu kısıt kalkana kadar kısmi kalır (`decisions.md:2044-2050`).

**Geriye uyumluluk.** Şema uyumlu, API şekli uyumlu. **Davranış değişir** (daralır). Kaçış valfi gerekirse kök `listable[]` ile açıkça geri açılabilir — kullanıcının ifadesi tam bunu söylüyor ("listable'da spesifik olarak söylenmediyse").

### D2 — B1 + kol yazmalarını ctx'te AD ALANINA ayırma (motor semantiği DEĞİŞMEZ)

**Ne yapar.** D1'in üstüne, **veri izolasyonunu bir BELGE DİSİPLİNİ olarak** kurar: kol içinde yazılan `wfes_effects` yolları `branches.<entry_node>.*` altına düşer; validator bunu **zorlar** (kol alt grafındaki bir transition'ın `wfes_effects.set` yolu kolun ad alanı dışına yazarsa uyarı/hata). Editör fork adımında bu ad alanını otomatik önerir (`parallelScope.ts`in BFS'i kol interior'ını zaten biliyor). Join'de merge yapılmaz — çünkü çakışma yapısal olarak imkânsızdır (her kol ayrı alt ağaca yazar). Kolun kendi ifadeleri `$ctx.branches.<kendi>.*` okur.

**Maliyet.** Orta. Motorda **yeni durum modeli YOK**; iş validator + editörde: `validator.rs`e yeni bir kural (`parallel_ctx_scope`), `validateParallelRules.ts`e aynası, editörde yol öneri/otomatik prefix. `expr_types` etkilenmez (yollar hâlâ statik). Tahmini 1-2 PR.

**Neyi kırar.** (1) Yayınlanmış paralel akışlar bu kurala uymuyor → kural **HATA** olursa yeni sürüm yayınlanamaz (`migration-notes` M-kaydı gerekir), **UYARI** olursa 2026-08-12→14 döneminin dersi tekrarlanır (uyarı yayını durdurmaz, hata üretime çıkar — `decisions.md:2026-2031`). (2) WFAH tarafını **hiç çözmez**: `count($wfah,…)` yine kardeş kolları sayar. (3) Kollar arası **kasıtlı** paylaşım (bir kolun diğerine bilgi geçmesi) zorlaşır.

**Geriye uyumluluk.** Motor tam uyumlu. Belge kapısı seviyesinde kırıcı olabilir (kuralın seviyesine bağlı).

### D3 — Kol-yerel WFAH GÖRÜNÜMÜ (depolama tek, okuma süzgeçli)

**Ne yapar.** `wf.wfah` ve `WfahEntry` **değişmez**. `EvalEnv`e kol kapsamı eklenir: `with_wfah_scoped(&wfah, scope)` — `scope` bir node kümesidir (kolun alt grafı, `validator.rs:2014-2100`in BFS'inin runtime karşılığı) ve `wf.wfah.from_node`/`to_node` ile eşleştirilir. Kolda koşan `when` / `possible_actions` / `calc` yalnız kendi kolunun kayıtlarını + fork ÖNCESİ kayıtları görür. Join sonrası görünüm **birleşik**e döner ("sonra join'lenince tekrar tek'e iner").

**Maliyet.** Büyük ama SINIRLI ve tek yönlü. Değişenler: `eval.rs` (yeni `with_wfah_scoped`, `project_entry` DEĞİŞMEZ) · `pipeline.rs`in her kol-bağlamlı ifade noktası (`:631`, `:1260`, `:2663`, trigger `calc`) · `Wfes`e kol atfı için `from_node`/`to_node`u taşımak gerekir → **`WfeStore::load` bugün bunları okumuyor** (`wfe_adapter.rs:322-342` `WfahEntry`e yalnız 5 alan koyuyor) → adapter seviyesinde bir yan-liste (`WfahPathSource` deseninin, `executor.rs`, genişletilmiş hâli) · sim (`sim.rs`) · `expr_types`e "bu ifade kol kapsamında koşuyor" bilgisi (tip çıkarımı listeyi değil şekli çıkardığı için muhtemelen etkilenmez — **ÖLÇÜLEMEDİ**, ayrıca incelenmeli).

**Neyi kırar.** C1'in TAMAMI. Kol içinde `count($wfah,…)` kullanan yayınlanmış her akış farklı sayar. Sistem marker'larının atfı belirsiz (`_fork`/`_collapse`/`_join` kolun değil) → kapsam kuralı bunları açıkça ele almalı. `$prev` anlam kazanır (fayda) ama **değeri değişir** (kırılma).

**Geriye uyumluluk.** Kırıcı. Tek güvenli yol: semantiği **belgeye bir bayrakla kapılamak** (`parallel.isolation: "shared" | "branch_local"`, varsayılan `shared`), böylece yayınlanmış akışlar aynen koşar ve yeni akışlar izolasyonu açıkça ister. Bu, `terminal_when`/`join_mode` gibi mevcut desenlerle uyumlu.

### D4 — Kol-yerel DEPOLAMA (tam B2: dallanan ctx + dallanan WFAH + join merge)

**Ne yapar.** `wf.wfe_dynctx`e kol boyutu (ya da `wf.wfe_branch_dynctx`), `wf.wfah`a kol kolonu, join'de açık merge kuralı. Kullanıcının ifadesinin **tam** karşılığı.

**Maliyet.** Çok büyük ve **her katmana** dokunur: şema + migration + `Wfes` tipi + `TransitionCommit` (`new_dynctx: Value` → kol başına) + adapter'ın tüm commit kolları + yarış modeli (mevcut `UNIQUE (wfe_id, seq)` koruması kol içine iner, WFE-seviyesi koruma yeniden kurulmalı — `wfe_adapter.rs:46-64`) + `rev` semantiği + `visibility` projeksiyonları (`fill_view_grants` hangi ctx'i görüyor?) + `filter_dynctx` + `expr_types` + sim + editör + `GET /wfe/:id` şekli (kol başına ctx döner mi?) + golden fixture riski. Kaba büyüklük: **birden çok faz, haftalar**, ve içinde en az üç yeni ürün kararı var (merge politikası, iptal edilen kolun yazmaları, join sonrası geçmiş).

**Neyi kırar.** C1 + C2 + potansiyel olarak C3 (kol kolonu `WfahEntry`e sızarsa golden fixture) + `wf.wfe_dynctx`in yarış koruması + `expected_rev` sözleşmesi. Ayrıca **bilinçli olarak ertelenmiş** bir işi (kol kimliğini node anahtarından ayırmak, `decisions.md:2049-2050`) fiilen önkoşul yapar: kol-yerel depolama kol kimliğine sağlam bir anahtar ister, `branch_node` değişken olduğu için `entry_node` yeterli mi sorusu netleşmeli.

**Geriye uyumluluk.** D3'ün bayrak yaklaşımı olmadan imkânsız. Bayrakla bile: iki durum modelini aynı motorda taşımak (paylaşılan ve dallanan ctx) kalıcı bir karmaşıklık borcudur.

### D — Karşılaştırma

| | Kollar birbirinin SATIRINI görmez | Kollar birbirinin VERİSİNİ görmez | Şema değişir | Yayınlanmış akışları etkiler | Kaba büyüklük |
|---|---|---|---|---|---|
| **D0** belgele | — | — | — | — | yarım gün |
| **D1** B1 süzgeci | ✅ | — | — | portal görünümü daralır | 1-2 gün |
| **D2** + ctx ad alanı | ✅ | ~ (disiplinle, motor zorlamaz) | — | belge kapısı (kural seviyesine bağlı) | 1-2 PR |
| **D3** kol-yerel WFAH görünümü | ✅ (D1 ile) | WFAH ✅, ctx — | — | **`count($wfah)` semantiği** | büyük, tek yön |
| **D4** kol-yerel depolama | ✅ | ✅ | ✅ | çok geniş | haftalar, çok faz |

---

## E. Açık sorular (kod okuyarak cevaplanamaz — kullanıcıya)

**Veri semantiği**

1. **İki kol AYNI ctx alanına yazınca hangi değer kazanmalı?** Son yazan mı, hata mı (join'de `WFD.CtxConflict`), yoksa yapısal olarak imkânsız mı kılınmalı (kol başına ad alanı, D2)?
2. **Kol iptal edilirse (collapse / quorum eşiği dolup geride kol kalması) o kolun ctx yazmaları geri mi alınmalı?** Bugün geri alınmaz (tek ctx, tek zaman çizgisi). Onay geçersizleşse bile o kolun yaptığı hesap ctx'te durur. Bu istenen mi?
3. **Join sonrası tek WFE hangi kolun geçmişini görüyor?** Kullanıcının "tekrar tek'e iner" ifadesi birleşik geçmişi ima ediyor ama açık değil: (a) hepsini, (b) yalnız join'i dolduran kolun, (c) yalnız fork öncesini + join marker'larını.
4. **Kol izolasyonu isteğe bağlı mı, zorunlu mu?** Yani belgede `parallel.isolation` gibi bir bayrak mı olsun (yayınlanmışlar korunur) yoksa motor semantiği topluca mı değişsin?

**Görünürlük**

5. **Kol satırını göremeyen ama WFE'yi gören aktör `GET /wfe/:id`de kaç kol görmeli?** Hepsini mi (B1 detayda delinir), yalnız kendi eşleştiklerini mi (istemci "3 koldan 1'i" durumunu anlamlandırmalı), yoksa sayı ama kimlik olmadan mı?
6. **Kolları göremeyen bir WFE hâlâ "aktif" görünmeli mi?** Bugün `current_node IS NOT NULL` süzgeci paralel WFE'nin WFE-seviyesi satırını havuzdan çıkarıyor (`pool.rs:164`). B1'de bir aktör WFE'yi kök `listable` ile görüyor ama hiçbir kolunu göremiyorsa havuzunda ne görecek?
7. **`listable`'da "kol" nasıl söylenir?** Kullanıcının ifadesi "listable'da spesifik olarak söylenmediyse" diyor. Bugün `listable[]` WFE seviyesinde, `nodes.<key>.listable[]` node seviyesinde. Kol seviyesi node seviyesiyle AYNI şey mi (kol = node kümesi), yoksa ayrı bir kavram mı?
8. **"Aynı havuzdan iki kol" kısıtı** (`duplicate_c_a`, 2026-08-14) bu iş için kaldırılmalı mı? Kaldırılmazsa "her kol yalnız kendini görsün" senaryolarının bir kısmı çizilemez.

**Kapsam**

9. **`$prev`in bugün paralel modda sistem marker'ı okuması bir arıza mı, kabul mü?** Ayrı ve çok daha küçük bir düzeltme olarak ele alınabilir (kol kapsamı olmadan bile: `$prev` marker'ları atlayabilir) — ama bu da yayınlanmış akışların davranışını değiştirir.
10. **Kardeş kolun aksiyonu diğer kolların `expected_rev`ini bozuyor** (A2 sonu). Bu kullanıcı için görünür bir sorun mu, yoksa portal bunu şeffaf retry ile yutuyor mu?

---

## Tavsiye (tek paragraf)

**Şimdi D0'ı yap, D1'i ayrı ve küçük bir iş olarak sıraya al, D3/D4'e girme.** Gerekçe üç ölçüme dayanıyor: (1) Bugünkü davranışın en pahalı yanı izolasyonun olmaması değil, **belgelenmemiş olması** — kol içinde `count($wfah, #.action == "x")` yazan tasarımcı kardeş kolun da sayıldığını bilmiyor ve akış sessizce yanlış karar veriyor; bu tuzağı yazıya geçirmek yarım günlük iş ve en yüksek getirili tek hamle. (2) B1 (D1) teknik olarak **hazır bir işe** dokunuyor — `wf.wfe_branch.c_a`/`.view_c_a` kolonları ve GIN index'leri zaten kol başına yazılıyor, eksik olan yalnız `branch_pool_sql()`in korelasyonlu süzgeci; migration yok, tek katman, geri alınabilir, ve kullanıcının cümlesinin ilk yarısını (`"listable'da yoksa göremezler"`) tam karşılıyor. (3) B2'ye girmemenin sebebi maliyet değil, **sıralama**: kol-yerel WFAH `count($wfah,…)` sözleşmesini değiştirir ve o sözleşme dört ayrı yerde kritik değişmez sayılmış; kol-yerel ctx ise `wf.wfe_dynctx`in `UNIQUE (wfe_id, seq)` yarış korumasını sökmek demek. Bu ikisi ancak bir belge-seviyesi bayrakla (`parallel.isolation`) güvenli hale gelir, o bayrak da ancak E bölümündeki 1-4 numaralı ürün kararları verildikten sonra tasarlanabilir. Ayrıca B2'nin en pahalı parçasının (kol-yerel ctx) ucuz bir yaklaşığı var — D2'nin ad alanı disiplini — ve kullanıcının gerçek ihtiyacının o yaklaşıkla karşılanıp karşılanmadığı **henüz sorulmadı**; sormadan derin işe girmek yanlış yönde birkaç hafta demektir.

---

## Yer imleri

| Dosya:satır | Ne var |
|---|---|
| `crates/wfe-core/src/v22/ports.rs:55-97` | `Wfes` — tek `dynctx`, tek `wfah`, `branches: Vec<BranchState>` |
| `crates/wfe-core/src/v22/ports.rs:32-51` | `BranchState`in TAM alan listesi (ctx/wfah YOK) |
| `crates/wfe-core/src/v22/ports.rs:112-129` | `rev()` = son WFAH seq; niye kolon değil |
| `crates/wfe-core/src/v22/ports.rs:260-288` | `TransitionCommit` — `new_dynctx: Value` (TEK), `branch_c_a`/`branch_view_c_a` |
| `crates/wfe-core/src/v22/pipeline.rs:588-811` | `apply_parallel` — WFE-seviyesi ctx okur/yazar, kol seçimi, marker'lar |
| `crates/wfe-core/src/v22/pipeline.rs:2633-2760` | Join tamamlanma ölçütü (All/Quorum/Expr), `WFD.JoinUnsatisfied` |
| `crates/wfe-core/src/v22/pipeline.rs:3044-3073` | `all_entry_nodes` / `arrived_entries_with` — kol kimliği = `entry_node` |
| `crates/wfe-core/src/v22/pipeline.rs:3075-3108` | Kol marker'ları sözleşmesi (`_fork`/`_branch_*`/`_collapse`/`_join`) |
| `crates/wfe-core/src/v22/pipeline.rs:1222-1295` | `possible_actions` — kol claim'i aktörde değilse BOŞ |
| `crates/wfe-core/src/v22/eval.rs:14-30` | `$wfah` izdüşümü `{seq, action, actor, input, at}` |
| `crates/wfe-core/src/v22/eval.rs:60-85` | `JoinEnv` → `$branches.*` (bool) + `$arrived` (dizi) |
| `crates/wfe-core/src/v22/eval.rs:117-120,165-180` | `with_wfah` süzgeçsiz; `$prev`/`$first` = birleşik listenin uçları |
| `crates/wfe-core/src/v22/effects.rs:47-61` | `apply_effects` — tam ctx klonu + `set_path` |
| `crates/wfe-core/src/v22/visibility.rs:150-249` | `can_view` (a)-(f); (c)/(f) TÜM aktif kolların node'larına bakar |
| `crates/wfe-core/src/validator.rs:1265-1280` | `check_duplicate_c_a` — "aynı havuzdan iki kol" yasağı |
| `crates/wfe-core/src/validator.rs:2014-2100` | Kol alt graf BFS'i — **ayrıklık garantisi** (kol atfının temeli) |
| `crates/wfe-core/src/expr_types.rs:40-58` | `WFAH_FIELDS` — editördeki `WFAH_FIELDS` ile aynı küme olmalı |
| `crates/wfe/src/wfe_adapter.rs:46-64` | `UNIQUE (wfe_id, seq)` = tek yarış koruması → `StaleRevision` |
| `crates/wfe/src/wfe_adapter.rs:483-528` | `commit` — ctx tam snapshot, seq = son WFAH seq |
| `crates/wfe/src/wfe_adapter.rs:623-742` | `ForkTo` / `BranchMoveTo` / `BranchArrived` / `JoinComplete` kolları |
| `crates/wfe/src/repo/dynctx.rs:6-15` | `load_latest` — `ORDER BY seq DESC LIMIT 1`, kol boyutu yok |
| `crates/wfe/src/visibility.rs:72-115` | `sql()` — kol EXISTS'i **korelasyonsuz**; `PARAM_COUNT = 6` |
| `crates/wfe/src/executor.rs:1039-1093` | `apply` retry döngüsü, `MAX_ATTEMPTS = 3`, `check_rev` |
| `crates/wfe/src/executor.rs:1302-1410` | `fill_view_grants` — kol `c_a`/`view_c_a`; kol projeksiyonunda `$node = None` |
| `crates/wfe/src/executor.rs:161-187` | `possible_actions_for` — aktif kollar üzerinden birleşim |
| `crates/wfe/src/executor.rs:325-388,510-516` | `WfeView` (tek `dynctx`, `branches[]`), `PathStep` (kol alanı YOK) |
| `crates/wfe/src/sim.rs:23,32,100,172` | `SimState` — tek `dynctx`, `branches` modellenmiş |
| `crates/wfe/src/reproject.rs:39-203` | Projeksiyonun tek yeniden-üretim yolu (kol kolonları dahil) |
| `crates/server/src/routes/portal/pool.rs:139-205` | İki havuz sorgusu; **`:177-180` "WFE görünüyorsa TÜM aktif kolları listelenir"** |
| `crates/server/src/routes/wfe.rs:1074-1108` | `ApplyBody` — `branch` (kol seçimi), `expected_rev` |
| `migrations/wf/20260521000001_initial.sql:33-41,43-53` | `wf.wfe_dynctx` ve `wf.wfah` — ikisi de `UNIQUE (wfe_id, seq)`, kol kolonu YOK |
| `migrations/wf/20260717000006_wfe_branch.sql:1-34` | `wf.wfe_branch` + `UNIQUE (wfe_id, branch_node)`; tasarım sözleşmesi başlıkta |
| `migrations/wf/20260731000002_join_expr.sql:29` | `entry_node` — kolun DEĞİŞMEZ kimliği |
| `migrations/wf/20260810000001_wfah_path.sql:10-24` | `from_node`/`to_node`; fork'ta `to_node` NULL, `WfahEntry`e alan EKLENMEDİ |
| `docs/spec/decisions.md:2015-2061` | 2026-08-14 `duplicate_c_a` kararı + FEDA EDİLEN ("aynı havuzdan iki kol") |
| `docs/spec/examples/paralel-onay.json` | 3 kollu AND-join örneği (finans/hukuk/İK → resultCoordinator) |
| `agnoflow-frontend/src/utils/validateParallelRules.ts:1-23,188-250` | Motorun `check_parallel` aynası; sözleşme yorumu |
| `agnoflow-frontend/src/utils/parallelScope.ts:1-13` | Kol interior BFS'i — **B2'nin editör altyapısı hazır** |
| `agnoflow-frontend/src/utils/whenFields.ts:8,203,340` | `WFAH_FIELDS` + `collectActionInputCtxMap`; `parallel`/`branch` kelimesi YOK |

---

## ÖLÇÜLEMEDİ

Aşağıdakiler bu raporda **iddia edilmemiştir**; ölçülmesi ayrı iş gerektirir.

1. **Cross-branch ctx görünürlüğünü kanıtlayan bir TEST yok.** `crates/wfe-core/tests/pipeline.rs`te "bir kol yazar, diğer kolun `when`i okur" senaryosu aranıp bulunamadı (`parallel_wfes` helper'ı `:2382` tek ctx alıyor). Kod yolu tek anlamlıdır ama davranış test altında kilitli DEĞİL — B1/B2 çalışılırsa ilk iş bu testi yazmak olmalı (mevcut davranışı sabitlemek için de).
2. **`expr_types` tip çıkarımının kol kapsamına duyarlı olup olması gerekip gerekmediği.** `#.input.<yol>` tipi `wfes_effects`ten çıkarılıyor; iki kol aynı yola farklı tip yazarsa bugün ne olduğu (hata mı, ilk/son kazanır mı) ölçülmedi.
3. **`filter_dynctx` (`x-visibility`) ile kol izolasyonunun etkileşimi.** Alan bazlı gizlilik B2'de "hangi kol görünümü üzerinde" koşacağı incelenmedi.
4. **WFC (`calls[].input`) kol bağlamı.** Kolda başlatılan alt akışa aktarılan ctx'in kol-yerel modelde ne olması gerektiği incelenmedi.
5. **Portal UI'ının bugün üç kol satırını nasıl gösterdiği** (aynı WFE için üç kart mı, gruplanmış mı) doğrudan gözlenmedi — yalnız sorgu şekli okundu (`pool.rs:181-205`, `ORDER BY e.created_at, br.branch_node`).
6. **Üretimde kaç WFE'nin paralel modda olduğu ve kaç akışın kol içinde `count($wfah,…)` kullandığı.** `visibility_report` deseninde bir ölçüm komutu yazılmadan D3/D4'ün gerçek kırılma yüzeyi bilinemez — karar öncesi yapılması önerilir.
7. **`join_when` (`JoinRule::Expr`) ifadelerinin `$wfah` kullanıp kullanmadığı.** Kullanıyorsa D3 join kuralını da etkiler; ölçülmedi.
