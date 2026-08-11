# v2.2 Migration — Alınan Kararlar (KARAR issue'ları)

Bu doküman v2.2 migration'ında alınan tasarım kararlarını kaydeder (main branch).
Linear referansları: WOR-24..WOR-30. İki repoda senkron tutulur
(kanonik kaynak: WFD-EDITOR/docs/spec/; kopya: workflow-engine/docs/spec/).

**Editör hassasiyetleri (2026-07-08, kullanıcı gereksinimi — kararları bağlar):**
Editör UI iyi seviyede, bozulmaz. (1) Aksiyonlar CaGroup altında ona bağlı kalır —
kullanıcıya gösterilmek istenen görsel budur. (2) Oklara (edge) anlam atfedilmez;
ok = bağlantı gösterimi + seçilince silme. (3) Görsel/UX yeniden tasarımı yasak.
Spec'in React Flow görsel eşlemesi (humanPool/when-edge) ile çelişince bu hassasiyet
kazanır; spec yalnız export edilen JSON formatını bağlar, editör görselini değil.

## WOR-24 — WFE DB şeması: `current_node`

**Karar:** `wf.wfe` tablosuna `current_node TEXT NULL` kolonu eklendi.

- `NULL` = WFE terminal (node'da beklemiyor). Aktif WFE'de her zaman dolu.
- `current_c_a` JSONB kolonu KALDIRILMADI ama artık türetilmiş veri: current_node'un
  c_a'sının resolve edilmiş hali okuma-yolu (pool listing) için denormalize edilir.
- `claimed_by` (mevcut kolon) assignment'tır: node değişiminde `NULL`'a resetlenir
  (M8 — yeni node'a UNASSIGNED giriş). Claim, current node c_a match'i ile yapılır (§7.1).

## WOR-25 — `WftRule::Parallel` kaldırıldı

**Karar:** v2.2 WFT formları yalnızca `{node}` / `{terminal}` / `{conditions, default?}`.
`Parallel` variant'ı ve editördeki `parallel_wft` tamamen silindi. WOR-8'deki
"join_when parse ediliyor ama kullanılmıyor" bug'ı böylece kökten kapanır
(kod yolu artık yok).

## WOR-27 — Kök `WFD-Specification.json`

**Karar:** Engine reposunda kanonik spec `docs/spec/` altına kopyalandı.
CI ve kabul testleri sibling repoya (WFD-EDITOR) değil, repo-içi kopyaya bağlıdır.
Spec güncellendiğinde iki repo senkronize edilir; kanonik kaynak WFD-EDITOR/docs/spec.

**Kök dosya:** `WFD-EDITOR/WFD-Specification.json` stale (deprecated `_step_*`,
`terminal:true`, c_a içi `from`, 2 kurallı `start.c_a`). Golden fixture
(`docs/spec/examples/kredi-basvuru.golden.json`) zaten kanonik örnek olduğundan
ikinci referans drift üretir → dosya `docs/legacy/` altına arşivlenmişti.
2 kurallı `start.c_a` dosyayla birlikte ölür; ayrıca modellenmesi gerekmiyor.

**Güncelleme (2026-07-28):** `docs/legacy/` arşivi her iki repodan tamamen
KALDIRILDI — arşiv kopyaları (eski `Terminology-*`, `WFD-Specification.json`,
`CLAUDE-*` snapshot'ları) tam da önlemek istenen drift'i üretiyordu. Tarihsel
referans git geçmişindedir; çalışan ağaçta tek spec kaynağı `docs/spec/`'tir.

## WOR-28 — Eski seeded WFD fixture'ları

**Karar:** Eski v2 formatındaki seed'ler (`seed_kart_limiti_artisi`, `seed_kredi_basvuru`)
v2.2 loader tarafından REDDEDİLİR (`wfd_version` yok). Yeni migration eski seed
satırlarını siler ve golden fixture'ı (kredi-basvuru-v2) v2.2 seed'i olarak ekler.
Kart-limiti akışının v2.2'ye çevirisi ayrı işe bırakıldı — çok elemanlı c_a içeriyorsa
İNSAN ONAYI gerekir (M10).

## WOR-26 — Editör canvas'ında autoexec temsili

**Karar (Seçenek B):** Autoexec canvas'ta **mevcut görsel node** olarak kalır
(`AutoexecStepNode`, inline config görünümü). Export'ta v2.2 root `autoexec` kataloğu
+ transition `trigger[]` listesine düzleşir; import'ta yeniden çizilir. Görsel↔veri
eşlemesi `useGraphNodes` (render) ve `useExport` (serialize) katmanlarında yapılır.

- Gerekçe: config'i edge property'sine taşıyan Seçenek A, "oklara anlam atfedilmez"
  hassasiyetine aykırı — reddedildi. Trigger/retry/catch/timeout düzenlemesi autoexec
  node'u seçilince PropertiesPanel'de yapılır (edge seçerek DEĞİL).
- In-progress node-tabanlı işler (WOR-20/21/22) bu kararla uyumlu; iptal gerekmez.

## WOR-29 — `caGroupDisplay` çoklu-kural gösterimi

**Karar (Seçenek A):** Çoklu-kural display yolu ve testi silinir; `formatCaHeader`
tek `CandidateActor` objesi alır. c_a tek kurala inince (`c_a: CandidateActor`) tip
sistemi bunu zaten zorlar. Tek kural + çoklu rol (`c_r: string[]`) GEÇERLİ kalır
(noktalı virgülle çoklu *kural* birleştirme kalkar, çoklu *rol* gösterimi kalır).
Çoklu bağımsız grant gerekirse v2.2 yolu: ayrı `listable[]` kayıtları (ayrı formatter).

## WOR-30 — Editör `x_editor` embed'i vs `additionalProperties:false`

**Karar (Seçenek A/C birleşimi — layout sidecar):** `x_editor` root-embed'i kalkar.
Engine'e giden export her zaman temiz, şema-valid v2.2'dir. Editör layout'u ayrı
**sidecar**'da saklanır: `slug → {x, y}` pozisyonları + node görünüm tipleri
(CaGroup / aksiyon / switch / autoexec). Node key'leri deterministik slug olduğundan
sidecar dokümandan bağımsız eşlenebilir.

- Import: sidecar varsa oku (görsel birebir geri gelir); yoksa ELK auto-layout.
- Round-trip (`import(export(x)) == x`) hem doküman hem görsel yerleşimi korur.
- Gerekçe: tek temiz format + kullanıcının elle düzenlediği görsel korunur.

## Simetrik Start (v2.2 — `from` + `action:"start"`)

**Karar:** v2.2 `start` `transitions` ile simetrik hale getirildi: `{ id, from, action:"start", wfes_effects?, trigger?, wft }`. `c_a` startRule'dan kaldırıldı; artık `start[].from` ile referans edilen bir `nodes` girdisinde durur. Start-node kimliği `start[].from` referansından TÜRETİLİR — node'a ayrı bir `kind` alanı eklenmedi (dual source-of-truth riski nedeniyle reddedildi). `wfd_version` yerinde kaldı (`"2.2"`); v2.3'e geçilmedi çünkü v2.2 henüz aktif geliştirmede, dış tüketici yok, tek örnek dosya var.

- Eski (bu değişiklikten önceki v2.2): `startRule = { id, c_a(inline), wfes_effects?, trigger?, wft }` — `from`/`action` yoktu, `c_a` node'a değil doğrudan startRule'a bağlıydı.
- Yeni: `c_a` node'da; `startRule.c_a` alanı SİLİNDİ.

## Start input = action input sözleşmesi (2026-07-14)

> **Kısmen GEÇERSİZ (WOR-70 + WOR-71, 2026-07-29):** bu maddede tarif edilen "başlangıç
> ctx'i declared yollardan tohumlanır" ve "`context.required` FINAL ctx üzerinde
> denetlenir" davranışları kaldırıldı (WOR-70); aşağıdaki `x-wf-readonly` denetimleri de
> uzantıyla birlikte tamamen kaldırıldı (WOR-71). Input artık ctx'e hiç yazmaz;
> `context.required` ve `x-wf-readonly` yoktur. Geçerli sözleşme: bu dokümanın sonundaki
> **WOR-70** ve **WOR-71** maddeleri.

**Sorun:** Start, gelen input objesini olduğu gibi başlangıç ctx'i yapıyordu; start
aksiyonunun `input.required/optional` bildirimi runtime'da hiç doğrulanmıyordu
(yalnız `context.required` + top-level `x-wf-readonly` bakılıyordu). Sonuç: (a) portal,
hangi alanların kullanıcıdan isteneceğini context şemasından tahmin etmek zorundaydı;
(b) API'ye doğrudan istek atan bir istemci, readonly işaretlenmemiş HER context alanını
start'ta enjekte edebiliyordu (ör. `durum: "onaylandi"`).

**Karar:** Start input'u transition input'larıyla (§7.5) simetrik doğrulanır:
- `input.required` eksikse `zorunlu input 'x' eksik`;
- bildirilmemiş leaf yol **hard reject** (`input yolu 'x' bu action'da tanımlı değil`) —
  sessiz düşürme bilinçli reddedildi (hatalı istemci sessizce yanlış çalışmasın);
- başlangıç ctx'i = yalnız declared yollar + start rule effects (serbest-form seed kalktı);
- `x-wf-readonly` denetimi declared yolların TÜM segmentlerinde (eskisi top-level'dı);
- `context.required` denetimi start zinciri bittikten sonra FINAL ctx üzerinde,
  noktalı yol destekli (alanlar `$action.input.X` effect'iyle de yazılabildiği için
  input-öncesi kontrol yanlış katmandaydı). Hata metni değişti:
  `context zorunlu alanı 'x' start sonrasında eksik`.

Golden fixture bu spec değişikliği gereği güncellendi: `create_application.input.required
= ["applicant", "credit_info"]` (start aksiyonu artık kabul ettiği girdiyi bildirmek
ZORUNDA — boş bildirim "start input almaz" demektir). Tüketici sözleşmesi: portal start
formları context şemasından değil, start aksiyonunun `input` tanımından üretilir
(work-pool-portal `WorkflowsPage` buna geçirildi).

## WOR-31 — Parallel fork/join yeniden tasarlandı

**Not:** Spec kaynağı olan WFD-EDITOR reposu `docs/spec/`'inin bu değişiklikle
senkronize edilmesi gerekir (follow-up iş).

**Karar:** WOR-25'in kaldırdığı `WftRule::Parallel` **yeniden eklendi** — ama eski
tasarım değil, sıfırdan: eski `Parallel` bir `join_when` ifadesi parse edip hiç
kullanmıyordu (WOR-8 bug'ı); yeni tasarımda `join_when` YOK, join tamamen
**deklaratif bir hedeftir** (`{node}` veya `{terminal}`), engine kendisi
"son kol vardı mı" sayar — evaluate edilecek bir ifade yok, dolayısıyla o bug
sınıfı kökten kapanır.

**Şekil (WFT'nin 4. formu):**
```json
{"parallel": {"branches": ["node-a", "node-b"], "join": {"node": "x"}}}
```
`branches` paralel kollara giriş node id'leridir (≥2, distinct). `join`,
`{node}` veya `{terminal}` — AND-join hedefi.

**Çalışma zamanı semantiği (kısa özet — tam akış T2'de kodlanacak):**
- Fork: bir transition'ın `wft`'i Parallel'e çözülünce WFE paralel moda girer;
  her branch için ayrı bir "branch token" oluşur, her biri kendi branch
  başlangıç node'undadır. `wfe.current_node` paralel modda `NULL`'dır.
- Kol hareketi: bir kol normal bir node'a geçerse sadece o kolun token'ı hareket
  eder (paralel mod devam eder).
- Kol join'e varış: bir kolun `wft`'i join node'una eşit bir hedefe çözülürse o
  kol **join node'a taşınmaz**, `arrived` işaretlenir. SON aktif kol vardığında
  WFE paralel moddan çıkar: `current_node = join node` (join hedefi terminal ise
  WFE o terminal'de biter). AND-join: cancel edilmemiş TÜM kolların varması
  gerekir.
- Kol terminal'e varış: bir kolun `wft`'i bir terminal'e çözülürse (ör. red
  transition'ı) TÜM WFE o terminal'de biter; diğer TÜM kollar `cancelled`
  olur (+ `_branch_cancelled` wfah kaydı). Bu, "bir kol reddederse akış durur"
  semantiğinin modelidir — red, bir terminal'e transition olarak modellenir.
- SLA/Fail/Terminated: WFE deadline'ı dolarsa veya trigger fail ederse tüm
  kollar cancel edilir. Escalation/claim_timeout paralel modda **kol-bazlı**
  hale gelir (her kolun kendi `claimed_by`/`claimed_at`/`entered_at`'ı vardır).
- wfah system marker'ları: `_fork`, `_branch_arrived`, `_join`,
  `_branch_cancelled`, `_branch_superseded`, `_collapse` (aynı paylaşımlı seq counter).

**Kısıtlar (v1, validator-enforced — `wfe-core/src/validator.rs::check_parallel`):**
- Parallel wft `start[].wft`'te YASAK.
- `branches` ≥2 ve distinct; her branch var olan bir node'a referans vermeli.
- `join` var olan bir node/terminal'e referans vermeli; kollardan biriyle AYNI
  OLAMAZ.
- Nested fork YASAK: bir branch subgraph'ı (fork'tan join/terminal'e kadar,
  transition `wft` kenarları izlenerek) içinde ikinci bir Parallel bulunamaz.
- Branch subgraph'ları birbirinden AYRIK (disjoint node set) olmalı.
- Her branch subgraph'ı join node'a veya bir terminal'e ulaşabilmeli (dead-end
  yasak).
- `check_graph` (§5, BFS reachability) Parallel'i de kenar kaynağı sayar: fork
  node → her branch + fork → join hedefi.

**Golden fixture (`examples/kredi-basvuru.golden.json`) DEĞİŞMEDİ** — WOR-31
ayrı bir fixture ile örneklenir: `docs/spec/examples/paralel-onay.json`
(satın alma onayı: review node'undan finans/hukuk/ik kollarına fork, her kolda
bir onay node'u — approve → join, reject → red terminal — join'de sonuç node'u
ve nihai transition ile başarı terminal'i).

## WOR-56 — Paralel dalda "sonlandıran aksiyon" (collapse + goto, rastgele hedef)

**Bağlam:** WOR-31'de bir kolun terminal'e transition'ı tüm WFE'yi o terminal'de
bitirir ("red" senaryosu). Genelleştirme ihtiyacı: bir kol aksiyonu paraleli
bitirip **rastgele bir node'a** (terminal ŞART DEĞİL) gidebilmeli. Kanonik örnek:
kredi için 3 ayrı c_a'dan review beklenirken bir tanesi review yerine
`başa_gonder` alır → paralel biter, WFE başvuru node'una döner. Bu aksiyon **kola
üyedir ama join'e bağlanmaz** ve hedefi paralel kapsamı dışındadır.

**Karar — `collapse` transition (explicit, topolojiden türetilmez):**

Salt topoloji yetmez: "kola üye + join'e bağlanmaz" iki ayrı şeyi kapsar —
(1) çok-seviye koldaki **interior** aksiyon (kapsam içinde kalır, devam eder),
(2) **collapse** aksiyonu (kapsamdan çıkar, kardeşleri düşürür). İkisinin de
hedefi rastgele node olabildiğinden ayrım ancak **explicit işaretle** yapılır.

- **Wire (LANDED):** collapse transition'ının `wft`'i `{"collapse": {node|terminal}}`
  sarmalayıcısıdır — `Wft` enum'unun 5. formu (`wfd_v22.rs`; şemada `wftCollapse`,
  `wft` oneOf'una eklendi). `Wft::Node`'dan AYRIDIR: normal node hedefi kolu
  ilerletir (BranchMoveTo), collapse-node hedefi paralel modu bitirir. Editör ayrıca
  `collapsesParallel` sidecar bayrağını (`isReject` gibi, engine şemasına gitmez)
  UI için taşır; export bu bayrakla wft'yi collapse sarmalayıcısına alır, import
  hem `wft.collapse` hem `== entry` sentinel'iyle bayrağı geri kurar.

**Çalışma zamanı semantiği (WOR-31 collapse kuralının genişletilmesi):**
- Kol collapse aksiyonu alınca: TÜM diğer aktif kollar `cancelled`
  (+ `_branch_cancelled` wfah). WFE paralel moddan çıkar.
- Hedef `{node}` ise `current_node = hedef` (paralel-dışı normal node);
  `{terminal}` ise WFE o terminal'de biter (= WOR-31'in mevcut red davranışı,
  artık genel kuralın özel hali).
- WOR-31'deki "kol terminal'e varış" maddesi bunun terminal-hedefli örneğidir;
  collapse hedefi artık node OLABİLİR.

**Validator (WOR-31 kısıtlarının collapse istisnası):**
- Collapse transition'ının hedefi **disjoint branch-subgraph** ve "join'e/terminal'e
  ulaş" (dead-end) kurallarından **muaf** — BFS (`check_parallel` / editörde
  `validateParallelRules.ts`) collapse kenarını **izlemez** (kapsamı büyütmez).
- Collapse hedefi paralel dışındaki normal grafın parçasıysa `check_graph`
  reachability onu ayrı bir kenar kaynağı sayar.
- Collapse transition'ı yalnızca bir branch-subgraph node'undan çıkabilir
  (paralel-dışı node'da collapse anlamsız → hata).

**Görsel gösterim (editör, keşfedilebilir):**
- Collapse işaretli aksiyon node'unda `⊗` rozet + çıkan kenar ayrı stil
  (uyarı rengi/kalın) + etiket ("paraleli bitirir → <hedef>"). Hedef nereye
  giderse gitsin görünür; join bracket'ine girmez.
- PropertiesPanel'de tek satır uyarı: "Bu aksiyon paraleli sonlandırır; diğer
  kollar iptal olur."

**Auto-when — ters polarite (KRİTİK, "kapsam dışı" DEĞİL):**

Collapse aksiyonu auto-when'den muaf tutulmaz; **ters polariteli** bir auto-when
alır. Gerekçe: collapse aksiyonunun node'u paralel dışından da girilebiliyorsa,
o node paralel-dışıyken bu aksiyon **alınamamalı** (paralel yokken "paraleli
bitir" anlamsız) → bir `when` gate şarttır.

- **Independent** interior aksiyon (WOR-31, mevcut): paralel bağlamdayken
  **gizle** → `$prev.action != "<entry>"`.
- **Collapse** aksiyonu (WOR-56): yalnızca paralel bağlamdayken **göster** →
  `$prev.action == "<entry>"`. Tam ters operatör, **aynı**
  entry-action hesabı (interior BFS: direkt dal node'unda entry=fork; derin
  node'da entry=oraya götüren dal aksiyonu; birden çok entry → OR:
  `==e1 or ==e2`).
- Kullanıcı kendi when'ini yazarsa `(user) and (auto ==)` düz top-level AND.
- Neden aynı `entry` sinyali: motor ZEN'e paralel-durumunu açmaz
  (`current_node=NULL`, kollar `wfe_branch` tablosunda); ZEN yalnızca
  DynCtx + WFAH görür → "paralel içindeyim" ancak "önceki aksiyon == entry"
  ile türetilir. Independent bunun `!=`'ini, collapse `==`'ini kullanır.
- Import round-trip: sentinel regex (`wfdImport.ts`) `!=` VE `==` varyantlarını
  yakalamalı; strip mantığı aynı (her clause ayrı top-level terim). **WOR-84**: sentinel
  `$wfah[len($wfah) - 1].action`'dan `$prev.action`'a taşındı (eski form boş geçmişte
  VM'i patlatıyordu) — regex İKİ formu da tanır, export yalnız `$prev` yazar.

**Durum (LANDED, uçtan uca):** editör (bayrak + görsel + export/import + validator)
VE engine (`Wft::Collapse` + `CommitOutcome::CollapseTo` runtime: cancel_active_branches
+ current_node=hedef + join_target NULL; validator: start-ban + BFS skip; sim + adapter)
tamamlandı. Testler: editör `parallel.joinin.test.ts` (collapse export/import/şema/validator),
engine `pipeline.rs::branch_collapse_to_node_ends_parallel_and_moves_wfe` +
`collapse_outside_parallel_is_rejected`. WOR-31 collapse kuralının genelleştirilmesi.

## WOR-60 — Geçersizleşen kol onayı: yeni statü DEĞİL, marker (2026-07-20)

**Sorun:** Bir kol join'e varıp `arrived` olduktan sonra kardeş bir kol collapse/
terminate ederse, o onayın geçersizleştiği hiçbir yere yazılmıyordu.
`cancel_active_branches` yalnız `status = 'active'` satırları vurur, marker döngüsü
de yalnız aktif kardeşleri gezerdi. Onay WFAH'ta duruyor ama "artık hükümsüz"
bilgisi yok — onaylanmış kol yan etki (kayıt açma, mail) üretmiş olabileceği için
enterprise audit'te kritik.

**Karar:** Kol satırının statüsü `arrived` OLARAK KALIR. `wf.wfe_branch` CHECK
constraint'i (`active|arrived|cancelled`, migration `20260717000006`) değişmez;
`superseded` diye yeni bir statü EKLENMEDİ. Gerekçe: statü, kolun join'e karşı
konumunu anlatır ve o konum gerçekten "vardı"dır — geçersizleşme kolun kendi
durumu değil, WFE'nin başına gelen bir OLAYdır. Olay WFAH'a yazılır:

- Aktif kol → `_branch_cancelled {node, reason, claimed_by, claimed_at}` (WOR-59)
- Arrived kol → `_branch_superseded {node, reason, approved_by, approved_at}`

İki marker net ayrılır; portal "yarıda kalan iş" ile "boşa giden onay"ı ayırt eder.
`reason` her iki markerda da AYNI değeri taşır (collapse yolunda `collapsed`,
diğerlerinde `sibling_terminal` / `failed` / `terminated`) — tetikleyen olay tek.

**approved_by nereden geliyor:** `mark_branch_arrived` varışta kolun claim'ini
düşürür, yani onaylayan runtime state'te DURMAZ. Bu yüzden `_branch_arrived`
marker'ı `approved_by` (tam Actor) + `approved_at` alanlarıyla zenginleştirildi ve
`_branch_superseded` bu kaydı WFAH'tan geri okur. WOR-60 öncesi yazılmış
`_branch_arrived` kayıtlarında alanlar yoktur → marker yine üretilir, alanları
`null` kalır (geriye dönük kırılma yok).

## WOR-61 — Collapse özet kaydı: `_collapse` (2026-07-20)

Collapse anında "ne olduğu" tek yerden okunamıyordu; audit WFAH'a kol-başına
dağılmış marker'lar hâlindeydi. Her collapse için TEK bir manşet kaydı eklendi:

`_collapse {trigger_branch, trigger_action, trigger_actor, kind, reason, target,
cancelled[], superseded[]}`

- Kol-başına detay marker'ları (`_branch_cancelled` / `_branch_superseded`) KALIR —
  özet onların yerine değil ÜSTÜNE geçer.
- Özet, detay marker'larından ÖNCE yazılır (seq olarak manşet önce gelir).
- `kind`: `collapse_to` | `terminal` | `failed` | `terminated` — hangi yolun
  collapse'ı tetiklediği. `reason` ise kol marker'larındaki değerle aynıdır
  (`collapsed` / `sibling_terminal` / `failed` / `terminated`).
- `target`: yalnız node hedefli collapse'ta node slug'ı; terminal yollarında
  `null` (akış bir node'a gitmez, sonuç `wfe.end_response`'tadır).
- `trigger_branch`: SLA-3 deadline gibi WFE-geneli yollarda `null` (tek bir
  koldan tetiklenmez).

## WOR-63 — Kol marker'larında tetikleyici bağlam (2026-07-20)

`_branch_cancelled` / `_branch_superseded` marker'larının `reason` alanı sabit ve
dar bir string'di (`collapsed` / `sibling_terminal` / `failed` / `terminated`);
collapse'ı hangi kolun, hangi aksiyonun, hangi actor'ün tetiklediğini taşımıyordu.

**Karar:** `reason` alanı AYNEN korunur (geriye dönük kırılma yok); tetikleyici
bağlam YALNIZCA ek alanlarla verilir:

- `trigger_node` — tetikleyen kol node'u (WFE-geneli yollarda `null`)
- `trigger_action` — tetikleyen aksiyon adı; sistem yollarında ilgili sistem
  marker'ının adı (`timeout:deadline`, `escalate:<node>:<idx>`,
  `claim_timeout:<node>`) — insan ve sistem tetikleri aynı alandan okunur
- `trigger_actor` — tam Actor objesi (`{orgu_id, user_id, role}`); sistem
  yollarında `role: "system"`, id'ler nil UUID

**İsimlendirme notu:** `_collapse` özetinde aynı bilgi `trigger_branch` adıyla
durur (WOR-61 sözleşmesi); kol marker'larında `trigger_node`'dur, çünkü o
kayıtlarda `node` zaten ETKİLENEN kolu gösterir ve iki alan ayırt edilmelidir.

## WOR-62 — CollapseTo yarış serileştirmesi + `conflict.*` hata kodları (2026-07-20)

`CollapseTo` "otoriter" kabul edildiği için commit'inde ne `SELECT ... FOR UPDATE`
ne de bir CAS vardı. Bir kol collapse ederken eşzamanlı bir kardeş kolda
apply/varış koşarsa iki işlem serialize edilmiyordu: kaybeden kardeşin akıbeti
tanımsızdı (sessizce yutulabilir veya tutarsız ara durum bırakabilirdi).

**Karar 1 — collapse otoriter KALIR, ama serileştirilir.** Kol-arrival SAYIMI
eklenmedi (collapse hâlâ kalan aktif kolları beklemez). Eklenen: commit tx'inin
başında `SELECT ... FOR UPDATE` + kilit ALTINDA "hâlâ paralel modda mıyım"
(`join_target IS NOT NULL`) doğrulaması. Kural tek ve deterministiktir:
**ilk kilidi alan kazanır**; kilidi sonra alan taraf paralel modun bittiğini
görür ve `Conflict(Collapsed)` alır. Aynı kapı `BranchMoveTo`, `BranchArrived`
ve `JoinComplete` için de geçerlidir — yani "kardeş collapse etti" durumu artık
kol CAS'ının belirsiz 0-satır sonucuna değil, NET bir koda düşer.

**Karar 2 — kilit sırası sözleşmesi: `wf.wfe` → `wf.wfe_branch`.** Kol satırlarına
dokunan HER commit yolu önce wfe satırını kilitler. `Terminal`/`Failed`/`Terminated`
arm'ları kolları wfe kilidini almadan güncelliyordu (ters sıra) — deadlock
potansiyeli; bu yollara da `lock_wfe` eklendi. Yeni bir kilit stratejisi
icat EDİLMEDİ, mevcut `JoinComplete` kalıbı genelleştirildi.

**Karar 3 — `EngineError::Conflict` artık sebep taşır: `Conflict(ConflictKind)`.**
Portal'ın "ne oldu" ayrımını hata METNİNİ parse etmeden yapabilmesi için. Kodlar
API sözleşmesinin parçasıdır (`wfe-core/src/error.rs`):

| `ConflictKind` | wire kod | retry? | anlamı |
|---|---|---|---|
| `Collapsed` | `conflict.collapsed` | hayır | paralel mod bitti (kardeş collapse/join/terminal kazandı) |
| `BranchMoved` | `conflict.branch_moved` | evet | kol node'u değişti / kol artık `active` değil |
| `BranchArrival` | `conflict.branch_arrival` | evet | kol-varış sayımı engine görüşüyle uyuşmadı |
| `WfeGone` | `conflict.wfe_gone` | hayır | wfe satırı yok / tenant uyuşmuyor |
| `AlreadyClaimed` | `conflict.already_claimed` | hayır | claim CAS'ı kaybedildi |
| `StaleRevision` | `conflict.stale_revision` | evet\* | revizyon token'ı eskimiş (WOR-65'te devreye alındı) |

\* `StaleRevision`'ın retry'ı yalnız ÖRTÜK yol (seq çakışması) içindir; istemci
`expected_rev` gönderdiyse retry edilmez. Bkz. WOR-65 Karar 4.

**Karar 4 — her Conflict retry edilmez.** `WfeExecutor::apply` retry döngüsü
(MAX_ATTEMPTS = 3, WOR-31'den değişmedi) artık `ConflictKind::is_retryable()`'a
bakar. `Collapsed` kalıcı bir durum geçişidir: reload aynı verdikti üretir, üstelik
3 tur sonunda KEYFİ bir engine hatası (`TransitionNotFound` / `AmbiguousAction` /
`PermissionDenied` — collapse hedefine göre değişir) dönerdi. Retry edilmez,
conflict aynen yukarı verilir.

**Karar 5 — 409 gövdesi `code` alanı kazandı (additive).** `AppError` üçüncü bir
`code: Option<&'static str>` alanı taşır; gövde:

```json
{ "error": "optimistic concurrency conflict [conflict.collapsed]: state changed under commit",
  "code": "conflict.collapsed" }
```

`error` alanı DEĞİŞMEDİ (geriye uyumlu); `code` yalnızca eklenir ve yalnızca
409 sınıfında doldurulur. `ConflictKind` dışındaki 409'lar da aynı namespace'e
girer: `conflict.ambiguous_action` (`AmbiguousAction`), `conflict.terminal`
(`WfeTerminal`), `conflict.expired` (`WfeExpired`).

**WOR-65 için bırakılan kancalar (hepsi kullanıldı — bkz. bir sonraki bölüm):**
revizyon token'ı + stale-write koruması yeni bir hata TİPİ açmadı,
`ConflictKind::StaleRevision` kullanıldı ve `From<EngineError> for AppError`
gerçekten değişmedi (kod `kind.code()`'tan gelir). Claim yarışının bugünkü yolu
409 DEĞİLDİR (`ClaimOutcome { success: false, reason: "already_claimed" }`,
HTTP 200) ve WOR-65'te de 409'a TAŞINMADI.

## WOR-65 — WFE revizyon token'ı + stale-write reddi (2026-07-20)

`work-pool-portal`'ın motorla tek senkron kanalı polling'dir (`refetchInterval: 4000ms`,
`staleTime: 10000ms`; WebSocket/SSE/webhook YOK). Motor durumu "sessizce" değiştirir —
en keskini collapse: paralel bir kolda sonlandıran aksiyon alınınca kardeş kollar
`cancelled` olur, WFE hedefe gider. `claimWfe`/`applyAction` hiçbir revizyon taşımadığı
için portal "altımdan durum değişti"yi tespit edemiyordu: collapse'tan sonra ~4-10s
boyunca kullanıcı hâlâ Claim/Apply gönderebiliyor, tek backstop motorun reddiydi ve o
red ayırt edilemiyordu (hangi node'a collapse edildiğine göre `TransitionNotFound` /
`AmbiguousAction` / `PermissionDenied` — hepsi jenerik toast).

**Karar 1 — revizyon token'ı = son WFAH `seq`'i. Yeni kolon/migration YOK.**
Değerlendirilen seçenekler ve gerekçe:

| Seçenek | Karar | Neden |
|---|---|---|
| `updated_at timestamptz` | RED | timestamptz precision/çakışma riski; aynı tx içinde `now()` sabittir, iki commit ayırt edilemeyebilir. Semantik olarak da "revizyon" değil. |
| yeni `rev integer` kolonu | RED | aynı gerçeği ikinci kez saklar + senkron tutma yükü; aşağıdaki gözlem karşısında gereksiz. |
| **WFAH `seq` (SEÇİLDİ)** | ✅ | `wf.wfah` zaten WFE başına monotonik, `UNIQUE (wfe_id, seq)` ile korunan bir sayaç tutar; her transition ≥1 kayıt yazar ve `commit` tek transaction'dır. |

Kodda doğrulandı (dogma olarak kabul EDİLMEDİ): `Engine::apply`/`start` ve escalation/
timeout yolları seq'i `wfes.wfah.entries().last().seq + 1`'den başlatır ve her yol en az
bir kayıt push eder — yani `Wfes::rev()` (son seq, WFAH boşsa 0) gerçekten monotonik bir
revizyon sayacıdır.

**Karar 2 — kapsam istisnası: `claim` revizyonu ARTIRMAZ (bilinçli).**
`WfeStore::claim` `commit()`'ten geçmez; saf bir CAS UPDATE'tir ve WFAH'a yazmaz.
Dolayısıyla seq tabanlı revizyon yalnız *transition*'ları kapsar. Bu bölünme kabul
edildi, çünkü claim yarışı zaten kendi CAS'ıyla (`claimed_by IS NULL`) çözülüyor ve
claim'i WFAH'a yazdırmak audit izini kirletir + `$wfah` ZEN namespace'inin anlamını
değiştirirdi (spec etkisi olurdu). `release_claim` ise WFAH marker'ı yazdığı için
revizyonu ARTIRIR — asimetrik ama doğru: claim salt atama, release gözlemlenebilir bir
SLA olayıdır.

**Karar 3 — wire biçimi: gövde alanı `expected_rev` (tamsayı), `If-Match` başlığı DEĞİL.**
Token opak bir entity-tag değil düz bir tamsayıdır; `If-Match`'in weak/strong
karşılaştırma, `*` ve liste semantiğinin yarısını uygulamak sözleşmeyi yanıltıcı
kılardı. Ayrıca ilgili endpoint'lerin gövdesinde zaten opsiyonel alanlar var (`node`) —
token da aynı yerde, aynı tipte durur. **OPSİYONEL**: göndermeyen istemci bugünkü
davranışı birebir görür.

Okuma tarafı (revizyon AÇIKÇA döner — liste endpoint'lerinde `wfah` YOKTUR, portal'ın
türetebileceği başka alan yok):

| Endpoint | Alan |
|---|---|
| `GET /wfe/:id` | `rev` (kök) |
| `GET /wfe` | satır başına `rev` |
| `GET /portal/wfe/:id` | `rev` (kök) |
| `GET /portal/pool` | satır başına `rev` |

Yazma tarafı: `POST /wfe/:id/actions`, `POST /wfe/:id/claim`,
`POST /portal/wfe/:id/action`, `POST /portal/pool/:id/claim` → gövdede `expected_rev`.

Revizyon **WFE-seviyesidir**: paralel modda tüm kollar aynı `rev`'i taşır (kol-bazlı
revizyon YOKTUR). Havuz listesinde aynı WFE'nin farklı kolları için üretilen satırlar
bu yüzden aynı `rev`'i gösterir.

**Karar 4 — iki katmanlı koruma; sunucu-içi retry ile istemciye dönen 409 ayrımı.**

*Açık katman:* `expected_rev` verilmişse `WfeExecutor::apply`/`claim` engine'i
koşturmadan ÖNCE `Wfes::rev()` ile karşılaştırır; uyuşmazlıkta hiçbir yan etki
üretmeden `Conflict(StaleRevision)`. Bu kontrol retry döngüsünün İÇİNDEDİR ama `?` ile
erken döner — yani **sunucu-içi retry yapılmaz**. Gerekçe: reload aynı uyuşmazlığı
üretir, 3 tur sadece gecikmeyi üçe katlardı. If-Match semantiği de bunu gerektirir:
durum değiştiyse aksiyon SESSİZCE uygulanmamalıdır.

*Örtük katman:* `UNIQUE (wfe_id, seq)` ihlali (Postgres 23505) artık `WfePort` (→ 500)
değil `Conflict(StaleRevision)`'a eşlenir (`wfe_adapter.rs::insert_err`). Bu, token
GÖNDERMEYEN istemciler için de lost-update korumasıdır ve özellikle **tekil
(paralel-olmayan) modda tek yarış korumasıdır**: `CommitOutcome::MoveTo` yolunda ne
`FOR UPDATE` ne de CAS vardır. Bu katman retry-EDİLEBİLİR (`is_retryable() == true`):
reload taze seq verir, aksiyon meşru biçimde uygulanabilir.

İkisi birlikte tutarlıdır: istemci `expected_rev` gönderdiyse örtük katmanın retry'ı
tek turda biter, çünkü reload sonrası açık kontrol uymaz ve `StaleRevision` döner.

**Karar 5 — claim akışı token'sız istemci için DEĞİŞMEDİ.** `expected_rev` yokken claim
bugünkü gibi HTTP 200 + `ClaimOutcome { success: false, reason: "already_claimed" }`
döner; 409'a taşınMADI (portal'ın mevcut claim akışını kırardı). `expected_rev` VARSA ve
eskimişse `Conflict(StaleRevision)` → 409. Yani portal, claim'i 409'a taşımadan, yalnız
token göndererek "listede gördüğüm satır artık geçersiz" durumunu ayırt edebilir —
yanıltıcı `already_claimed` / `not_eligible` gerekçesi yerine.

409 gövdesi WOR-62'deki şekliyle aynıdır, yeni hata TİPİ açılmadı:

```json
{ "error": "optimistic concurrency conflict [conflict.stale_revision]: state changed under commit",
  "code": "conflict.stale_revision" }
```

## WOR-67 — Acting (collapse'ı tetikleyen) kolun düşen claim'i: `_collapse` manşetine (2026-07-20)

**Sorun:** WOR-59 kardeş kolların düşen claim'ini (`claimed_by`/`claimed_at`)
`_branch_cancelled` marker'ına yazdı. Ama collapse'ı TETİKLEYEN kol (`acting_branch`)
marker döngüsünden dışlanır (`stage_parallel_markers`, gerekçe: aksiyon kaydı zaten
WFAH'ta, ikinci "iptal edildi" kaydı gürültü). Oysa adapter (`cancel_active_branches`)
acting kol için istisna yapmaz — onun da claim'i DB'de NULL'lanır. Sonuç: acting kol
DB'de `cancelled` + claim düşmüş ama düşen claim'i kaydeden hiçbir marker yok. Audit
asimetrisi: "reddeden kişi claim'i ne kadar tuttu" collapse anında kaybolur.

**Karar — (a′): claim `_collapse` manşetine, AYRI MARKER YOK.** Değerlendirilen yollar:

| Seçenek | Karar | Neden |
|---|---|---|
| (b) acting için de `_branch_cancelled` yaz | RED | aynı olaya iki kayıt (aksiyon + iptal); dışlamanın orijinal gerekçesine ters. |
| (a) acting kolun AKSİYON kaydını zenginleştir | RED | o kayıt `stage_parallel_markers` dışında (primary action entry); claim'i oraya taşımak + `action.input` namespace'ine engine metadata karıştırmak (kullanıcı payload alan adlarıyla çakışma). |
| **(a′) `_collapse` manşetine ek alan (SEÇİLDİ)** | ✅ | manşet zaten `trigger_*` taşıyor; yeni marker yok (gürültü hedefi korunur), user input namespace'ine dokunmaz, TÜM collapse yolları otomatik kapsanır (manşet koşulsuz yazılır), portal geriye uyumlu (`_collapse`'ı zaten tüketir). |

`_collapse` özetine iki alan eklendi: `trigger_claimed_by` + `trigger_claimed_at`. Claim,
acting kolun `BranchState`'inden commit-öncesi snapshot'ta okunur (kardeş kolların
`_branch_cancelled` için okuduğu yerin aynısı — adapter clear'ından önce). `trigger_actor
== trigger_claimed_by` çoğu yolda (reddeden = claim sahibi); asıl kıymetli alan
`trigger_claimed_at` (hold süresi = `now − claimed_at`). `claimed_by` kardeş marker'larıyla
simetri için tutulur. Sistem yollarında (SLA/escalation) acting kol yoksa iki alan da `null`.

Portal tarafında iş YOK — tüketilen marker ŞEKLİ değişmedi, alan EKLENDİ (WOR-64 etkisiz).
(`pipeline.rs::stage_parallel_markers`, test `fork_join.rs`.)

## WOR-68 — Arrived kolun claim hold süresi: `_branch_arrived` marker'ına `claimed_at` (2026-07-20)

**Sorun:** WOR-67 acting kolu, WOR-59 aktif kardeşleri kapattı. Ama join'e VARMIŞ
(`arrived`) kolun claim hold süresi hâlâ hesaplanamıyordu: `mark_branch_arrived` varışta
claim'i düşürür, `_branch_arrived` marker'ı yalnız onay anını (`approved_at`) yazardı —
claim BAŞLANGICI (`claimed_at`) hiçbir yere yazılmadan siliniyordu. "Onaylayan kişi kolu
onaylamadan önce ne kadar tuttu" WFAH'tan çıkmıyordu (sonradan collapse olsa
`_branch_superseded` de yalnız `approved_at` taşır).

**Karar:** `_branch_arrived` marker'ına `claimed_at` alanı eklendi (varış anında,
`CLEAR_BRANCH_CLAIM` clear'ından ÖNCE `wfes.branches` snapshot'ından okunur). Hold süresi =
`approved_at − claimed_at`. Bu "şu an tutuyor" değil, GEÇMİŞ hold metriğidir (arrived kol
claim'i bırakmıştır) — iş yükü/gecikme analizi için. Alan yazılmadan önceki eski
`_branch_arrived` kayıtlarında `claimed_at` yoktur → tüketici `null` görür (WOR-60'daki
`approved_by`/`approved_at` geriye uyum deseniyle tutarlı). Portal geriye uyumlu (alan
eklendi, şekil değişmedi). (`pipeline.rs::stage_parallel_markers`, test `fork_join.rs`.)

## Ek kararlar (bağımsız issue'lar)

- **WOR-10:** /org admin API'si `X-Admin-Key` başlığı ile korunur (`ADMIN_API_KEY` env).
  Unset ise dev modu: koruma kapalı + startup uyarısı. Kalıcı çözüm (admin JWT rolleri)
  ayrı işe bırakıldı.
- **WOR-11:** `CorsLayer::permissive()` kaldırıldı; `CORS_ORIGINS` env'i, unset ise
  yalnızca localhost dev origin'leri.
- **WOR-12/13/14/19:** Eski engine kodu silindiğinden kökten kapandı — v2.2'de
  autoexec'ler transition'a bağlı sonlu trigger listesidir (sonsuz döngü yolu yok),
  `trigger` alanı artık gerçekten çalışır, `next_seq` dead-code'ları kaldırıldı.
- **WOR-16:** OrgAdapter'ın inline `orgtnt_for_orgu` SQL'i korundu (tek sorgu,
  repo'ya taşıma davranış değiştirmiyor) — istenirse ayrı refactor.

## Terminal id = kullanıcı adı (case-insensitive unique)

**Sorun:** Editör terminal'lere `step-<uuid>` biçiminde otomatik id veriyordu; kullanıcının
UI'da girdiği `label` export'a hiç yansımıyordu (`useExport.ts` terminal id'yi `label`'dan
bağımsız, internal step id'den üretiyordu). Sonuç: WFD JSON'da terminal id'leri anlamsız
UUID'ler, isimler kayıp.

**Karar:** Terminal export id'si artık **doğrudan kullanıcının girdiği `label`** (trim
dışında dönüştürülmez — node key'lerdeki `slug(c_a)` mekanizmasının aksine terminal id'nin
engine tarafında charset/format kısıtı yok, sadece uniqueness). Terminal isimleri
**case-insensitive unique olmak zorunda** ("Start" ile "sTaRT" aynı isim sayılır):

- Editör: `assignTerminalKeys()` (src/utils/v22.ts) her export'ta `internalId → label` eşlemesi
  kurar; case-insensitive çakışma veya boş label varsa **throw** eder (export bloklanır) —
  node'lardaki `_<fnv1a16>` collision-suffix mekanizmasının aksine burada sessiz çözüm YOK,
  kullanıcı ismi değiştirmek zorunda. `PropertiesPanel`'de canlı uyarı (`findTerminalLabelCollision`).
- Internal step id (uuid, ReactFlow/flows/positions için) DEĞİŞMEDİ — sadece export-time
  id hesaplaması ayrıştırıldı (CaGroup node key'lerinin `nodeKeyOf` deseniyle aynı).
- Import: yeni-şema dosyalarda (`terminal.id` insan-okunur) `label = terminal.id`. Eski
  `step-<uuid>` id'li dosyalarda geriye uyumluluk için `label`, `wfe_end_response.status`'tan
  çıkarılır (önceki davranış korunur).
- Backend (`wfe-core/src/validator.rs::check_uniqueness`): terminal id'leri artık
  case-insensitive de karşılaştırılır — editör dışı üretilen WFD JSON'ları da korur (spec
  kaynaklı, sadece UI-side guard değil). Runtime lookup (`pipeline.rs::resolve_wft`)
  case-sensitive exact-match olarak DEĞİŞMEDİ — case-insensitivity yalnızca authoring-time
  uniqueness kuralı.
- Golden fixture (`examples/kredi-basvuru.golden.json`) değişmedi — `terminal_approved` /
  `terminal_rejected` zaten case-insensitive unique.

## WFE-seviyesi VIEW: WFAH katılımcısına kalıcı okuma hakkı (2026-07-14)

**Sorun:** Terminal commit `assigned_to` ve `current_node`'u temizler. `can_view` kapısının
üç kriteri de (owner / aktif node c_a / listable) düştüğünden, `listable` tanımlamayan
WFD'lerde biten WFE'nin dynctx'i ve end_response'u HİÇ KİMSEYE görünmez oluyordu
(work-pool-portal bulgusu: Reddet sonrası `red_sebebi` bağlamda görüntülenemiyor).

**Karar:** `can_view`'a katılımcı kapısı eklendi: viewer, WFAH'ta eylemi bulunan gerçek
bir kullanıcıysa (system/nil aktör hariç) WFE'yi görüntüleyebilir — WFE durumu fark etmez
(aktif veya terminal). Gerekçe: WFAH append-only audit izidir; sürece eylem katmış aktör
sonucu izleyebilmelidir. Bu kapı ACT/claim/listability ÜRETMEZ — yalnız VIEW; field-level
`x-visibility` filtresi katılımcılara da aynen uygulanmaya devam eder.
(`wfe-core/src/v22/visibility.rs::can_view`, testler `tests/visibility_view.rs`.)

## Kapsam notları

- WOR-26 / WOR-29 / WOR-30 (editör kararları) yukarıda kayıt altına alındı; kod
  uygulaması ilgili [EDITOR] issue'larında (WOR-50/52/53/49/54/55).
- Autoexec `python` / `lambda` tipleri şemada tanımlı; engine'de `Unsupported` hatası
  döner (executor'ları sonraki iş).
- Eski `crates/wfe-core/src/types/wfd.rs` modeli ve ona bağlı tüm deprecated yollar
  bu branch'in sonunda silindi; `$ctx.status` konvansiyonu kalktı (M1).

## Madde 7 — Yetkili claim devri (node.reassign)

**Sorun:** Bir WFE adımını claim eden kişi izne çıkınca / iş yanlış kişideyken, claim'i
başkasına devretmenin ya da havuza geri almanın yolu yoktu. `POST /wfe/:id/claim` yalnız
boş claim'i CAS ile alıyordu; `release_claim` sadece SLA-1 claim_timeout'un iç yoluydu.

**Karar:** Node'a opsiyonel `reassign` C_A kuralı eklendi (node `c_a` ile birebir aynı
şekil, AYNI `authorize()` matcher'ı → "C_A tek kuraldır" korunur). Bu kurala uyan aktör
(amir), `POST /wfe/:id/reassign` ile:
- belirli bir kullanıcıya devredebilir (`{"to": {orgu_id,user_id,role}}`) — hedef, o
  node'un `c_a`'sına uygun olmalıdır (aksi `TargetNotEligible`/400);
- havuza bırakabilir (`{"to": null}`) — sahiplik temizlenir, herkes yeniden claim edebilir.

`reassign` kuralı tanımlı değilse devir o node'da tamamen kapalıdır (`Unauthorized`/403).
Her başarılı devir append-only WFAH marker'ı yazar: `action` = `reassign` (hedefli) /
`unclaim` (havuz), `actor` = amir, `input` = `{from, to}`. Paralel modda (WOR-31) devir
kol-bazlıdır (`node` alanı). Alan opsiyonel olduğundan golden fixture değişmeden geçerli.
(`wfe-core/src/v22/pipeline.rs::reassign`, `wfe/src/wfe_adapter.rs::reassign`,
`server/src/routes/wfe.rs`, testler `tests/pipeline.rs` reassign_*.)

## Madde 8 — Ek-belge (attachments): katalog + node referansı

**Sorun:** Bir aksiyonun alınabilmesi için dış bir UI'dan (work-pool-portal) yüklenmiş
belgelerin varlığına bağlı olması gerekiyordu. Aynı belge kümesi bir node'a girdikten
sonra o node'dan çıkan tüm aksiyonların koşuluydu; belgeleri her aksiyona tekrar yazmak
istenmedi. Ek olarak: engine dış kaynaklara (S3/dosya sistemi) bağımlı OLMAMALI.

**Karar:** İki parça.
1. **Katalog + referans (WFD şeması).** Root'ta opsiyonel `attachments` katalogu
   (adlandırılmış gruplar; her grup `items[]` = `{id, label?, description?, required?
   (default true), formats?}`; her `formats[]` kaydı `{accept: string[], max_size_mb?}` =
   bir MIME grubu + o gruba ÖZEL boyut sınırı → farklı formatlar farklı MB). Node'lar `nodes.<key>.attachments`
   (grup key'leri dizisi) ile katalogu ADIYLA referanslar. `id` = "verilen dosya ismi";
   grup içinde tekildir. Custom validator: item.id grup-içi tekil (`attachment_item_dup`),
   node referansı katalogda var olmalı (`attachment_ref`), node içinde grup tekrarı yok
   (`attachment_ref_dup`). Alan opsiyonel — golden fixture değişmeden geçerli.

   **2026-08-07 — aksiyon kapsamı.** Referans girdisi artık iki biçimli: düz `"grup"`
   (node'un TÜM aksiyonlarına kapı — eski biçim, eski dosyalar aynen çalışır) ya da
   `{group, actions?}` (yalnız sayılan aksiyonlara kapı). Gerekçe: bir node'da "Onayla"
   belge isterken "Reddet"in istememesi olağandır; tek listeyle bu ancak node'u ikiye
   bölerek anlatılabiliyordu. `actions` **Option**'dır — `[]` (hiçbirini kapamaz,
   opsiyonel yükleme) ile alanın hiç verilmemesi (tümü) ZIT anlamlıdır; `#[serde(default)]`
   bir `Vec` ikisini aynı gösterirdi. İki biçim de "bu grup bu node'da TOPLANIR" der,
   ayrıldıkları yer yalnız kapıdır. Validator: kapsamdaki aksiyon o node'dan çıkan bir
   transition'da bulunmalı (`attachment_action_ref` — yoksa kapı hiç kapanmaz, dosya
   zorunlu sanılır ve hiçbir submit'i durdurmaz), kapsam içi aksiyon tekrarı yok
   (`attachment_action_dup`). Grup tekrarı denetimi biçimden bağımsızdır. Erişilebilirlik
   `start[]` kurallarını DA sayar (M16: `start[].action` normal bir ACT'tir) — yalnız
   transition'lara bakmak başlatma aksiyonuna konan kapıyı yanlışlıkla reddederdi.

   > **TARİHSEL (2026-08-11, aynı gün ikinci güncelleme):** bu paragrafın anlattığı
   > `POST /wfe/reserve` → `PUT /wfe/{wfe_id}/attachments/{grup}/{item}` (direkt X-Actor
   > ağacı) → `POST /wfe {…, wfe_id}` sırası ve `DELETE /wfe/reserve/{wfe_id}` ucu HTTP
   > olarak KALDIRILDI — tarama bu workspace'te çağıranı kalmadığını gösterdi. Yerini
   > altta anlatılan tek istekli multipart `POST /wfe` aldı (artık TEK yol; "ikinci yol"
   > değil). `POST /wfe` gövdesindeki `wfe_id` alanı da kaldırıldı — wfe_id'yi DAİMA
   > engine üretir, rezerve edilmiş id almanın dışarıdan yolu yok. `wf.wfe_reservation`
   > tablosu + `reservation.rs` + saatlik süpürücü DURUYOR ama YALNIZ crash ağı olarak
   > (istek ortasında sunucu ölürse yazılmış baytların sahibini süpürücüye bildirmek;
   > satır istemciye hiç görünmez). Bu blok geçmiş kararın gerekçesini anlatmak için
   > AYNEN kalır — güncel sözleşme `CLAUDE.md` → "Attachments (ek-belge) sözleşmesi"nde.

   **Başlatma aksiyonunda sıra TERSTİR: rezerve → yükle → başlat.** Dosya anahtarı
   `attachments/{wfe_id}/…` ve `wfe_id` eskiden start'ın İÇİNDE doğardı — bu yüzden
   başlatma kapısı sunucuda zorlanamıyordu. Çözüm id'yi başlatmadan önce üretir:

   ```text
   POST /wfe/reserve {wfd_id, version, environment?}  → { wfe_id }   (wf.wfe satırı YOK)
   PUT  /wfe/{wfe_id}/attachments/{grup}/{item}       → dosyalar NİHAİ anahtarına
   POST /wfe {…, wfe_id}                              → kapı → 422 ya da WFE o id ile doğar
   ```

   Taslak alan (`attachments/draft/…`) + kopyalama SEÇİLMEDİ: her başlatmada dosya taşımak
   ve iki anahtar uzayı taşımak gerekirdi. Bedeli, başlatılmayan rezervasyonların sahipsiz
   dosya bırakmasıdır — `wf.wfe_reservation` defteri + saatlik süpürücü (TTL 24 saat,
   `server/src/reservation.rs`) bunu karşılar. Defter aynı zamanda yükleme rotasının
   "bu id hangi WFD'nin katalogu" sorusunu cevaplar; yetki rezervasyonun SAHİBİYLE verilir
   (o aşamada `executor.query` çağrılamaz — ortada durum yok). Kapı `start[]` kuralının
   `from` node'u + aksiyonu üzerinden uygulanır; birden çok start kuralı varsa ve çağıran
   aksiyon adı vermediyse kapı uygulanmaz (hangi kuralın koşacağı belli değildir).
   Rezervasyonsuz gelen ve belge isteyen bir başlatma `422 attachment.reservation_required`
   ile reddedilir — sessizce başlatmak kuralı delerdi. (**2026-08-11:** rezervasyon uçları
   kaldırıldığı için bu kodun adı `attachment.multipart_required` oldu; anlamı "dosyaları
   multipart gövdesiyle aynı istekte gönder".)

   **2026-08-11 — ikinci yol: tek istekte başlatma.** Yukarıdaki sıra (rezerve → yükle →
   başlat) YANLIŞ değildi; başta kaldırılmadı, tamamlayıcı bir ikinci yol olarak eklendi
   (aşağıdaki adımlar o hâliyle uygulandı). **Aynı gün daha sonra** tarama HTTP uçlarının
   bu workspace'te tek kullanıcısı olmadığını gösterince eski yol tamamen kaldırıldı —
   bkz. yukarıdaki TARİHSEL kutusu; tek istekli yol artık tamamlayıcı değil, TEK yoldur.
   Gerekçe (tarihsel, eklendiği andaki gerekçe): 2+N
   isteğin her biri istemciye "hangi hatada rezervasyonu bırakmalıyım" sorusunu bilme
   yükü bindiriyordu — motor bilgisi istemci disiplinine bırakılmıştı. Aynı gün önce
   yalnız bu ikinci yolun kapıları eklendi (`assert_can_start` rezervasyonda + ortak
   `reservation::release` yardımcısı, yukarıdaki iki madde), sonra tek istekli yol bunların
   üstüne inşa edildi:
   - `POST /wfe` artık `multipart/form-data` da kabul eder; `payload` (JSON) parçası İLK
     olmak ZORUNDADIR (`400 multipart.payload_first`) — yetki kararı (aynı `assert_can_start`)
     dosya baytları okunmadan verilsin diye. Dosya part adı `{grup}/{slot}`; `filename`
     yalnız metadata, storage anahtarı dosya adından etkilenmez.
   - Dosyalar `AttachmentStore::writer` ile STREAM yazılır — bellek kullanımı dosya
     sayısından/boyutundan bağımsızdır. Her hata yolunda (413/415/422/5xx) o istekte
     yazılmış TÜM baytlar silinir ve rezervasyon satırı bırakılır: **istemci hiçbir
     telafi çağrısı yapmaz** (`DELETE /wfe/reserve/{id}` yeni yolun normal akışında hiç
     çağrılmaz; **güncelleme:** eski yol da aynı gün ikinci turda tamamen kaldırıldığından
     bu uç artık hiçbir yoldan çağrılamaz — bkz. yukarıdaki TARİHSEL kutusu).
   - Rezervasyon satırı crash ağı olarak GENİŞLETİLDİ: tek istekli yolda da istek başında
     yazılır, başarıda silinir, istemciye hiç görünmez — süreç istek ortasında ölürse
     (deploy/OOM) yazılmış baytların sahibi süpürücüye bildirilmiş olur.
   - **Çift başlatma koruması eklendi** (`start_dedupe.rs`, tablo `wf.wfe_start_dedupe`,
     migration `migrations/wf/20260811000001_wfe_start_dedupe.sql`): tek istek büyüyüp
     uzadıkça timeout/bağlantı kopması → "Başlat"a tekrar basma riski büyüdü. Parmak izi
     İSTEKTEN türetilir (actor+wfd+version+action+kanonik input+attachments bildirimi),
     istemciden `Idempotency-Key` gibi bir header İSTENMEDİ (bilinçli: üretmeyen istemci
     korumasız kalırdı). Pencere içinde (`WFE_START_DEDUPE_WINDOW_SECS`, 60 sn) tekrar →
     ilk `wfe_id` + `Idempotent-Replay: true`; hâlâ koşuyorsa `409 conflict.start_in_progress`.
     Kaçış: `X-Allow-Duplicate: true`. Parmak izi YALNIZ `payload`tan türer, dosya
     baytlarından DEĞİL — karar baytlar okunmadan verilir, tekrar istek dosyaları
     aktarmadan yanıtlanır.
   - `POST /wfe/preflight` eklendi: gövdesiz ön kontrol (yetki + slot kuralları +
     bildirilen boyut/tip). YAN ETKİSİZ ve KAPI DEĞİLDİR — `ok:true` dese bile gerçek
     denetim `POST /wfe` içinde yeniden koşar.
   - Yan düzeltmeler aynı turda: `DefaultBodyLimit` hiçbir yerde tanımlı değildi (axum
     varsayılanı 2 MB, katalogdaki `max_size_mb` sözünü yalanlıyordu) — `ATTACHMENT_MAX_REQUEST_MB`
     (200) ile yalnız `/wfe`+`/portal` alt ağaçlarına uygulandı. İçerik tipi artık
     `sniff_content_type`/`detect_mismatch` ile SNIFF edilir (415 `TypeMismatch`),
     `Sha256Stream` ile akış halinde bütünlük doğrulanır. `AppError` opsiyonel `items`
     alanı kazandı (çok-dosyalı ret slot bazında, `422 attachment.rejected`).
   - **Dosyanın DB'de bir karşılığı oldu**: `wf.wfe_attachment` (ad/tip/boyut/sha256/
     yükleyen/sürüm). Aynı slota tekrar yükleme üzerine yazmaz, yeni sürüm açar — denetimde
     "karar anında hangi belge oradaydı" cevaplanabilir. `wfe_id` FK'sı CASCADE: satır varsa
     WFE vardır. **Kapı yine DEPOYA bakar** (`status_for_node` → `exists`); metadata gösterim
     katmanıdır (`enrich_with_meta`), kaynak yapılsaydı tablo eklenmeden önce yüklenmiş her
     belge "yok" görünürdü.
   - **Baytlar isteğe hiç girmeden de gelebilir**: `POST /uploads` ile staging'e konur
     (`wf.upload_staging`, anahtar `staging/{upload_id}`, nihai anahtarla AYNI depoda),
     başlatmaya yalnız `upload_id` girer, sunucu server-side COPY ile taşır. s3'te presigned
     PUT — 500 MB'lık bir rapor engine'in bant genişliğini hiç kullanmaz. Sahipsiz staging
     `staging::sweep_expired` ile (TTL 24s) toplanır.
   - Tasarım, reddedilen alternatifler (K1-K10) ve UYGULAMADAKİ SAPMALAR:
     `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md`.
2. **Engine saf kalır; gate portal edge'inde.** wfe-core yalnız katalog + referansı
   METADATA olarak taşır, dosya I/O YAPMAZ. Varlık kontrolü ve yükleme server'ın portal
   katmanındadır: `AttachmentStore` (opendal; local fs default kök `../work-pool-portal/
   storage`, `ATTACHMENT_STORAGE_*` env, S3'e geçince aynı arayüz). Storage anahtarı
   `attachments/{wfe_id}/{grup}/{item}` — aynı grubu referanslayan farklı node'lar dosyayı
   tekrar istemez.

**Akış.** "Hangi aksiyonlar alınabilir?" (`GET /wfe/:id/attachments`, direkt X-Actor
ağacı) → aktörün gördüğü node(lar)ın referanslı gruplarının item bazlı yükleme durumu +
`satisfied`. Bu uç AKSİYON SORMADAN çağrılır: her grup `gates: true` döner ve kapsamı
`actions` alanında taşır (`null` = tümü) — süzmeyi seçili aksiyona göre istemci yapar.
JWT `/portal/wfe/:id` detayında ise durum AKSİYON BAŞINA hesaplanır; oradaki
`attachments_satisfied` yalnız o aksiyonu kapayan grupları sayar. UI, submit'i seçili
aksiyonun cevabına göre disable eder. Zorlama server-side: `apply_action` (ve JWT
`submit_action`) submit edilen aksiyonu kapayan grupların `required` dosyaları eksikse
engine'e HİÇ gitmeden `422 code: "attachment.missing"` döner (UI-only gating'e güvenilmez).
Yükleme/indirme/silme: `PUT/GET/DELETE /wfe/:id/attachments/:group/:item` (ham gövde;
upload'ta içerik tipi bir `formats` kuralına uymalı ve uyan kuralın `max_size_mb`'si
uygulanır — uymazsa 415, aşarsa 413). Aynı endpoint'ler JWT `/portal/wfe/*`
ağacında da vardır. Örnek fixture: `examples/belge-onay.json`.
(`wfe-core/src/types/wfd_v22.rs` AttachmentGroup/Item, `validator.rs::check_attachments`,
`server/src/attachments.rs`, `server/src/routes/attachments.rs`,
`server/src/routes/portal/attachments.rs`, `server/src/routes/wfe.rs::apply_action`.)

---

## SLA sözleşmesi ikinci tur (2026-07-28): terminal-hedef yasağı + SLA-1 effects

**Sorun 1 — SLA-2'nin effect'i.** `escalation[].wfes_effects` opsiyoneldi ve editör bu
alanı hiç yazmıyordu; SLA süresi dolduğunda DynCtx'e hiçbir şey yazılmıyordu ("SLA aşımı"
notu, breach damgası vb. modellenemiyordu). SLA-1 (`claim_timeout`) ise şemada
`wfes_effects` alanını hiç TAŞIMIYORDU.

**Karar 1.** SLA-1'e opsiyonel `wfes_effects` eklendi; her iki SLA türü de editörden
düzenlenebilir. Süre dolduğunda effect'ler **system aktörü** adına uygulanır: `$actor` =
`{role: "system"}`, `$node` = SLA'nın tetiklendiği node. `$action.input.*` /
`$exec.result.*` YASAK (`sla_effect_namespace`) — SLA'yı ne bir aksiyon ne bir autoexec
tetikler, bu yollar sessizce `null` yazardı. Verilmezse hiçbir şey yazılmaz.

SLA-1'in `wft`'siz (havuza-dönüş) yolu `commit()`'ten geçmez — node/status değişmez —
bu yüzden `ClaimTimeoutOutcome::Release` artık `WfahEntry` yerine
`ClaimRelease { wfah_entry, new_dynctx: Option<Value> }` taşır ve
`WfeStore::release_claim` yeni ctx'i marker'ın seq'i ile AYNI transaction'da
`wf.wfe_dynctx`'e yazar (`None` → ctx satırı yazılmaz).

**Sorun 2 — SLA hedefinin terminal olması.** SLA-1/SLA-2 bir terminal'i hedefleyebiliyordu.
Bu, zaman aşımını "başarılı bitiş" gibi kaydediyor: terminal'in `wfes_effects`'i ve
`wfe_end_response`'u sanki biri aksiyon almış gibi uygulanıyor, `SLA.Dwell` sinyali
kayboluyor ve raporlamada breach ile normal kapanış ayırt edilemiyordu.

**Karar 2.** SLA hedefleri **yalnız node** olabilir (`sla_terminal_target`). Akışı zaman
aşımıyla bitirmek için SLA-2'nin `terminate: true` adımı kullanılır
(`end_response.reason = "SLA.Dwell"`). Kapsam: `{"terminal": …}`, `conditions[].terminal`,
`conditions.default.terminal`, `{"collapse": {"terminal": …}}` → hata.
`{"parallel": …}`'in `join` hedefi **muaftır** — join, kollar bittikten sonraki ayrı bir
hop'tur; SLA'nın indiği yer kolların giriş node'larıdır (hepsi zaten node).

**Uyumluluk.** Terminal hedefli SLA taşıyan mevcut WFD'ler artık `validate` hatası verir
(upload + fetch kapısı). Elle düzeltilmeleri gerekir: hedefi bir node yapın ya da SLA-2'de
`terminate: true`'ya çevirin. Editör de aynı kuralı uygular — SLA hedef listelerinde
terminal'ler artık listelenmez, mevcut terminal-hedefli kayıt "kayıt yok" hatası verir.
(`wfe-core/src/types/wfd_v22.rs::ClaimTimeout`, `validator.rs::check_sla` +
`sla_terminal_landings` + `check_sla_effect_namespaces`,
`v22/pipeline.rs::fire_claim_timeout`, `v22/ports.rs::release_claim`,
`wfe/src/wfe_adapter.rs`, `docs/spec/schema.json`.)

---

## SLA yetki sınırı (2026-07-28, ikinci düzeltme): akışı yalnız SLA-3 bitirir

**Sorun.** Terminal-hedef yasağı getirildikten sonra SLA-2'nin `terminate: true` adımı
tek "akışı bitirme" yolu olarak kaldı. Ama sorunun kökü hedefin terminal olması değil,
**SLA-1/SLA-2'nin bitirme yetkisi olması**: ikisi de tek bir node'daki bekleme süresini
ölçer. O sürenin dolması "iş bitti" değil "bu adımda tıkandı" demektir; işi kapatma
kararı bir node'un beklemesine değil, TÜM akışın bütçesine bakmalıdır.

**Karar.** Zaman aşımıyla akışı `terminated` yapma yetkisi YALNIZ **SLA-3**'e (root
`timeout`, `end_response.reason = "SLA.Deadline"`) aittir.

- `escalation[].terminate` **kaldırıldı** (`escalation_terminate_removed`); `wft`
  ZORUNLU oldu (`escalation_wft_required`). `SLA.Dwell` artık üretilmez.
- SLA-1 zaten bitiremiyordu (ya claim'i havuza bırakır ya bir node'a taşır) —
  terminal-hedef yasağı bunu tamamlar.
- Alan struct'ta `Option<bool>` olarak KALIR ama yalnız reddetmek için: aksi halde
  `deny_unknown_fields` yüzünden eski dokümanlar ham serde parse hatası verirdi;
  böyle anlaşılır bir validasyon mesajı verilebiliyor. Yeni dokümanlara yazılmaz.
- Kardeş kolları düşürme ihtiyacı `wft: {"collapse": {"node": …}}` ile karşılanır —
  paralel mod biter, akış hedef node'da AKTİF kalır (bitmez).

**Editör.** "Akışı sonlandır (terminate)" seçeneği kaldırıldı. Eski dokümanlardan gelen
`terminate: true` adımı SESSİZCE DÜŞÜRÜLMEZ: kendi grubuna işaret eden bir adım olarak
import edilir ve `ESCALATION_SELF_TARGET` hatası kullanıcıya ne yapması gerektiğini
söyler (hedefi değiştir ya da adımı sil; akışın toplam süresi için "Tamamlanma süresi").

**Uyumluluk.** `terminate: true` taşıyan mevcut WFD'ler upload/fetch'te reddedilir.
(`wfe-core/src/types/wfd_v22.rs::EscalationStep`, `validator.rs::check_sla`,
`v22/pipeline.rs::fire_escalation`, `docs/spec/schema.json`.)

---

## SLA hedef formu (2026-07-28, üçüncü düzeltme): `wft` yalnız `{node}`

**Sorun.** Terminal-hedef yasağı ve `terminate` kaldırılması yalnız DOĞRUDAN yolu
kapatıyordu. `escalation[].wft` hâlâ tam bir `Wft` olduğu için dolaylı yollar açıktı:
`{"conditions": [{"when": …, "terminal": …}]}` bir switch üzerinden terminal'e iniyor,
`{"parallel": …}` bir fork açıyor, `{"collapse": …}` kardeş kolları düşürüyordu. Editör
tarafında da escalation hedefi bir `switch` step'i olabiliyor ve `buildSwitchWft` ile
conditions'a açılıyordu.

**Karar.** SLA-2'nin `wft`'i YALNIZ `{"node": …}` formunu kabul eder
(`sla_target_not_node`; terminal için ayrıca daha açıklayıcı `sla_terminal_target`).
Şemada `$ref` `wft` → `wftNode` olarak daraltıldı. SLA-1 zaten bare node key taşıyordu.

Gerekçe: SLA tek bir node'daki bekleme süresini ölçer. Bir zamanlayıcının verebileceği
tek meşru sonuç "işi sıradaki sorumluya devret"tir. Dallanma (conditions), fork
(parallel), kol düşürme (collapse) ve akışı bitirme birer KARARDIR — bunları bir aksiyon
ya da SLA-3 verir. Autoexec zaten bir `wft` hedefi değildir (wire formatında varyantı
yok), dolayısıyla "SLA → autoexec → terminal" yolu hiç var olmadı.

Böylece bir SLA-2 adımının tek olası runtime sonucu `MoveTo` (paralel modda
`BranchMoveTo`, join hedefliyse `BranchArrived`) olur. Önceki turda kardeş-kol düşürme
için önerilen `wft: {"collapse": {"node": …}}` da bu kararla YASAKTIR — o yol bir
aksiyona aittir (`branch_collapse_to_node_ends_parallel_and_moves_wfe`).

**Editör.** Hedef listeleri yalnız aktör grubu (CaGroup) gösterir; `switch` hedefi artık
conditions'a AÇILMAZ (`useExport`'tan `buildSwitchWft` çağrısı kaldırıldı). Yeni
validasyon kodu `SLA_TARGET_NOT_CAGROUP` bitiş adımı / dallanma / otomasyon / paralel
hedeflerini adıyla bildirir. Import: `{node}` dışındaki her form (ve eski `terminate`)
kendi grubuna işaret eden bir placeholder olarak alınır — adım kaybolmaz, hata olarak
görünür (`ESCALATION_SELF_TARGET`).

**Uyumluluk.** `escalation[].wft` içinde conditions/parallel/collapse/terminal taşıyan
mevcut WFD'ler upload/fetch'te reddedilir.
(`wfe-core/src/validator.rs::check_sla` + `wft_form_name`, `docs/spec/schema.json`
`escalationStep.wft → wftNode`, `agnoflow-frontend/src/utils/validation.ts::slaTargetProblem`,
`src/hooks/useExport.ts`, `src/utils/wfdImport.ts`.)

## WOR-70 (2026-07-29) — `context.required` kaldırıldı; context'e tek yazma yolu `wfes_effects`

**Sorun:** Zorunluluk üç yerde bildiriliyordu ve ikisi ölüydü.

1. `context.required` — start zinciri bittikten sonra FINAL ctx üzerinde bir kez
   denetleniyordu. Üç fixture'ın hepsinde start aksiyonunun `input.required`'ının
   birebir kopyasıydı; sıfır ek bilgi taşıyordu. Editör de onu doğrudan oradan
   türetiyordu (`deriveFullContextSchema`), yani iki katmanın farkı hiç kullanılamıyordu.
   Üstelik validator `context.required` yollarını HİÇ denetlemiyordu: `["applicantt"]`
   gibi bir yazım hatası upload'dan geçip her `start` çağrısını patlatıyordu
   (`input.required` yolları ise denetleniyordu — asimetri).
2. `context.properties.<field>.required` — motor bunu hiç okumuyordu (JSON Schema
   validator'ı yok; `validate_context_required` yalnız kökü okuyordu). Tamamen süstü.
3. `actions.<ad>.input.required` — gerçek kapı. Anlamı net: *"bu aksiyonu tetikleyen
   istekte şu isimde parametreler bulunmak zorunda."*

Asıl boşluk başka yerdeydi: ctx'e **iki** yazma yolu vardı. `merge_action_input`
declared input yollarını doğrudan ctx'e yazıyordu, `wfes_effects` de yazıyordu. Bir
alanın değerinin nereden geldiği akışa bakılarak cevaplanamıyordu. Ve hiçbir kural
"bu context alanını yazan var mı" diye sormuyordu — hiç dolmayacak ölü alanlar
şemada durabiliyor, `when` ifadelerinde ve portal formlarında görünebiliyordu.

**Karar:** Çalışma-anı doluluk denetimi bırakıldı, yerine tasarım-zamanı bütünlük
kuralları kondu. Zorunluluk tek yerde bildirilir (`input.required`), ctx'e tek yol
yazar (`wfes_effects`).

- **`merge_action_input` → `validate_action_input`:** aksiyon girdisi ARTIK ctx'e
  yazılmaz; yalnız sözleşme denetlenir (required mevcut mu, bildirilmemiş leaf var mı).
  Girdiyi ctx'e taşımak akışın açık işidir:
  `"set": { "applicant": "$action.input.applicant" }`.
- **`validate_context_required` SİLİNDİ** — start sonrası ctx doluluk denetimi yok.
- **`context.required` + iç içe `required` HARD REJECT** (`context_required_removed`).
  Sessiz yoksayma reddedildi: eski dokümanlar elle temizlenmeli, çünkü sessiz
  yoksayma "kural hâlâ işliyor" yanılgısını sürdürürdü. Şema düzeyinde de kapatıldı
  (`contextSchemaNode.not.required`), böylece editör/istemci JSON Schema doğrulaması
  da aynı cevabı verir.
- **`context_field_never_written`:** her context yaprağı en az bir `wfes_effects.set`
  hedefi tarafından kapsanmalı. Kapsama iki yönlü (ata yazımı torunu, torun yazımı
  opak atayı kapsar). Taranan yazar siteleri: start/transition effects + trigger
  catch + escalation + claim_timeout + terminal + autoexec.
- **`unused_action_input`:** bir kuralın aksiyonunun bildirdiği her input yolu
  (`required ∪ optional`) o kuralın effects'inde `$action.input.<yol>` ile tüketilmeli.
  Tüketici olarak kuralın kendi effects'i, `trigger[].catch.wfes_effects`'i ve
  tetiklediği `autoexec.<ad>.wfes_effects` sayılır — aksi halde autoexec'e yazdırılan
  girdiler yanlış yere hata verirdi.
- **Absent-input skip (zorunlu tamamlayıcı):** effect değeri TAM OLARAK
  `"$action.input.<yol>"` ise ve o yol istekte YOKSA set atlanır, `null` yazılmaz.
  Bu kural olmadan Yol A opsiyonel input'u ifade edemezdi: gönderilmeyen
  `internal_notes`, golden fixture'da escalation'ın yazdığı SLA notunu silerdi.
  "Yok" ile "açıkça null gönderildi" ayrıdır — ikincisi yazılır. Kural yalnız bu
  tam-eşleşme formuna özgü; `$ctx.*` / `$exec.result.*` eskisi gibi `null` çözer.

**Reddedilen alternatif (Yol B):** otomatik input→ctx yazımını korumak ve ölü-alan
kuralını "input VEYA effects yazıyor mu" diye gevşetmek. Üç fixture'ı olduğu gibi
geçirirdi ve geriye uyumluydu; ama iki yazma yolu belirsizliğini sürdürürdü. İzlenebilirlik
tercih edildi (kullanıcı kararı, 2026-07-29).

**Golden fixture DEĞİŞTİRİLDİ** — CLAUDE.md'nin "golden fixture değiştirilmez" kuralı bu
karar için kullanıcı onayıyla askıya alındı. Değişiklik: `context.required` + iç içe
`required` silindi; `create_application` / `analyst_approve` / `manager_decide`
kurallarına girdilerini yazan `set` satırları eklendi. Aynı düzenleme `belge-onay.json`
ve `paralel-onay.json`'a da uygulandı; `crates/wfe-core/tests/fixtures/` kopyaları
senkron tutulur.

**Tüketici etkisi:** work-pool-portal formları zaten `inputDef.required`'dan üretiliyordu
(`WorkflowsPage` / `InstanceDetail`), `context.required`'ı okumuyordu — geride yalnız
`DynamicForm.SchemaFields`'ta ölü bir `required` prop'u ve yanlış bir yorum kaldı,
temizlendi. Editör tarafında `contextAuthoredRequired` alanı ve `deriveFullContextSchema`'nın
`required` üretimi kaldırıldı; yerine yayın öncesi kullanıcıya dönük iki uyarı eklendi
(tüketilmeyen input / yazılmayan context alanı) ve bunlar giderilmeden export/upload
edilemez.

## WOR-70b (2026-07-29) — `required` non-null; gönderilmeyen `optional` `null` yazar

WOR-70'in devamı; aynı gün, kullanıcı düzeltmesi üzerine.

**Sorun:** WOR-70'te "absent-input skip" getirilmişti — effect değeri tam olarak
`$action.input.<yol>` ise ve yol istekte yoksa set atlanıyordu. Gerekçe opsiyonel
input'un mevcut değeri silmesini önlemekti. Ama bu, `optional`'ın anlamını
bulanıklaştırıyordu: alan "yazılmamış" mı, "boş yazılmış" mı belirsizdi ve ctx'te
alanın var olup olmaması isteğin şekline bağlı hale geliyordu.

**Karar (kullanıcı):** `required` ile `optional` arasındaki fark ctx'e yazılıp
yazılmamasında DEĞİL, yazılan değerdedir.

- İkisi de `wfes_effects` ile ctx'e eşlenmek ZORUNDADIR. `optional` olması muafiyet
  değildir — `unused_action_input` her ikisini de denetler (WOR-70'te de böyleydi).
- `required`: gönderilmek zorunda VE `null` olamaz → yeni hata
  `zorunlu input 'x' null olamaz` (`validate_action_input`). Denetim YALNIZ bildirilen
  yolun kendisine bakar; `required: ["applicant"]` ile `{"applicant":{"name":null}}`
  geçerlidir (alt alanı zorunlu istiyorsan `applicant.name`'i ayrıca bildir).
- `optional`: gönderilmeyebilir; gönderilmediğinde ctx'e `null` YAZILIR.
  **absent-input skip KALDIRILDI** — `apply_effects` her satırı koşulsuz uygular.

Böylece `ek_bilgi` gibi bir alan context'te tanımlı, bir aksiyonun `optional` girdisi
ve o aksiyonun effects'inde `"ek_bilgi": "$action.input.ek_bilgi"` ile eşlenmiş
olabilir; kullanıcı doldurmazsa `null` kalır. Alan ölü değildir — yazarı var, değeri boş.

**Yan etkinin ele alınışı:** koşulsuz yazım, bir alanı hem opsiyonel girdi hem başka
bir yazar (escalation / autoexec / terminal / başka kural) yazdığında diğerinin
değerini `null`'a çevirir. Sessiz bırakmak yerine tasarım anında bildirilir:
`optional_input_nulls_other_writer` **UYARISI** (yayını engellemez — bilinçli tasarım
olabilir). Golden fixture bu durumun canlı örneğidir (`internal_notes`: iki aksiyonun
opsiyonel girdisi + `self__creditAnalyst` escalation'ı) ve fixture bu tek uyarıyı
BEKLENEN olarak taşır; `golden_fixture_is_valid` testi onu ismen kabul eder, başka
uyarıya izin vermez.

**Reddedilen alternatif:** golden fixture'daki çakışmayı kaldırmak (internal_notes'u
opsiyonel listelerden çıkarmak). Örneğin öğretici değeri korundu — kuralın gerçek bir
akışta nasıl göründüğü fixture'da görünsün.


## WOR-71 (2026-07-29) — `x-wf-readonly` KALDIRILDI

WOR-70'in devamı; aynı gün, kullanıcı kararıyla.

**Sorun:** WOR-70 sonrası context'e tek yazma yolu `wfes_effects` oldu ve her input
`$action.input.<yol>` ile açıkça tüketilmek zorunda kaldı. Bu haliyle
`x-wf-readonly` üç ayrı gerekçeden de düştü:

1. **Runtime denetimi ölü koddu.** `pipeline::validate_readonly_paths` yalnız
   *bildirilen* yollara bakıyordu; readonly bir yolu bildiren WFD ise zaten
   validator'dan (`readonly_input`) geçemiyordu. Her giriş noktası (`/wfd` upload,
   `/wfe/simulate`, `wfd` adapter fetch) run öncesi `validate()` çağırdığı için bu
   fonksiyon hiç ateşlenemezdi.
2. **Kalan tasarım-zamanı denetimi sızıntılıydı.** Effect'in readonly alan yazmasını
   engelleyen bir kural YOKTU (olması da istenmezdi — flag'in amacı "engine yazar"dı).
   Dolayısıyla `set: { credit_score: "$action.input.puan" }` ile kullanıcı değeri
   readonly alana rahatça inebiliyordu. Flag bir güvenlik sınırı değil, isim lint'iydi.
3. **Bilgi ikizlendi.** "Bu alanı yalnız engine yazar" WFD'den zaten türetilebiliyor:
   alan hiçbir `actions.<ad>.input`'ta bildirilmemişse onu ancak `wfes_effects`
   doldurabilir. Flag ile gerçek yazar listesi arasında sessiz çelişki riski vardı.

**Karar:** uzantı tamamen kaldırıldı. Yerine geçen değişmezler WOR-70'ten gelir:
`context_field_never_written` (her alanın en az bir yazarı var) + `unused_action_input`
(her bildirilen girdi açıkça tüketiliyor). "Engine-only" alan = hiçbir action input'unda
adı geçmeyen alan.

**Reddedilen alternatif:** flag'i tutup boşluğu kapatmak — "readonly alan yazan effect
`$action.input.*` kaynaklı olamaz" kuralını eklemek. Flag'i anlamlı yapardı ama zaten
türetilebilir bir bilgiyi elle bildirmeye devam etmek + üçüncü bir çapraz denetim
taşımak anlamına geliyordu.

**Kaldırılanlar.** Backend: `validator::PathResolution::Readonly` + `readonly_input`
kuralı, `pipeline::validate_readonly_paths`, `action_input_targeting_readonly_field_is_error`
testi, `docs/spec/schema.json` `contextSchemaNode.x-wf-readonly`. Frontend:
`ContextSchemaNode['x-wf-readonly']`, JsonSchemaEditorModal'daki `xWfReadonly` alanı +
checkbox'ı, `wfdDiff` `CONTEXT_ATTR_KEYS` girdisi (`salt-okunur` etiketi), store seed
şeması. Fixture'lar (golden dahil, CLAUDE.md'deki "fixture değişmez" kuralına spec
değişikliği istisnası) ve `docs/spec/examples/*` temizlendi.

**Yan etki — simülasyon start formu.** Frontend'de flag'in tek işlevsel kullanıcısı
`SimInputFields.SchemaFields`'ti: başlangıç formunu `context.properties`'ten çizip
readonly alanları filtreliyordu. Bu katman WOR-70'ten beri yanlıştı (engine bildirilmemiş
yolu hard reject eder). Form `PathFields`'e geçirildi — alanlar artık start kuralının
aksiyonunun `input.required/optional`'ından geliyor; zorunluluk denetimi de
`context.required` (kaldırıldı) yerine o listeden yapılıyor. `SchemaFields`/
`SchemaEntries`/`missingRequired` ölü kaldığı için silindi.

**Yan etkinin devamı — çoklu start kuralı.** `start[]` bir dizidir ve engine aktörü
yetkilendiren İLK kuralı seçer (`Engine::start`), dolayısıyla "hangi alanlar sorulacak"
`start[0]`'a değil SEÇİLİ AKTÖRE bağlıdır. Çözüm saf yardımcılara ayrıldı
(`src/utils/startRules.ts` + testleri):

- `startPoolCas(rules)` — aktör listesindeki ✓ ve "Akışı kimler başlatabilir" satırı artık
  TÜM start kurallarının c_a birleşimini kullanır (eskiden yalnız `start[0]`'ın c_a'sı;
  ikinci kuralın aktörü ✓ almadığı için seçilemiyordu).
- `resolveStartCandidates(rules, actor)` — seçili aktörün başlatabildiği kurallar, dizi
  sırası korunarak; aynı `action`'ı taşıyan kurallar ada göre tekilleştirilir (input
  listeleri zaten aynı olduğu için kullanıcıya anlamsız seçim sorulmaz).
- Aday sayısı 0 → form yerine "bu atama akışı başlatamaz" uyarısı + Başlat kilitli
  (eskiden sessizce `start[0]`'ın alanlarını soruyordu). 1 → doğrudan o kural.
  >1 → start aksiyonu seçici, seçim değişince girilen değerler temizlenir.
- Seçilen `action` start isteğinde AÇIKÇA gönderilir (M16 `SimStartBody.action`).
  Gerekçe: frontend'in `matchesCa`'sı iyimserdir — `c_orgu` scope çözümü org resolver
  ister, o yüzden UI aday kümesi engine'inkinden geniş olabilir. `action` gönderilmezse
  engine sessizce başka bir kuralla başlayabilir ve form yanlış alanları sormuş olur;
  gönderilince yetki yoksa net `StartNotEligible` döner.

Bilinen sınır: `currentCa` `serializedRef.current`'tan türediği için (dosyada var olan
`react-hooks/refs` ihlali) `activeCas` memoize edilmez; koşu-içi ve başlangıç başlıkları
ayrı bloklara bölünerek ref okuma sayısı baseline'da tutuldu.

**Geriye uyumluluk.** Migration GEREKMEZ: `contextSchemaNode` `additionalProperties: true`
olduğu için eski WFD'lerdeki `x-wf-readonly` anahtarı hâlâ valid — yalnız artık hiçbir
anlamı yok, yok sayılır.

## WFC (2026-07-30) — İş Akışı Çağrısı: alt akış node'u + ardıl akış

**Karar.** Bir WFE başka bir WFD'yi çalıştırabilir. Tek katalog (`calls`), tek belirleyici
eksen (`mode`), üç mod:

| `mode` | Yerleşim | Davranış | Sonuç |
|---|---|---|---|
| `wait` | `nodes.<k>.call` | Çağıran o node'da **bekler** | `$call.*` ile döner |
| `detached` | `nodes.<k>.call` | Çağrılan başlatılır, çağıran hemen devam eder | yok |
| `terminal` | `terminals[].call` | Çağıran **biter**, ardıl akış başlar | yok |

Plan ve tam gerekçe: `docs/plans/workflow-call.md`.

**Katalog ↔ referans ayrımı `autoexec` ↔ `trigger`'ın aynısıdır.** Katalog NE çağrılacağını
ve hangi girdiyle çağrılacağını tutar; referans NASIL çağrıldığını. Böylece aynı katalog
kaydı üç modda da kullanılabilir.

**Bekleme node'da durur, transition'da değil.** Çağrılan günler sürebilir; bekleme bir
*durum*tur. `WFES = current_node + assignment + DynCtx + WFAH` değişmezi korunur —
beklemenin kalıcı yeri `current_node`'dur. Bu yüzden çağrı `nodes.<k>.call`'dadır.

**WFC node'unda `c_a` hâlâ zorunludur.** "Node key = slug(c_a)" ve "aynı canonical c_a
ikinci node'da olamaz" değişmezlerine dokunulmadı. Anlamı daralır: *alt akış sürerken bu
WFE'yi kim görür ve kim iptal edebilir* — ACT/claim vermez. Node'da `kind` alanı YOKTUR;
node'u WFC node'u yapan şey `call` bloğunun varlığıdır (start node'un "referans ile
türetilmiş kimlik" deseninin aynısı).

**`sync` (trigger gibi bloklayan) modu değerlendirildi ve ELENDİ.** Yeni yetenek
getirmiyordu: çağrılan tam otomatik ise `wait` zaten saniyeler içinde döner (çağrılanın
terminal commit'inde opportunistic nudge). İçinde bir insan havuzu varsa daima
`WFD.CallTimeout` olurdu ve validator "bu WFD insansız mı" diye statik karar veremez —
sessiz üretim tuzağı. `mode` bir enum olduğu için ileride kırıcı olmadan eklenebilir.

**`$call.*` ayrı bir namespace'tir, `$exec.result.*` ile birleştirilmedi.** Autoexec bir
sistem çağrısıdır, WFC bir WFE örneğidir. WFC-RETURN dışındaki bağlamlarda `$call` boş bir
kabuktur (null döner, ifade patlamaz).

**WFC-IN'de `$action.input.*` YASAK.** İki gerekçe: (1) moddan bağımsızlık — `terminal`
modunda ACT girdisi güvenilir biçimde mevcut değil (SLA-3 ile ulaşılan terminal'de hiç
yok), (2) WOR-70 tutarlılığı — ctx'e tek yazma yolu `wfes_effects`'tir, böylece "çağrılana
ne gitti" DynCtx'te denetlenebilir kalır. Bir ACT girdisini çağrılana geçirmek isteyen onu
önce effects ile ctx'e yazar.

**Ardılın üç sert kuralı.**
1. **Chain Isolation:** ardıl çağrı, çağıranın sonucunu ASLA değiştirmez. Ardıl
   başlatılamasa bile çağıran `completed` kalır; hata yalnız WFAH marker'ı + çağrı
   satırında görünür.
2. **Cascade ardılı kapsamaz.** Çağıran sonlandığında koşan alt akışlar (`wait`/
   `detached`) `cancelled` edilir; ardıl edilmez — ardıl, astın aksine çağıranın ömrüne
   bağlı değildir.
3. **Yalnız başarılı `Terminal` tetikler.** `Failed`/`Terminated` (SLA ihlali, engine
   hatası) ardıl tetiklemez.

**Ardıl döngüsü, autoexec'te karşılığı olmayan yeni bir başarısızlık sınıfıdır.** A bitince
B, B bitince A → sonsuz WFE üretimi. İki katmanlı fren: statik `call_next_cycle` (reddet) +
runtime ardıl derinliği sınırı. Meşru döngü isteyen terminal'de `max_next: N` ile AÇIKÇA
izin verir. Yuvalanma döngüsünün (`call_cycle`) böyle bir kaçışı YOKTUR.

**Döngü tespiti kenar üzerinde yapılır.** Kökün kendisi `WfdProvider`'dan çözülemeyebilir
(yayınlanmamış taslak). "Hedefe git, orada kendini gör" yaklaşımı döngüyü kaçırırdı;
bu yüzden DFS yığınına giden bir kenar görüldüğünde döngü bildirilir.

**Versiyon:** `version` verilmezse çağrı anındaki en son yayınlanmış sürüm; verilirse
pinlenir. Yaratılan WFE her hâlde start anında bir sürüme sabitlenir — yani pin'siz
çağrıda yeni sürüm yayınlamak KOŞAN WFE'leri etkilemez.

**Validator iki katmanlıdır.** `validate()` yalnız yerel kuralları koşar (saf `wfe-core`,
I/O yok). `validate_with(wfd, Some(&provider))` cross-WFD kurallarını da koşar (girdi
kümesi, tip uyumu, `$call.result.*` anahtarları, döngü). Upload yolunda resolver DAİMA
verilir. Kritik olan kural — *"çağrılanın girdileri çağıranın context'inde bulunmalı"* —
YEREL'dir (`call_input_source_undeclared`): çağıranın kendi şemasına bakar, resolver
gerektirmez.

**Bilinen boşluk (Faz 2 girdisi): `wf.wfd_meta` doküman kimliğini indekslemiyor.** Tablo
WFD'yi `(orgtnt_id, name, integer version)` ile saklar; `CallDef.wfd_id` ise dokümanın
`id` alanına, `CallDef.version` ise dokümanın semver `version`'ına atıfta bulunur. DB
üzerinden çözüm için `wf.wfd_meta`'ya indeksli bir `doc_id` (ve semver) kolonu eklenmesi
gerekir — aksi halde her upload'da tenant'ın tüm WFD JSON'larını okumak gerekirdi. Bu
yüzden DB-destekli `WfdProvider` Faz 2'ye (migration fazı) bırakıldı; Faz 1'de resolver
trait'i ve tüm kurallar hazır, sahte katalogla test edilmiş durumdadır.

## WOR-72 (2026-07-31) — OR-join: `join_mode` + K-of-N quorum

**Karar.** `wft.parallel` iki alan kazandı: `join_mode: "and" | "or"` (varsayılan
`and`) ve yalnız OR ile geçerli `join_threshold: K` (varsayılan 1). WOR-31'in
AND-join'i tek seçenek olmaktan çıktı; "üç departmandan İKİSİ onaylarsa yeter"
gibi kurallar tasarımcı tarafından ifade edilebiliyor.

**Neden collapse ile modellemedik.** WOR-56 `collapse` zaten "kardeşleri düşür,
hedefe git" yapıyor; her kolun join aksiyonunu `collapse: <join hedefi>`'ne
derleyerek motor DEĞİŞMEDEN OR-join taklit edilebilirdi. Reddedildi çünkü:
(a) audit'te `_join` yerine `_collapse` görünür, "join doldu" ile "biri akışı
kesti" ayrımı kaybolur; (b) `parallel.join` ölü alana dönüşür — model kendi
davranışını anlatmaz; (c) K-of-N quorum collapse ile İFADE EDİLEMEZ (collapse
sayaç tutmaz, ilk collapse otoriterdir); (d) editör OR modunu geri okurken
heuristiğe mahkûm kalırdı ("tüm kollar aynı hedefe collapse ⇒ OR").

**Runtime TEK sayı taşır.** Mod + eşik ikilisi fork anında
`ParallelSpec::quorum()` ile `Option<u32>`'a indirgenir (`None` = AND) ve
`wf.wfe.join_threshold` kolonuna yazılır. İki alanın runtime'da ayrı ayrı
yaşaması "mod or ama eşik yok" gibi tutarsız durumları mümkün kılardı. Eşik
WFD'den her seferinde okunmaz: aynı join hedefine giden iki ayrı fork mümkündür,
yani "hangi fork'un içindeyiz" bilgisi WFD'den tek başına çıkmaz.

**K = kol sayısı REDDEDİLİR** (`parallel_join_threshold`). Matematiksel olarak
AND'dir; iki yazım aynı davranışa gitse audit ve iki ayrı kod yolu (AND: kalan
aktif kol sayımı, quorum: varış sayımı) bölünürdü. Tek temsil kuralı.

**Quorum üyesi `superseded` DEĞİLDİR.** Eşiği dolduran varışta zaten varmış
kardeşler (K'ya ancak K−1 varış + bu varış ile ulaşılır) quorum'un üyesidir;
onayları geçersizleşmemiştir. WOR-60'ın `_branch_superseded` marker'ı yalnız
collapse/terminal/failed yollarında üretilir. Eşik dışında kalan AKTİF kollar ise
`cancelled` + `_branch_cancelled` alır; `_collapse` özetinin `kind`/`reason` alanı
`join_quorum`'dur — "kimse reddetmedi, join yeterli onayı topladı".

**Kol satırları quorum yolunda SİLİNMEZ.** AND-join'de `wfe_branch` satırları
join anında silinir (audit WFAH'ta). Quorum'da iptal edilen kolların satırı
`cancelled` olarak kalır: "hangi kol yetişemedi" portal tarafında görünür.

**Yarış.** Tamamlanma kararı adapter'da `FOR UPDATE` altında TEKRAR hesaplanır
(`JoinState::completes`) — engine'in (commit öncesi snapshot'a dayanan) görüşüyle
uyuşmazsa `Conflict(BranchArrival)`, executor reload edip yeniden koşar. WOR-31'in
"engine saf, adapter doğrular" sözleşmesi aynen korunur.

**Geriye uyumluluk.** `join_mode` verilmeyen WFD'ler AND'dir ve serileştirmede alan
YAZILMAZ (golden fixture birebir aynı). `wf.wfe.join_threshold` NULL = AND, veri
dönüşümü gerekmez (migration: `20260731000001_join_quorum.sql`).

## WOR-73 (2026-07-31) — ZEN join koşulu (`join_mode: expr`) + kol kimliği

**Karar.** `wft.parallel` üçüncü bir join modu kazandı: `join_mode: "expr"` +
`join_when: "<zen>"`. Gerekçe: K-of-N eşiği "üç departmandan ikisi" der ama
**"(finans VE hukuk) YA DA genel müdür"** diyemez — bu kural bir sayı değildir.
Eşik (`or`) KALDI: yaygın hâli ifade yazmadan kurulabilsin, portal "2/3" gösterirken
Zen parse etmek zorunda kalmasın.

**Kol kimliği `entry_node`'dur, `branch_node` DEĞİL.** `wfe_branch.branch_node` kol
içinde her aksiyonla değişir (`BranchMoveTo`); "finans kolu vardı mı" sorusunu o
kolonla cevaplamak, kol iki adım ilerlediğinde yanlış cevap verirdi. Fork'ta yazılan
ve BİR DAHA DEĞİŞMEYEN `entry_node` eklendi — join koşulu namespace'i
(`$branches.<entry_node>`), `_branch_*` marker'ları ve varış-kümesi doğrulaması bu
kimlikle çalışır.

**Namespace ikilidir, çünkü iki soru var.** `$branches.<kol>` "şu kol vardı mı"
(bool; hiç varmamış kol `false` döner — eksik alanın null olmasına güvenmek
gerekmesin), `$arrived` ise "kaç/hangi kol vardı" (dizi → `len($arrived) >= 2`,
`'x' in $arrived`). İkisi aynı durumun iki görünümüdür, çelişemezler. Join bağlamı
dışında boş obje/boş dizi olarak bağlanır (`$call` deseni): ifade patlamaz.

**Tatmin edilemeyen join SESSİZ KALMAZ.** `and`/`quorum` bitişi garanti eder; ZEN
koşulu etmez ("hukuk kolunu isteyen bir kural, hukuk kolu iptal edildiyse"). Son
aktif kol da varıp ifade hâlâ `false` ise WFE paralel modda kilitlenirdi. Bunun
yerine `Failed` + `end_response.reason = "WFD.JoinUnsatisfied"` (+ `join_rule`,
`arrived`). Validator'dan statik garanti İSTEMİYORUZ: ifade tatmin edilebilirliği
genel olarak karar verilemez; validator yalnız parse hatası ve bilinmeyen kol
referansını yakalar (`parallel_join_when_unknown_branch` — yazım hatası runtime'da
her zaman `false` dönen bir alan olurdu).

**Yarış doğrulaması SAYIDAN KÜMEYE geçti.** WOR-72'de adapter "kaç kol vardı"
sayarak engine'in kararını doğruluyordu; ZEN koşulu sayıyla ifade edilemediği için
bu yetersiz kaldı. Artık engine kararını hangi VARIŞ KÜMESİ üzerinde verdiyse onu
outcome'a koyar (`arrived_entries`), adapter kilit altında DB'deki kümeyle
karşılaştırır. Küme aynıysa saf engine'in kararı da aynıdır — **adapter ZEN
çalıştırmaz**; I/O katmanı motorun mantığını ikinci kez yazmaz. Bu değişiklik üç
modun HEPSİ için tek doğrulama yolu bıraktı (AND/quorum'un ayrı sayımları gitti).

**Runtime tek çözülmüş kural taşır.** `Wfes::join_rule: JoinRule` =
`All | Quorum(k) | Expr(zen)`; DB'de iki nullable kolon (`join_threshold`,
`join_when`) + `CHECK (biri NULL)`. "Mod expr ama ifade yok" gibi ara durumlar
runtime'a hiç ulaşmaz (`ParallelSpec::join_rule()` tek noktada indirger).

**Editör modeli AĞAÇ tutar, metin DEĞİL.** `ParallelStep.joinWhen` bir
`JoinCond` ağacıdır (`branch` yaprakları STEP ID taşır, `group` VE/VEYA,
`raw` elle yazılmış ZEN). Neden: panelde kol seçimli VE/VEYA ağacı kayıpsız
düzenlenebilsin ve c_a yeniden adlandırıldığında (node key değişir) koşul bozulmasın.
Metne çeviri yalnız export'ta (`compileJoinCond`), geri okuma import'ta
(`parseJoinCond`); dar gramerin (yalnız kol referansları + and/or + parantez) dışına
çıkan her ifade `raw` yaprağı olarak KORUNUR — panelde "Gelişmiş" sekmesinde görünür,
sessizce düşmez. İç gruplar daima parantezlenir, kök grup parantezlenmez → ağaç →
metin → ağaç turu birebir aynı metni üretir.

**Geriye uyumluluk.** `join_mode` verilmeyen WFD'ler hâlâ AND'dir; `join_when`/
`entry_node` yeni kolonlardır (migration `20260731000002_join_expr.sql`,
`entry_node` backfill = `branch_node`). Golden fixture değişmedi.

---

## SLA-1 ve SLA-2 paraleli sonlandırabilir (2026-08-03, WOR-56)

**Sorun.** Paralel kolda bekleyen bir iş için "kimse süresinde bakmadıysa bu paraleli
kapat, işi şuraya götür" kuralı yazılamıyordu. Collapse yalnız bir AKSİYONUN kararıydı
(`transition.wft = {collapse:{…}}`); SLA-1'in hedefi ise şemada **çıplak string** olduğu
için collapse formu fiziksel olarak temsil edilemiyordu (SLA-2'de form parse ediliyor ama
`sla_target_not_node` ile reddediliyordu). Sonuç: kimse aksiyon almazsa kol sonsuza kadar
açık kalıyor, join hiç dolmuyordu.

**Karar.** SLA-1'e opsiyonel bir BAYRAK eklendi: `claim_timeout.collapses_parallel`
(varsayılan `false`). Hedef alanının tipi DEĞİŞMEDİ — `wft` hâlâ çıplak node key'i.

- Neden yeni alan, `wft`'i `Wft` union'ına çevirmek değil: union'a geçmek wire formatını
  kırardı (tüm mevcut dokümanlar + `cross_ref` + import yolu migration'ı). Bayrak
  `deny_unknown_fields` altında ek bir alandır, eski dokümanlar bit-bit aynı kalır.
- Neden bayrak, otomatik davranış değil: collapse kardeş kolların onaylarını iptal eder.
  Bu bir politika kararıdır, bir zamanlayıcının varsayılanı olamaz — TASARIMCI ister.
- `wft` ZORUNLU olur (`claim_timeout_collapse_requires_wft`): "aynı havuza dön" ile
  collapse birlikte anlamsızdır (gidilecek hedef yok).
- Hedef hâlâ yalnız NODE (`sla_terminal_target` değişmedi): collapse paralel modu
  bitirir, AKIŞI bitirmez. Zaman aşımıyla akışı bitiren tek kural SLA-3 kalır — yani
  2026-07-28 kararı daralmadı, yalnız "kolları düşürme" yetkisi ayrı bir kapıdan açıldı.
- Paralel modda DEĞİLKEN bayrak yok sayılır, normal `{node}` devri uygulanır. Aynı node
  hem kol içinden hem dışından erişilebilir; `resolve_wft` collapse'ı Single modda hata
  saydığı için katı davranmak WFE'yi zaman aşımında kilitlerdi.
- Fork'u olmayan dokümanda bayrak ölü ayardır → uyarı (`claim_timeout_collapse_no_parallel`),
  yayın engellenmez.

**Runtime.** `fire_claim_timeout` hedefi `Wft::Collapse{collapse:{node}}`'a sarar ve
mevcut genel yoldan geçer: `CommitOutcome::CollapseTo` + `stage_parallel_markers`
(`_collapse` özeti, kardeş kollar `cancelled`, varmış kollar `superseded`). Aksiyon
collapse'ıyla tek fark tetikleyicinin system aktörü olması; audit'te SLA marker'ının
input'una `collapse: true` yazılır (bayrak yokken anahtar hiç yazılmaz — eski kayıtların
şekli korunur).

**Editör.** `ClaimTimeoutMeta.collapsesParallel` → SLA/Claim Süresi modalında
"Süre dolunca paraleli sonlandır" tiki; tik yalnız gerçekten bir kolun içindeki gruplarda
(ya da zaten işaretli kayıtta) sunulur. "Aynı havuza dön"e geçilince tik düşer. Kapılar:
`CLAIM_TIMEOUT_COLLAPSE_NO_TARGET` (hata), `CLAIM_TIMEOUT_COLLAPSE_OUTSIDE_PARALLEL` (uyarı).

**SLA-2 (aynı gün, ayrı adım).** Escalation için AYNI yetki açıldı ama YENİ ALAN
EKLENMEDİ: `escalation[].wft` zaten bir `Wft` union'ı olduğu için form yeterli —
`{collapse:{node}}`. Yani iki SLA'nın wire biçimi farklı (bayrak vs. form), sebebi tek:
SLA-1'in hedefi string, SLA-2'nin hedefi union. Editör modeli ikisini AYNI kavramla
taşır (`collapsesParallel`), fark yalnız serileştirmede.

Validator: `sla_target_not_node` artık node hedefli collapse'ı GEÇİRİR; terminal hedefli
collapse `sla_terminal_target`'a düşer (akışı bitirme yasağı korunur). Runtime:
`fire_escalation` paralel modda değilken collapse'ı düz `{node}` devrine indirger.

Eski test `escalation_collapse_target_is_error` bu kararla GEÇERSİZ oldu.

**Kapsam kuralı (aynı gün, ikinci tur).** Collapse YALNIZ bir paralel kolun İÇİNDEKİ
node'da kullanılabilir — paralel akışa bağlı olmayan bir node'un süresi dolduğunda
düşürülecek kardeş kol yoktur, ayar sessizce hiçbir şey yapmaz. İlk turda bu "dokümanda
hiç fork var mı" uyarısıydı (`*_collapse_no_parallel`); yetersizdi: fork'u OLAN bir
dokümanda kol DIŞINDAKİ bir node (join sonrası, kol dışı bir dal) hâlâ collapse
işaretleyebiliyordu.

Yerine gerçek kapsam hesabı geldi: `parallel_interior_nodes` — fork'un `branches`
girişlerinden transition kenarlarıyla BFS, join'de dur (`check_parallel`'in branch
subgraph yürüyüşünün aynısı). Kol GİRİŞİ olmak şart değil, kolun İÇİNDE kalmak şart.
Sonuç uyarı değil HATA: `claim_timeout_collapse_outside_parallel` /
`escalation_collapse_outside_parallel`. Editör aynı kuralı `parallelBranchCaGroupIds`
ile uygular (tik yalnız kol içindeki gruplarda sunulur; koldan çıkmış bir kayıt tikini
görmeye devam eder ki kaldırılabilsin).

BFS bedeli yalnız gerçekten collapse isteyen bir SLA varsa ödenir (`wants_collapse`
kapısı). Runtime fallback KALDI ama artık savunma yolu: kol içi bir node grafın başka
bir yerinden de erişilebilir, o çağrıda WFE paralel modda olmaz.

(`wfe-core/src/types/wfd_v22.rs::ClaimTimeout`, `validator.rs::check_sla`,
`v22/pipeline.rs::fire_claim_timeout` + `fire_escalation`, `docs/spec/schema.json`
(`claimTimeout.collapses_parallel`, `escalationStep.wft`, yeni `wftCollapseNode`);
editör: `types/wfd.types.ts` (`ClaimTimeoutMeta`/`EscalationMeta.collapsesParallel`),
`hooks/useExport.ts`, `utils/wfdImport.ts`, `utils/validation.ts`,
`components/shared/ClaimTimeoutModal.tsx` + `EscalationModal.tsx`,
`components/graph/PropertiesPanel.tsx`, `schema/wfd.schema.json`,
`src/tests/sla.collapse.test.ts`.)

## WOR-84 — WFAH ifade yüzeyi: `$prev`/`$first`, tam izdüşüm, iki-argümanlı gerçek (2026-08-03)

Editörün WFAH koşul kurucusu beş fonksiyon sunuyordu; **üçü çalışmıyordu**. Sebep:
zen-expression 0.55'te `count/some/all/none/one/filter/map/flatMap` **closure**
fonksiyonlarıdır ve **iki argüman** alırlar (dizi + closure), `every` diye bir fonksiyon
ise hiç yoktur.

| üretilen | sonuç |
|---|---|
| `count(filter($wfah, P)) >= n` | `ParserError` — tek argümanlı `count` grammar'da yok |
| `every($wfah, P)` | `ParserError` — fonksiyon adı `all` |
| `some/none($wfah, P)` | çalışır |

Hata yalnız **upload'ta** görünüyordu (`zen_parse`); editörün kendi önizlemesi ve
simülatörü bu fonksiyonları JS'te ayrıca hesaplayıp **yeşil** gösteriyordu. Yani
tasarımcı yayınlamaya çalışana kadar bozuk olduğunu öğrenemiyordu. Doğru form
`count($wfah, P) >= n` / `all($wfah, P)`; editör artık bunu üretir, ters ayrıştırıcı
eski iki formu da tanır (dosya bir kez açılıp kaydedilince kendiliğinden düzelir).
Yayınlanmış WFD'lerde eski metin OLAMAZ — upload kapısı zaten reddediyordu; yani
migration ve koşan WFE riski yoktur.

**`$prev` / `$first` — uç girdi namespace'leri.** "Bir önceki aksiyon şuydu" koşulu
elle `$wfah[len($wfah) - 1].action` yazmayı gerektiriyordu ve bu ifade **boş geçmişte
VM'i patlatıyor** (`Fetch: Failed to convert to usize`); parse aşaması yakalamıyor.
`$prev` (son girdi) ve `$first` (ilk girdi) bağlandı; boş geçmişte alanları `null`
döner, ifade patlamaz — `$call`/`$branches`'te kurulan "boş kabuk" deseninin aynısı.
WOR-31/WOR-56'nın paralel auto-when sentinel'i de `$wfah[len($wfah) - 1].action`'dan
`$prev.action`'a taşındı; import her iki formu tanır.

Yan karar: `$wfah`'ı doğrudan indeksleyen ifade artık `wfah_index_unguarded` **uyarısı**,
negatif indeks (`$wfah[-1]`, parse edilir/runtime'da patlar) `zen_negative_index`
**hatası** alır. Parse kapısının tek başına yetmediği kanıtlanmış durumlar bunlar.

**Tam izdüşüm.** `$wfah` girdisi yalnız `{action, actor, at}` taşıyordu; `WfahEntry`'nin
`seq` ve `input` alanları ZEN'e hiç açılmıyordu → `#.input.tutar` sessizce `null`
okuyordu ("önceki onayda girilen tutar" koşulu yazılamıyordu). İzdüşüm
`{seq, action, actor, input, at}` oldu. Not: zen'de sıralama operatörleri `null` ile
**hata** verir, bu yüzden `#.input.*` üzerinde sayısal karşılaştırma aksiyona
kapılanmalıdır (`#.action == "x" and #.input.tutar > 1000`).

**`calc` autoexec'te `$wfah` bağlı değildi.** `run_calc` yalnız ctx/node/actor/wfe_id
bağlıyordu; `$wfah` kullanan calc ifadesi sessizce yanlış hesaplıyor, `len($wfah)` ise
patlıyordu — oysa `AUTOEXEC_GUIDE` namespace'i mevcut ilan ediyordu. `ExecEnv`'e `wfah`
ve `action_input` eklendi; kapsam trigger'ın `when` guard'ıyla **aynı**: bu aksiyondan
ÖNCEKİ geçmiş (aksiyonun kendi girdisi `$action.input.*`). İki namespace'in aynı anı
göstermesi, guard ile ifadenin ayrışmasını önler. `$exec.result.*` calc içinde bilinçli
olarak bağlı DEĞİL — aynı zincirdeki ara değer `wfes_effects` ile ctx'e yazılıp `$ctx`
üzerinden okunur.

Ayrıca `calc` ifadeleri **artık upload kapısından geçiyor** (`config` şemasız `Value`
olduğu için `check_expressions` buraya hiç bakmıyordu → bozuk ifade yayınlanıp akış
koşarken `ExecFailure` veriyordu).

**Yan temizlik — `incoming_action_gates` KALDIRILDI.** Editörde geçiş panelinde "Ön Koşul:
Gelen Aksiyonlar" diye bir checkbox bloğu ve altında `Üretilen: some($wfah, …)` önizlemesi
vardı. İki kat ölüydü:

1. **Hiç render edilmiyordu.** Blok `needsGates`'e bağlıydı; o da bir ACTION adımına *gelen*
   akış arıyordu (`resolveIncomingActions(flow.from)`). Editör modelinde akışların hedefi
   daima CaGroup/switch/parallel/terminal'dir — bir action adımına giden akış hiç kurulmaz,
   dolayısıyla liste daima boş, koşul daima false.
2. **Export'a hiç girmiyordu.** `useExport` bu alanı okumuyordu; yani blok görünse bile
   gösterdiği ZEN motora ulaşmayacaktı.

Alan (`StepFlow.incoming_action_gates`), panel bloğu, `resolveFlowPath`'in bu alan üzerinden
"ön koşul zinciri" izleyen dalı (daima düz push'a düşüyordu) ve `wfahGateHelp` /
`selectAtLeastOne` sözlük anahtarları kaldırıldı. Geçmişte bir aksiyonu arama isteğinin
gerçek yolu `when` alanındaki WFAH satırıdır — bu turda ergonomisi düzeltilen yer.

**`terminal_when` DEPRECATED.** Modelde duruyor, validator ZEN'ini kontrol ediyordu,
**motor hiç okumuyordu** — v1 kalıntısı (o modelde her aksiyondan sonra koşan global bir
"akış bitti mi" guard'ıydı). v2.2'de terminal `wft: {terminal}` ile açıkça verilir;
ikinci bir terminal-belirleme yolu tek-kural ilkesine aykırı olurdu. Alan parse edilmeye
devam eder (eski dosya reddedilmesin), `terminal_when_ignored` uyarısı basar ve yeniden
serileştirmede DÜŞER — dosya bir kez kaydedilince kendiliğinden temizlenir.

(`wfe-core/src/v22/eval.rs` (`project_entry`, `empty_entry_shell`, `$prev`/`$first`),
`v22/ports.rs::ExecEnv`, `v22/pipeline.rs::execute_with_retry`, `wfe/src/runner.rs::run_calc`,
`validator.rs` (`has_negative_index`, `indexes_wfah_directly`, calc yürüyüşü,
`terminal_when_ignored`), `types/wfd_v22.rs::Wfd::terminal_when`,
`server/routes/autoexec.rs` (test rotasına örnek `wfah`/`action_input`),
`docs/spec/schema.json` + `terminology.md` + `README.md` + `AUTOEXEC_GUIDE.md`;
editör: `utils/zenUtils.ts`, `utils/zenReverseParser.ts`, `utils/zenEval.ts`,
`utils/zenHumanize.ts`, `types/wfd.types.ts`, `components/zen/*`, `hooks/useExport.ts`,
`utils/wfdImport.ts`.)

### WOR-84 (2. tur) — koşul kurucusu motorun bilmediği hiçbir şeyi kaydettirmez (2026-08-03)

Birinci tur editörün ÜRETTİĞİ metni düzeltti; elle yazılabilen yerler açık kaldı. Üç
delik vardı ve üçü de aynı sınıftı: **yayınlanabilen ama çalışmayan koşul.**

| delik | eski davranış | şimdi |
|---|---|---|
| WFAH alan hücresi serbest metin | `#.actor.name` motorda sessizce `null` → koşul hep-false; yayın kapısı da geçirir (geçerli ZEN) | alan motorun izdüşüm kümesinde değilse **hata**, Kaydet kapalı |
| WFAH/koşul değeri boş | `== ""` üretilir; eksik ctx alanı `null` okuduğu için neredeyse hep false | boş değer **hata** |
| serbest ZEN kutusu | yalnız parantez dengesi + dangling operatör bakılıyordu; `every(...)` yazılabiliyordu | bilinmeyen fonksiyon / closure arity / negatif indeks **hata**; ayrıca motorun kendi parser'ı sorulur |
| `#.input.*` üzerinde sıralama operatörü | motor RUNTIME'da patlıyordu (`Compare: Unsupported type` → HTTP 500), parse geçtiği için validator da göremiyordu | aksiyon kapısı yoksa **hata** (tablo satırı) / **uyarı** (serbest ZEN) |

**Alan kümesi motorun izdüşümünden gelir, şemadan değil.** `$wfah` tasarım-zamanı bir
nesne DEĞİL: motor her aksiyonda `WfahEntry {seq, action, actor, input, applied_at}`
yazar, ZEN'e `{seq, action, actor, input, at}` olarak açılır. Tasarımcı bunu hiçbir yere
yazmadığı için "geçerli alan" listesi de koddan gelmek zorunda —
`types/wfd.types.ts::WFAH_FIELDS` tek kaynak, `whenFields.ts::wfahFieldVerdict` onu
uygular. `actor` ve `input` çıplak hâlde de geçerlidir (`#.input == null` anlamlı bir
koşul); `input.<yol>` ise **dokümandaki tüm aksiyonların input yollarının birleşimiyle**
doğrulanır — bir WFAH satırı tek aksiyona bağlı olmadığı için "bu when'in aksiyonu"
yeterli değildir. Aynı küme serbest ZEN'deki `#.x` / `$prev.x` / `$first.x` için de
kullanılır: üçü de aynı izdüşüme baktığı için ayrı kural tutmak ayrışma üretirdi.

**Boş değer kuralı bilinçli olarak sıkı.** Yeni bir yordam/koşul satırı `value: ''` ile
açılır, yani boş değer "yarım satır" demektir. `0` ve `false` GEÇERLİ değerlerdir (boş
sayılmaz). Gerçekten boş metinle karşılaştırmak isteyen serbest ZEN satırını kullanır —
bu kaçış kapısı bilinçlidir, kural onu kapatmaz.

**Serbest ZEN: yerel sezgisel + motor onayı.** Yalnız JS ile doğrulamak WOR-84'ün ilk
turunu doğuran ayrışmayı geri getirirdi (zen grameri JS'te taklit edilemez); yalnız
motorla doğrulamak offline çalışmayı kırardı. İkisi birlikte:

1. **Yerel, anında** (`zenParser.ts`): bilinmeyen fonksiyon adı (+ `every`→`all` gibi
   öneri), closure fonksiyonlarının iki-argüman kuralı, literal negatif indeks (hata),
   korumasız `$wfah[...]` (uyarı). Fonksiyon adları artık `utils/zenFunctions.ts`'te —
   autocomplete ile doğrulama AYNI kümeye bakar; ilk turda `every` tam olarak iki ayrı
   liste tutulduğu için sızmıştı.
2. **Motor, 400ms debounce** (`POST /wfd/validate-expression`): `validator::expression_issues`
   — WFD validator'ının kullandığı fonksiyonun ta kendisi, yani rota ile yayın kapısı
   ayrışamaz. Cevap beklenirken Kaydet KAPALIDIR (o pencerede basılan Kaydet kapıyı
   anlamsız kılardı). Motora ulaşılamazsa harita boş kalır → yalnız yerel kontroller
   geçerli, Kaydet kilitlenmez ve durum footer'da söylenir.

Uyarı ile hata ayrımı korunur: `wfah_index_unguarded` Kaydet'i kapatmaz (çalışır ama
riskli), `zen_parse` / `zen_negative_index` kapatır.

**`#.input.*` sıralama kapısı.** Zen'de `null` ile sıralama `Compare: Unsupported type`
hatasıdır ve girdisi olmayan geçmiş satırı DAİMA vardır (start aksiyonu, sistem
marker'ları, gönderilmeyen optional girdi — WOR-70'te `null` yazılır). Kapısız
`some($wfah, #.input.tutar > 1000)` bu yüzden yayınlanabiliyor ama akış koşarken
`EngineError::ZenEvaluation` → **HTTP 500** veriyordu; parse geçtiği için validator da
göremez. Kuralın üç dayanağı motorda ölçülüp
`editor_zen_contract.rs::ordering_on_wfah_input_requires_a_preceding_and_gate` ile
sabitlendi:

1. kapı `and` ile ve karşılaştırmadan **ÖNCE** olmalı (zen soldan sağa kısa devre yapar
   — kapı sonra gelirse HÂLÂ patlar),
2. **`or` kapı DEĞİLDİR**,
3. dış `and`'deki kapı iç gruba geçer.

Kapı sayılan: `action == "x"` ve `action in [...]`. `action != "x"` girdinin varlığını
garanti etmediği için sayılmaz. `$prev`/`$first` kapsamı da bağışık değildir (o tek
girdinin input'u null olabilir, boş geçmişte kabuk null döner).

Tablo satırında kural **hata**dır: ağaç yapısı kesin bilinir, `whenFields.ts::wfahIssues`
`and` zincirini soldan sağa yürüyüp devralınan kapıyı iç gruplara taşır. Serbest ZEN
kutusunda **uyarı**dır: elde yalnız token'lar var, `and` sırası güvenilir çıkarılamaz ve
yanlış pozitif Kaydet'i kilitlerdi — kesin olunan yerde blokla, sezgisel olunan yerde
uyar. Kalan risk: kapılı biçim de optional bir girdi hiç gönderilmemişse patlayabilir;
kapı yaygın durumu (başka aksiyonların satırları + boş geçmiş) kaldırır, hepsini değil.

Yaprak mesajları artık KÖKTEN hesaplanır (`wfahLeafIssueMap`): kapı kuralı yaprağın
ağaçtaki KONUMUNA bağlı olduğu için tek yaprağa bakarak karar verilemez; bileşenler
nesne kimliğiyle sorgular (`issueOf`). Hata yalnız karşılaştırma yaprağına yazılır,
kapıya değil.

(`wfe-core/src/validator.rs::expression_issues` (public, tek kaynak),
`server/routes/wfd.rs` (`validate_expression` + `expression_report`),
`wfe-core/tests/validator.rs::expression_issues_matches_wfd_validator_verdicts`,
`wfe-core/tests/editor_zen_contract.rs::ordering_on_wfah_input_requires_a_preceding_and_gate`;
editör: `utils/zenFunctions.ts` (yeni), `utils/zenParser.ts` (`extractZenCalls`,
`hasNegativeIndex`, `indexesWfahDirectly`, `hasUngatedInputOrdering`, `zenSyntaxWarnings`,
`ZenRefs.wfahFieldRefs`), `utils/whenFields.ts` (`wfahFieldVerdict`, `wfahLeafIssues`,
`wfahLeafIssueMap`, `collectAllActionInputPaths`, `zenConditionIssues` artık tek bağlam
nesnesi alır), `hooks/useEngineExpressionVerdicts.ts` (yeni),
`api/engineApi.ts::validateExpressionsOnEngine`, `components/zen/*`,
`components/shared/WhenModal.tsx`.)

---

## DB bağlantı kapsamı (2026-08-04): global (tenant) + lokal (tek WFD)

**Sorun:** `wf.db_connection` tek kapsamlıydı — `UNIQUE (orgtnt_id, name)` ile tenant
genelinde. Yani bir WFD için tanımlanan bağlantı tenant'taki HER WFD'nin listesinde
çıkıyordu ve yönetimi de WFD editörünün ayarlar sekmesindeydi (ortak bir kaydı bir WFD'nin
içinden düzenlemek). İki ihtiyaç ayrışmadı: kurumsal, her projede kullanılan bağlantılar
ile yalnız tek bir akışın kullandığı bağlantılar.

**Karar:** `db_connection.scope ∈ {global, local}`.

1. **global** — tenant genelinde; her projedeki her WFD'de görünür ve SQL autoexec
   adımlarında kullanılabilir. **Ayarlar sayfasından** yönetilir. Sahiplik alanları
   (`project_id`, `wfd_name`) NULL'dur. Proje seçimi YOKTUR: global = tüm projeler
   (proje bazlı görünürlük istenirse ayrı bir eşleme tablosu gerekir, bilinçli olarak
   alınmadı).
2. **lokal** — yalnız TEK bir WFD'de görünür/kullanılabilir; başka WFD'nin listesinde
   çıkmaz. **WFD ayarları sekmesinden** yönetilir.

**Lokal sahiplik anahtarı `(project_id, wfd_name)`'dir — `wfd_id` DEĞİL.** Çünkü
`wfd_meta`'da her versiyon AYRI bir `wfd_id` satırıdır (`wfd_id` PK): `wfd_id`'ye bağlamak
yeni versiyon yayınlandığında bağlantıyı koparırdı, oysa WFD JSON'undaki
`autoexec.<k>.config.connection` uuid'i versiyonlar arası kopyalanır. Mantıksal WFD kimliği
bu repoda her yerde `(project_id, name)`'dir (bkz. `wfd_meta_project_name_version_key`,
`wfd_single_draft`). Bunun iki sonucu var ve ikisi de bilinçli:
- Grup yeniden adlandırılınca lokaller de taşınır — `repo::update_group_metadata`
  ifadesindeki `renamed_conns` CTE'si (veri değiştiren CTE referans edilmese de bir kez ve
  tam olarak koşar).
- Gruptaki son satır silinince lokaller sahipsiz kalırdı → `repo::delete_draft` aynı
  transaction'da onları da siler.

**İsim benzersizliği kapsam başına iner:** kısmi unique index'ler
`db_connection_global_name (orgtnt_id, name) WHERE global` ve
`db_connection_local_name (project_id, wfd_name, name) WHERE local`. Aynı ad bir global ve
bir lokalde yan yana durabilir (referans uuid'dir); editör seçicisi lokali işaretler.

**Kapsam create'te belirlenir, update'te DEĞİŞMEZ.** Global'i lokale (ya da tersine)
çevirmek, ona referans veren WFD'lerin görünürlüğünü sessizce kaydırırdı.

**Yüzey.** `GET /db/connections?orgtnt_id=..&wfd_id=..` — `wfd_id` verilirse global'lerin
yanına O WFD'nin lokalleri eklenir, verilmezse (ayarlar sayfası) yalnız global'ler döner;
her satır `scope` taşır. `POST` gövdesi `scope` (default `global`) + `scope=local` için
zorunlu `wfd_id` alır; grup kimliği sunucuda çözülür. WFD ekranında global satırlar
salt-okunurdur (test AÇIK, düzenle/sil KAPALI) — düzenleme tüm projeleri etkilerdi.

**Yazma kapısı.** `POST /wfd` ve `PUT /wfd/draft/{id}/{v}`, doküman BAŞKA bir WFD'ye ait
bir lokal bağlantıya referans veriyorsa `422` döner
(`routes::db::assert_no_foreign_local_connections`). Elle düzenlenmiş/kopyalanmış JSON
içindir; editör listesi zaten kapsamla filtreli. Bilinmeyen (silinmiş) id'ler hata DEĞİL —
eskiden de kaydedilebiliyorlardı, çalışma anında "connection bulunamadı" ile düşerler.

(`migrations/wf/20260804000001_db_connection_scope.sql`,
`server/src/routes/db.rs`, `server/src/routes/wfd.rs`, `wfd/src/repo.rs`; editör:
`components/engine/DbConnections.tsx`, `components/engine/EngineSettings.tsx`,
`components/shared/AutoexecConfigModal.tsx`, `api/engineApi.ts`.)


---

## Ek-belge deposu WFD BAŞINA (2026-08-07)

**Sorun:** Depo tek bir deployment ayarıydı (`ATTACHMENT_STORAGE_*`) — tüm tenant'lar ve
akışlar aynı bucket'a yazardı. Kurumsal gereksinim bunun tersi: bir akışın belgeleri
müşterinin kendi S3'ünde durabilmeli, üstelik ortama göre (test/prod ayrı bucket).

**Karar:** Konfigürasyon WFD DOKÜMANINA GİRMEZ, `$env`ten okunur (`wf.wfd_env_var`,
sahiplik `(project_id, wfd_name)`). Gerekçe `$env`in var oluş gerekçesidir: doküman
`(wfd_id, version)` bazında immutable'dır, prod bucket'ı değişince yeni versiyon
yayınlamak gerekmemeli. Anahtar İSİMLERİ sözleşmedir:

| Anahtar | Anlam |
|---|---|
| `ATTACHMENT_STORAGE_BACKEND` | `local` \| `s3`. Yoksa/tanınmazsa deployment varsayılanı |
| `ATTACHMENT_STORAGE_PATH` | local kök |
| `ATTACHMENT_STORAGE_S3_BUCKET` / `_S3_REGION` / `_S3_ENDPOINT` | S3 hedefi |
| `ATTACHMENT_STORAGE_S3_ACCESS_KEY_ID` / `_S3_SECRET_ACCESS_KEY` | kimlik (secret girilir) |

Secret'lar yalnız bu katmanda çözülür (`RunEnv::full()`); ZEN/effects onları göremez.
Tanınmayan `BACKEND` değeri sessizce local'a düşmez — yanlış yazılmış bir değer yüzünden
belgelerin müşterinin bucket'ı yerine sunucu diskine yazılması fark edilmesi en zor hata
sınıfıdır; konfigürasyon yok sayılır ve deployment varsayılanına dönülür.

Operator ÖNBELLEKLENİR (anahtar = çözülmüş konfigürasyonun kendisi): S3 istemcisi kurmak
her istekte yapılacak iş değildir, konfigürasyon değişince anahtar da değişir.
(`server/src/attachment_store.rs`; tüm attachment rotaları ve gate'ler bu çözücüden geçer.)

---

## Şema kapısı motorda + çapasız C_A (2026-08-07)

İki karar, tek yerde: ikisi de "elle yazılan JSON kafaya göre olmasın" ekseninde.

### 1. `docs/spec/schema.json` artık RUNTIME kapısıdır

**Sorun:** Şema kanonik dosyaydı ama hiçbir yerde koşmuyordu. Backend'in kapısı
`wfd_version` + serde + `validator`'dı; serde `#[serde(default)]`li alanları eksik kabul
eder, `minItems`/`uniqueItems`/`pattern` gibi kısıtları hiç bilmez. Tek zorlayıcı ajv ile
EDİTÖRdü. Sonuç: editörü atlayıp API'ye POST edilen bir belge şemayı ihlal ettiği halde
kabul ediliyordu — ör. `"c_r": []` şemada `minItems: 1` ile yasak, serde için `Some([])`,
motor için "rol kanalı kapalı". Belge geçersizdi ve çalışıyordu.

**Karar:** Şema `include_str!` ile motora GÖMÜLÜR (`wfe_core::schema`) — binary ile spec
ayrı düşemez. Kapı `Wfd::from_value_checked` / `from_json_checked`'te; koştuğu yerler:
upload, publish, submit, approve, **fetch (okuma)**, `/wfd/validate`, `/wfe/simulate`,
senaryo koşumu. Sürüm kapısı şemadan ÖNCE koşar: 2.1 belgesi 2.2 şemasına karşı onlarca
ihlal üretir, gerçek sebep o gürültüde kaybolur.

Okuma da doğrulanır (bilinçli, kullanıcı kararı): depodaki her belge şemaya uygun olmak
zorunda. Riski kabul edildi — şemayı ihlal eden yayınlanmış bir belge varsa o akış 422'ye
düşer ve yeniden yayınlanması gerekir. Editör bugüne kadar `c_r: []` üretiyordu ama kendi
ajv kapısı o belgeyi zaten reddediyordu (`serializeAndValidate`), yani depoda olması
beklenmez; elle POST edilmiş belgeler taranmalıdır.

Taslak KAYDI (`save_draft`) kapsam DIŞI: yarım belge kaydedilebilir, yayınlanamaz. Ham
`from_value` de kasıtlı olarak açık kalır (testler iskelet belge kurar).

### 2. `c_orgu` opsiyonel — çapasız C_A (yalnız `c_u`)

**Sorun:** "Şu kişi, hangi birimde olursa olsun bu node'u yapabilsin" ifade edilemiyordu.
`{ "c_orgu": "self", "c_u": ["ayse"] }` yaklaşımı TUTARSIZ çalışıyordu: `authorize`
`self`'i claimant'ın kendi birimine çözdüğü için claim kapısı daima açılır, ama havuz
listesi denormalize `current_c_a` cache'inden okunur ve o cache node'a GİRİŞTEKİ aktörün
birimiyle donar → kişi başka birimdeyse görevi listede hiç görmez, ama wfe_id'yi bilse
claim edebilir.

**Karar:** `c_orgu` HİÇ verilmeyebilir. O zaman orgu kanalı kısıtsızdır ve kural
`match = user_match`'e indirgenir. Bu biçimde **`c_u` zorunlu, `c_r` YASAK**:

- Kişi kanalı adı adı sayılmış bir istisna listesidir; çapasız hali de sayılabilir kalır.
- Rol kanalının çapasız hali ("tenant'taki tüm müdürler") kurulabilecek en geniş kapıdır ve
  `c_orgu` yazmayı unutan tasarımcının kazara ürettiği şey tam olarak odur. Ayrıca aday
  cache'i ORGU × rol satırı üretirdi (sınırsız).

Üç katman reddeder: şema (`candidateActor.oneOf`), validator (`c_a_anchorless_role`,
`c_a_anchorless_needs_user` — `reassign` dahil), matcher (çapasız kuralda rol kanalını hiç
sormaz). Tek noktaya güvenilmiyor çünkü ikisi de belgeyi başka bir yoldan girmiş olabilir.

**Aday cache:** çapasız kural kişi başına TEK girdi yazar — `orgu_id` YOK, `any_orgu: true`
(`types::actor::CandidateActor`). Tenant'taki her ORGU için satır üretmek hem sınırsız hem
yanlış olurdu (küme org ağacı değiştikçe değişir, cache node'a girişte donar). Havuz sorgusu
(`portal/pool.rs`) bu girdiler için AYRI containment filtreleri koşar. İşaret açıktır
(`any_orgu`) çünkü çıplak `[{"user_id": U}]` sorgusu aynı kişinin BAŞKA bir birimdeki
scope'lu grantını da kapsardı — jsonb `@>` alt küme sorar.

**Node key:** orgu parçası `any` (`ANCHORLESS_SLUG`) → `{ "c_u": ["ayse"] }` = `any__u_ayse`.
Çakışma imkânsız: ORGTRVLANG ifadesi `self` ya da `*:` ile başlamak zorundadır.

---

## WFE not defteri: motorun dışında ÜÇÜNCÜ katman, `$notes` namespace'i yok (2026-08-10)

**Sorun:** İş elden ele geçerken tasarım anında öngörülemeyen insan-insana mesaj/belge
paylaşımı gerekiyor ("kredi miktarını yükselt, öyle yolla"). Bunu WFD'ye node alanı olarak
eklemek tam da öngörülemeyeni öngörmeyi ister ve her seferinde yeni versiyon publish
gerektirir. Konuşmayı nereye yazmalı: `$ctx` mi, `$wfah` mı, ayrı bir yer mi?

**Karar:** Ne `$ctx` ne `$wfah` — ayrı, şemasız, örnek (WFE) bazlı bir defter
(`wf.wfe_note`), engine core bundan tamamen habersiz. İki adayın elenme gerekçesi kalıcı
bir sınır çiziyor:

- **`$ctx` değil:** context'e tek yazma yolu `wfes_effects`'tir ve alan tipleri
  `collectActionInputCtxMap` üzerinden çıkarılıp `expr_types.rs` ile denetlenir. Ad-hoc,
  şemasız bir not bu çıkarımı bozar; ayrıca "kim, ne zaman yazdı" bilgisini context zaten
  taşıyamaz (context bir değer kutusu, bir kayıt defteri değil).
- **`$wfah` değil:** yayınlanmış akışlar bu defteri `count($wfah, #.action == "x") >= n`
  gibi **sayarak** okuyor. Araya bir sistem-notu satırı koymak (`__note` aksiyonu gibi) bu
  sayımı, `$prev`/`$first` kısayollarının anlamını ve `project_entry` izdüşümünü kaydırır —
  motorun resmi defterine insan yorumu karışmaz.

Bu ikisi elenince kalan tek tutarlı seçenek üçüncü bir katmandı; `attachments`'ın zaten
kurduğu düzenin (metadata DB'de, dosya I/O'su portal edge'inde, engine dışarıda) aynısı not
için de tekrarlandı.

**`$notes` diye bir ZEN namespace'i BİLEREK yoktur.** Sınır "motoru okur mu" sorusuyla
çizilir: bir içerik akışın KARARINI etkileyecekse artık ad-hoc değildir, tasarım verisidir
ve doğru yer WFD'de tanımlı bir action input alanıdır — not defteri bunun kaçış yolu
olmamalı. `$notes` eklemek bu sınırı bulanıklaştırır: tasarımcı "yükselt" notunu okuyup
koşul yazmaya başladığı an not, denetlenmeyen bir ikinci `$ctx`'e dönüşür. İleride gerçek
bir talep gelirse `v22/dollar.rs` (`EXACT`/`PREFIXES`) + `expr_types.rs` genişletilerek
salt-okunur bir namespace açılabilir — bu tasarım o kapıyı kapatmaz, sadece bugün açmaz.

Detay (K1–K9, veri modeli, API yüzeyi, fazlar): `docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md`.
Sözleşme özeti: CLAUDE.md "WFE not defteri (ad-hoc not + belge)".

## T‑A5 — WF Admin: akış-içi yetkili (`wfd.wf_admin[]`, 2026-08-11)

**Sorun:** Tıkanmış bir akışa müdahale etme yetkisi yalnız NODE başına tanımlanabiliyordu
(`node.reassign`, Madde 7). 20 node'lu bir akışta aynı amiri 20 yere yazmak gerekiyordu; ve
escalation sayaçlarına çalışma anında dokunmanın hiçbir yolu yoktu.

**Karar:** WFD köküne `wf_admin[]` eklendi — `listable[]` ile AYNI şekil (`{c_a, when?}`,
tip `CaGrantRule`) ve AYNI matcher. Bu kurallardan birine uyan aktör:

1. **claim devredebilir** — node'un kendi `reassign` kuralı olmasa bile. Kapı iki yollu:
   `node.reassign eşleşir VEYA wf_admin eşleşir`. Hedef hâlâ node `c_a`'sına uymak
   zorundadır (`TargetNotEligible`): uymayan kişi claim'i tutar ama `apply_action` c_a'yı
   yeniden sorduğu için hiçbir aksiyon alamaz — WF Admin akışı kilitlemiş olurdu. Devir
   marker'ının `input`'una `"via": "wf_admin"` yazılır; node amiri yolunda YAZILMAZ
   (eski kayıtların şekli korunur). Bu, T‑A6'yı ("amir işi doğrudan bir kişiye
   claim'letsin") da karşılar.
2. **escalation sayacına müdahale edebilir** — `POST /wfe/:id/escalation/fire` (sıradaki
   adımı vade beklemeden uygula) ve `.../skip` (adımı atla). Adım numarası istemciden
   ALINMAZ: sıradaki ateşlenmemiş adım işlenir, böylece adımların sıralı olma sözleşmesi
   korunur. Yetki YALNIZ `wf_admin`'dir — `node.reassign` bunu açmaz.
3. **WFE'yi görebilir** — `can_view` (e) kriteri. Ayrı kriter olmasının nedeni: yönettiği
   akışı göremeyen admin işe yaramaz, ve aynı kuralı bir de `listable`'a yazdırmak
   ikisinden birinin unutulmasıyla biter.

**Marker sözleşmesi.** Elle tetikleme OTOMATİK yolun aynı marker'ını yazar
(`escalate:<node>:<idx>`): yayınlanmış akışlar `count($wfah, #.action == "escalate:...")`
ile karar veriyor, ayrı ad o sayımları bozardı. Ayrım AKTÖRDEDİR (otomatikte system,
ellede admin). `wfes_effects` bağlamındaki `$actor` ise system KALIR — effects akışın veri
semantiğidir, elle tetikleme onu değiştirmemeli.

Atlama marker'ı `escalate:<node>:<idx>:skipped`. `escalate:` öneki ZORUNLUDUR:
`next_escalation` node giriş zamanını "son escalation-DIŞI WFAH kaydı"ndan hesaplıyor, yani
başka bir adla yazılan atlama marker'ı tabanı kendine kaydırır ve o node'un TÜM
sayaçlarını sessizce sıfırlar. Atlama adım başınadır; "escalation'ı komple kapat" yoktur.

**Atlama `commit` değil `append_marker` kullanır** (yeni `WfeStore` metodu, varsayılan
implementasyon YOK): atlama bir geçiş değil, yalnız "bu adım kapandı" defterine düşen bir
kayıttır. `release_claim` claim'i de temizlerdi, `commit` node/status taşırdı. Varsayılan
no-op bir implementasyon, atlamanın çalıştığını sanıp hiçbir şey yazmayan bir store'a izin
verirdi — adım bir sonraki turda yine ateşlenirdi.

**WF Admin'in YAPAMADIKLARI:** akışı bitirmek/iptal etmek (yalnız aksiyonlar ve SLA-3),
rastgele node'a taşımak, `$ctx`'e yazmak, kendi adına aksiyon uygulamak (bunun için node
`c_a`'sına uyması gerekir). **WF Admin işi yönetir, işi yapmaz.**

**Reddedilenler:** node başına `wf_admin` (zaten `node.reassign`); yeni C_A kanalı
`c_admin` ("C_A TEK KURALDIR" bozulur); yetkiyi tenant permission havuzuna (`org.p`)
bağlamak (havuz STATİK iş yetkileridir, "başlatanın şubesindeki müdür" gibi akış örneğine
göre değişen yetki orada ifade edilemez); escalation ötelemesi (adım başına offset yeni
kalıcı durum ister, "saati sıfırla" ise WFAH tabanını örtük kaydırır — ihtiyaç atlama ile
karşılanır).

Detay: `docs/superpowers/specs/2026-08-11-wf-admin-design.md`.
Sözleşme özeti: CLAUDE.md "WF Admin (akış-içi yetkili)".

## T‑B4 — WFD taslak kilidi: pessimistic (2026-08-11)

**Sorun:** `save_draft`'ta eşzamanlılık denetimi yoktu — son yazan kazanıyordu. İki
tasarımcı aynı taslağı açıp çalışırsa ikinci kaydeden birincinin emeğini sessizce siler.

**Karar:** Pessimistic kilit. `wf.wfd_meta`'ya üç kolon (`lock_user_id`,
`lock_acquired_at`, `lock_expires_at`) — AYRI TABLO DEĞİL, çünkü kilit taslakla 1:1 ve
taslak zaten o satırdır; böylece kilit koşulu mutasyonun kendi `WHERE`'ine girebiliyor
(kontrol-sonra-yaz açığı olmadan).

**Alma ve tazeleme AYNI ifadedir** — tek `UPDATE`, `WHERE` cümlesi CAS:
`(lock_user_id IS NULL OR lock_user_id = $user OR lock_expires_at <= now())`. Sıfır satır
→ başkasında canlı kilit. `lock_acquired_at` tazelemede DEĞİŞMEZ (`COALESCE`): "bu kişi
bu taslağı ne zamandır tutuyor" ancak böyle cevaplanır.

**Süresi geçmiş kilit yok sayılır, SİLİNMEZ** — `lock_expires_at <= now()` koşulu onu
zaten geçirir, süpürücüye gerek yok; kolonlar son sahibin izini taşımaya devam eder.

**TTL 5 dakika ve tazeleme YALNIZ İNSAN eylemiyle olur** (kör zamanlayıcı YOK). Editör
`T-60s`'de "Devam et / Kaydet / Kaydetmeden çık" sorar; `T-30s`'de cevap yoksa ÖNCE
KAYDEDER sonra kilidi bırakır. Popup `T-0`'da değil `T-30s`'de karar verir: otomatik
kaydetme tam bitiş anında koşarsa kilit düşmüş olabilir ve emeği kurtaracak mekanizma tam
o anda `409` alır. Bu tasarım "açık ama idle sekme taslağı rehin alır" deliğini kapatır.

**Kilit TÜM taslak mutasyonlarında zorunlu** (kaydet / yayınla / onaya gönder / sil):
A kilidi tutup düzenlerken B yayınlarsa A'nın YARIM işi yayına çıkar — kaydetmeyi
korumak tek başına yetmez. Onay/ret kilit İSTEMEZ (pending satır düzenlenemez).
Başarılı publish/submit kilidi BIRAKIR.

**`publish`/`submit` kilidi ROTADA da sorar** (`require_draft_lock`): o rotalar adapter'a
girmeden belgeyi parse eden ön kapılar koşuyor (`assert_env_keys_defined`,
`assert_attachment_storage_env`); kilit önce sorulmazsa yetkisiz kullanıcı `422` alır ve
"JSON'u düzelt" gibi yanlış yola sevk edilir. Asıl kapı hâlâ mutasyonun `WHERE`'inde.

**Kilit durumu `GET /draft/{id}/{ver}` yanıtına GÖMÜLMEZ**, ayrı `GET .../lock` ucu
vardır: draft GET'i ham WFD belgesini döndürüyor ve kökü `additionalProperties: false` —
kilit alanları belgeyi geçersiz kılardı.

**İki hata kodu:** `draft.locked` (başkasında → kullanıcıya gösterilir) ve
`draft.lock_required` (kimsede değil / sende değil → istemci kilidi alıp kendiliğinden
tekrar dener). Tek kod olsaydı istemci ikisini metinden ayırmak zorunda kalırdı.

**Bilinçli sözleşme kırılması:** kimse kilidi tutmuyorsa da kaydetme reddedilir; aksi
halde kilit almayan iki istemci birbirini yine ezer ve mekanizma dekoratif kalır.
Taslaklar agnoflow'a özgüdür (başka istemci taslak kaydetmez), etki yalnız editördedir.

**Zorla açma (`?force=true`) EKLENMEDİ:** gözetimsiz kilit 5 dakikada kendiliğinden
düşer, klasik "çöken sekme" vakası yok. Seam hazır — `DELETE .../lock`'a admin dalı.

Detay: `docs/superpowers/specs/2026-08-11-draft-kilidi-design.md`.
