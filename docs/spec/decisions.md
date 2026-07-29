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

> **Kısmen GEÇERSİZ (WOR-70, 2026-07-29):** bu maddede tarif edilen "başlangıç ctx'i
> declared yollardan tohumlanır" ve "`context.required` FINAL ctx üzerinde denetlenir"
> davranışları kaldırıldı. Input artık ctx'e hiç yazmaz; `context.required` yoktur.
> Geçerli sözleşme: bu dokümanın sonundaki **WOR-70** maddesi.

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
  **gizle** → `$wfah[len($wfah) - 1].action != "<entry>"`.
- **Collapse** aksiyonu (WOR-56): yalnızca paralel bağlamdayken **göster** →
  `$wfah[len($wfah) - 1].action == "<entry>"`. Tam ters operatör, **aynı**
  entry-action hesabı (interior BFS: direkt dal node'unda entry=fork; derin
  node'da entry=oraya götüren dal aksiyonu; birden çok entry → OR:
  `==e1 or ==e2`).
- Kullanıcı kendi when'ini yazarsa `(user) and (auto ==)` düz top-level AND.
- Neden aynı `entry` sinyali: motor ZEN'e paralel-durumunu açmaz
  (`current_node=NULL`, kollar `wfe_branch` tablosunda); ZEN yalnızca
  DynCtx + WFAH görür → "paralel içindeyim" ancak "önceki aksiyon == entry"
  ile türetilir. Independent bunun `!=`'ini, collapse `==`'ini kullanır.
- Import round-trip: sentinel regex (`wfdImport.ts`) `!=` VE `==` varyantlarını
  yakalamalı; strip mantığı aynı (her clause ayrı top-level terim).

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
2. **Engine saf kalır; gate portal edge'inde.** wfe-core yalnız katalog + referansı
   METADATA olarak taşır, dosya I/O YAPMAZ. Varlık kontrolü ve yükleme server'ın portal
   katmanındadır: `AttachmentStore` (opendal; local fs default kök `../work-pool-portal/
   storage`, `ATTACHMENT_STORAGE_*` env, S3'e geçince aynı arayüz). Storage anahtarı
   `attachments/{wfe_id}/{grup}/{item}` — aynı grubu referanslayan farklı node'lar dosyayı
   tekrar istemez.

**Akış.** "Hangi aksiyonlar alınabilir?" (`GET /wfe/:id/attachments`, direkt X-Actor
ağacı) → aktörün gördüğü node(lar)ın referanslı gruplarının item bazlı yükleme durumu +
`satisfied`. UI, `satisfied=false` iken submit'i disable eder. Zorlama server-side:
`apply_action` (ve JWT `submit_action`) hedef node'un `required` dosyaları eksikse
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
