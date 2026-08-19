# CLAUDE.md — agnoflow-engine

Bu repo **WFD v2.2** (Named Nodes, Single-Rule C_A) modelini çalıştıran çok-tenant'lı
workflow engine'dir. Spec ile kod çelişirse SPEC kazanır: kanonik dosyalar
`docs/spec/` altındadır (kaynak: WFD-EDITOR reposu `docs/spec/`; senkron tutulur).
Alınan tasarım kararları: `docs/spec/decisions.md`.

## Crate haritası

| Crate | Sorumluluk |
|---|---|
| `wfe-core` | Saf engine. `types/wfd_v22` (model + `canonical()` tekillik formu; **slug ÜRETİMİ 2026-08-14'te SİLİNDİ** — kimlik tasarımcının, slug önerisi editörün işi), `validator` (cross-ref/`duplicate_c_a`/graf/expression), `v22/` runtime: `eval` (ZEN namespace'leri), `resolver` (c_orgu), `matcher` (§3 authorize), `visibility` (§4 AYRI matcher), `effects` ($-string), `pipeline` (§7 atomik transition + trigger retry/catch/timeout + escalation), `ports` (WfdStore/WfeStore/AutoexecRunner). I/O YOK. |
| `org` | ORGT/ORGU/ORGTNT/UR repo + ORGTRVLANG parser/executor (ltree SQL). |
| `wfd` | WFD depolama: meta PostgreSQL, JSON OpenDAL. Upload/fetch'te v2.2 kapısı + validator; (wfd_id,version) immutable cache. |
| `wfe` | Adapter'lar: `WfeAdapter` (WfeStore — create/commit TEK transaction, claim CAS), `OrgAdapter`, `LiveAutoexecRunner` (rest/sql/calc), `WfeExecutor` (orkestrasyon + `tick_timers`), `sim` (store'suz simülasyon durumu). |
| `server` | Axum API: `/wfd` (upload/validate/list/get), `/wfe` (start/apply/claim/query/possible-actions/list), `/wfe/simulate`, `/autoexec/test`, `/portal` (JWT: login, pool, wfd, wfe), `/org` (X-Admin-Key). 60s timer sweeper. |

## Değişmezler (spec'ten)

- **Node kimliğini TASARIMCI verir** (2026-08-12, KIRICI): `node key == slug(c_a)` kuralı
  KALDIRILDI (`validator.rs` §2b notu). `c_a` node'un bir ALANIDIR, kimliği değil → org
  yolu (ORGTRVLANG) artık anahtara sızmıyor ve "kim yapar"ı değiştirmek adımın kimliğini
  bozmuyor. Biçim kısıtı şemada duruyor (`nodes` propertyNames: `idName`).
  **Veri taşıması YOK** — yalnız kısıt kalktı, mevcut anahtarlar deseni zaten sağlıyor.
- **Aynı `c_a` = aynı kimlik** (2026-08-14, KIRICI — geri getirildi): `duplicate_c_a`
  yeniden **HATA** (2026-08-12'de `shared_c_a` uyarısına çevrilmişti). Bir canonical `c_a`
  belgede EN FAZLA BİR node'da bulunabilir; aynı kimlik daima aynı `c_a`'yı taşır. Kimlik
  hâlâ tasarımcınındır — geri gelen tek şey TEKİLLİK. Ardışık adımların ("müdür inceler" +
  "müdür onaylar") farkı TEK node + aksiyonların `when`iyle (`$wfah`) verilir. **Paralel
  kolda "aynı havuzdan iki kol" ŞİMDİLİK DESTEKLENMEZ** (kol kimliği node anahtarıdır →
  K-of-N quorum'un N kolu aynı havuza bakamaz) — bilinçli, GEÇİCİ kısıt. Gerekçe ve feda
  edilenler: `docs/spec/decisions.md` 2026-08-14; kırıcılık + düzeltme reçetesi:
  `docs/spec/migration-notes.md` M18.
- **Ham JSON'da ÇİFT node anahtarı REDDEDİLİR** (`wfe_core::dupkeys`): `serde_json` çift
  anahtarı hata saymaz, sessizce SONUNCUYU alır — kimlik tasarımcıya geçtiği için iki
  adım aynı adı alırsa biri iz bırakmadan kaybolur ve akış çizilenden başka bir şey
  yapardı. Kapı ancak HAM METİNDE kurulabilir (`Value`ya dönmüş belgede çakışma zaten
  silinmiştir): `Wfd::from_json`/`from_json_checked` + `POST /wfd` (bu uç bu yüzden
  `Json<UploadBody>` değil `Bytes` alır).
- C_A **TEK KURALDIR**, iki biçim: **çapalı** `{c_orgu, c_r?, c_u?}` → match = `resolved(c_orgu) AND (rol OR c_u)`; **çapasız** `{c_u}` (c_orgu HİÇ yok) → match = `c_u`, kişi tenant genelinde eşleşir. Çapasızda `c_u` zorunlu, **`c_r` YASAK** (şema `oneOf` + validator `c_a_anchorless_role` + matcher rol kanalını hiç sormaz). Verilmeyen alan **false** (wildcard değil); c_u rol-agnostik. Çapasız aday cache girdisi birim taşımaz (`any_orgu: true`), görünürlük predicate'inde ayrı kanal (`ViewerFilters`).
- Transition: `from` + `action`; aynı (node, action) için array sırasında İLK when-match.
- wft: `{node}` / `{terminal}` / `{conditions[], default?}`; default yoksa `WFD.NoConditionMatched`.
- Pipeline atomiktir: tüm diff'ler staged, `WfeStore::commit` tek transaction; unhandled fail'de hiçbir şey yazılmaz. Node değişiminde assignment (claimed_by) sıfırlanır.
- Visibility matcher'ı authorization'dan AYRIDIR; kriterler arası OR.
- ZEN namespace'leri: `$ctx $wfah $prev $first $node $actor $timestamp $wfe_id $action.input.* $exec.result.*` (`$exec.response.*` = hata). `$wfah` girdisi `{seq, action, actor, input, at}`; `$prev`/`$first` uç girdi kısayolları, boş geçmişte null (patlamaz). `$wfah`'ı DOĞRUDAN indeksleme (`wfah_index_unguarded` uyarısı; negatif indeks `zen_negative_index` hatası).
- **Dizi fonksiyonları İKİ argümanlı** (WOR-84): `count($wfah, #.action == "x") >= n` ✅ — `count(filter(...))` parse HATASI, `every` diye fonksiyon YOK karşılığı `all`. Tam liste: `count some all none one filter map flatMap`.
- **`#.input.*` sıralama karşılaştırması aksiyon kapısı İSTER**: `null` ile `>` `<` zen'de `Compare: Unsupported type` (runtime, parse yakalamaz). Kapı `and` ile ve karşılaştırmadan **ÖNCE** olmalı; `or` kapı değildir; dış `and`'deki kapı iç gruba geçer. `$prev`/`$first` de bağışık değil. Sözleşme testi: `tests/editor_zen_contract.rs`.
- **İfade TİP denetimi motordadır** (`wfe-core/src/expr_types.rs`, AST tabanlı): obje karşılaştırması (`zen_object_compare` — **obje==obje dahil**, VM eşleştirmez), metinde sıralama (`zen_ordering_not_number`), iki taraf tip uyuşmazlığı (`zen_type_mismatch`), izdüşüm dışı `$wfah` alanı (`zen_wfah_field_unknown`), kapısız `#.input.*` sıralaması (`zen_input_needs_action_gate`), liste öğesi tip uyuşmazlığı (`zen_list_type_mismatch` — `In` opcode'u öğe öğe `Equal` yapar, `#.seq in ["a"]` hep-false), metin operatörünün metin olmayan tarafı (`zen_text_op_not_string` — `contains`/`startsWith`/`endsWith`/`matches`), `#.at` sabitinin biçimi (`zen_timestamp_format` — `at` düz METİNDİR, `yyyyMMddHHmmss`/14 rakam UTC; karşılaştırmaları STRING temellidir, `d()` yok. Eşitlik/`in` tam damga ister, `startsWith` anlamlı önek sınırı (4/6/8/10/12/14), `contains`/`endsWith` yalnız rakam, `matches` muaf. Sıralama `zen_ordering_not_number`a düşer). `#.input.<yol>`un tipi girdiyi context'e yazan `wfes_effects` üzerinden çıkarılır — editör de aynı çıkarımı yapar (`whenFields.collectActionInputCtxMap`). **Elle yazılan JSON ile editörün ürettiği JSON aynı kapıdan geçer**; kural seti motorun, editör yalnız aynı cevabı önden verir.
- **Editör ifade doğrulaması `POST /wfd/validate-expression`** ile motora sorulur — `validator::expression_issues` (WFD validator'ının kullandığı fonksiyonun aynısı). Yeni bir ifade-yüzeyi kuralı eklenirken O fonksiyona yazılır, iki tüketici birlikte güncellenir. **İstek gövdesinde `wfd` de gider** (editör `serializeWfdPreview` ile yollar): belge varsa TİP kuralları da koşar ve yanıt `typed: true` döner; yoksa/parse edilemezse yalnız yüzey kuralları koşar (`typed: false`) — kurucu yarım taslakta da çalışmalı.
- **Tanınmayan `$` referansı yayını ENGELLER** (`unknown_dollar_ref`): motor çözemediği `$`-string'i HATA saymaz, alana düz METİN yazar (`effects::resolve_dollar_string` son satırı) → `$actor.role` / `$call.state` gibi yazım hataları yayında iz bırakmadan sessiz bozukluk üretiyordu. Gramerin tek kaynağı `v22/dollar.rs`; denetlenen yerler çözücülerin olduğu yerlerdir (`wfes_effects.set`, `calls[].input`, `terminals[].wfe_end_response`, `autoexec[].config` — obje/dizi içleri dahil). Yeni bir namespace eklenirse `dollar::EXACT`/`PREFIXES` de genişletilir.
- `terminal_when` DEPRECATED (WOR-84): motor okumaz, validator uyarır, yeniden serileştirmede düşer. Terminal `wft: {terminal}` ile verilir.
- `wfd_version: "2.2"` zorunlu; eski format hem upload hem fetch'te reddedilir.
- **`docs/spec/reference-types.rs` DERLENİR ve motorla PARİTESİ test edilir** (2026-08-18,
  `crates/wfe-core/tests/reference_types_parity.rs`): dosya `#[path]` ile modül olarak
  alınır (tip rotunu derleyici yakalar), tip/alan kümeleri `types/wfd_v22.rs` ile
  karşılaştırılır (motor ÜST KÜMEDİR — motorda olup referansta olmayan alan HATA; tersi
  değil, referans `CandidateActor::slug`'ı bilerek fazladan taşır) ve `docs/spec/examples/`
  altındaki her belge bu modelle parse edilir. Bilerek dışarıda kalan tip
  `ENGINE_ONLY` listesinde GEREKÇESİYLE yazılır. Sebep: `docs/` altında olduğu için
  hiçbir derleyici bakmıyordu ve sessizce çürümüştü (2026-08-17 ölçümü: 8 tip + 10'dan
  fazla alan eksik, `c_u` hâlâ `Vec<String>`). Motora alan eklenince ORASI da güncellenir.
- **`docs/spec/schema.json` RUNTIME kapısıdır** (`wfe_core::schema`, `include_str!` ile gömülü): `Wfd::from_value_checked`/`from_json_checked` upload/publish/submit/approve/**fetch**, `/wfd/validate`, `/wfe/simulate` ve senaryo koşumunda şemayı zorlar — serde `minItems`/`pattern` bilmez, elle yazılan JSON o boşluktan giriyordu (`"c_r": []`). Taslak KAYDI kapsam dışı; ham `from_value` testler için açık. Şema değişirse frontend kopyası (`src/schema/wfd.schema.json`) birlikte güncellenir.

## Çalışma kuralları

- Her değişiklikten sonra `cargo test --workspace`; golden fixture (`docs/spec/examples/kredi-basvuru.golden.json`) DEĞİŞTİRİLMEZ — kod fixture'a uyar. (Tek istisna: WOR-70/2026-07-29, spec değişikliği gereği kullanıcı onayıyla. Kural yürürlükte.) Fixture'ların `crates/wfe-core/tests/fixtures/` kopyaları senkron tutulur.
- Context'e TEK yazma yolu `wfes_effects`'tir (WOR-70): aksiyon girdisi ctx'e kendiliğinden yazılmaz, `$action.input.<yol>` ile açıkça yazılır. `context.required` ve alan içi `required` YASAK.
- Zamana bağlı testlerde `#[tokio::test(start_paused = true)]` kullan (retry/timeout gerçek beklemeden koşar).
- Migration'lar psql ile manuel uygulanır (`migrations/org`, `migrations/wf` sırasıyla); sqlx migrate kullanılmıyor.
- Çok elemanlı eski c_a array'i ile karşılaşırsan OTOMATİK dönüştürme — dur ve sor (M10).

## Git remote / push politikası

- İki remote: `origin` = GitHub, `gitlab` = kurumsal GitLab
  (`gitlab.cs.com.tr:agnoflow/src/agnoflow-backend.git`).
- **`main` HER İKİ remote'ta senkron tutulur.** Kullanıcı "push" dediğinde main'i **hem GitHub
  hem GitLab**'e at: `git push origin main && git push gitlab main`. Birine push edilirse
  diğerine de edilir — ayrı düşmesinler.
- **`staging` branch'ine ASLA push/merge etme** — kullanıcı açıkça "deploy" / "deployla"
  demedikçe. `gitlab staging`'e push CI/CD'yi tetikler (build → image.cs.com.tr → Flux →
  `agnoflow-staging` deploy). Deploy istenince: `git push gitlab main:staging`.
- **Commit mesajlarına ASLA `Co-Authored-By: Claude ...` veya benzeri Claude/AI imzası yazılmaz.**

## Deployment (staging — Kubernetes + Flux GitOps)

Deployment manifest'leri AYRI repoda: **`agnoflow-infra`** (GitLab `agnoflow/config/agnoflow-infra`),
Flux ile cluster'a uygulanır. Bu repo yalnız kod + `Dockerfile` + `.gitlab-ci.yml` tutar.

- **Akış:** `staging`'e push → GitLab CI image build → `image.cs.com.tr/agnoflow/agnoflow-backend:staging-<CI_PIPELINE_IID>` push → Flux ImageUpdateAutomation tag'i agnoflow-infra'ya bump'lar → `agnoflow-staging` namespace'ine deploy.
- **CI** (`.gitlab-ci.yml`) yalnız `staging` branch'inde koşar; runner `$EFP_RUNNER_TAG`, Nexus'a zaten login (docker build/push).
- **Image:** multi-stage Rust `Dockerfile`, `wf-server` :3000, non-root uid 1000.
- **Cluster:** node `10.10.10.189`; Flux logimesh ile aynı `flux-system`'i paylaşır → tüm Flux nesnelerimiz `agnoflow-*` prefix'li (çakışma önlendi). Secret'lar SOPS (age) ile şifreli (infra repoda).
- **DB:** cluster içi postgres (`agnoflow-staging` ns). Migration OTOMATİK DEĞİL — şema elle uygulanır (ilk kurulumda yerel DB pg_dump'landı).
- **ADMIN_API_KEY staging'de TANIMSIZ** → `/org` dev gibi açık (frontend admin header göndermiyor).
- **Erişim:** `http://agnoflow.staging.cs.com.tr` (şimdilik HTTP; TLS cert bekleniyor) + `agnoflow.10-10-10-189.nip.io`.

## Ortam değişkenleri

`DATABASE_URL`, `PORT`, `JWT_SECRET` (zorunlu), `STORAGE_BACKEND=local|s3` (+`STORAGE_PATH`),
`CORS_ORIGINS` (virgülle; unset = localhost dev), `ADMIN_API_KEY` (/org koruması; unset = dev uyarısı).
`DB_CONN_SECRET` (base64 32 byte; `db_connection` secret'ları + `$env` secret'ları). **Virgülle
ayrılmış LİSTE olabilir** — şifreleme daima ilkiyle, çözme hepsi denenerek: anahtar rotasyonu
yeni anahtarı başa eklemekle yapılır (GitLab `db_key_base` dizisiyle aynı yaklaşım).
Ek-belge deposu (attachments, WFD JSON storage'ından AYRI): `ATTACHMENT_STORAGE_BACKEND=local|s3`
(+`ATTACHMENT_STORAGE_PATH`, local default `../work-pool-portal/storage`; `ATTACHMENT_STORAGE_S3_BUCKET/REGION`).
`ATTACHMENT_MAX_REQUEST_MB` (varsayılan 200, yalnız `/wfe`+`/portal` alt ağaçlarına
uygulanır) ve `WFE_START_DEDUPE_WINDOW_SECS` (varsayılan 60) — 2026-08-11, aşağıda.

## Attachments (ek-belge) sözleşmesi

- WFD şeması: root `attachments` katalogu (adlandırılmış gruplar → `items[]`) + `nodes.<key>.attachments`
  (grup referansları). Engine core dosya I/O YAPMAZ — yalnız katalog+referansı metadata tutar.
- **Referans iki biçimlidir** (`AttachmentRef`, 2026-08-07): düz `"grup"` = node'un TÜM aksiyonlarına
  kapı (eski biçim, eski dosyalar aynen çalışır); `{group, actions?}` = yalnız sayılan aksiyonlara kapı.
  `actions` **Option**'dır: `[]` hiçbirini kapamaz (opsiyonel yükleme), alan HİÇ verilmezse tümü —
  `#[serde(default)]` bir `Vec` bu iki zıt anlamı aynı gösterirdi. İki biçim de "bu grup burada
  TOPLANIR" der; fark yalnız kapıdır. Validator: kapsamdaki aksiyon o node'dan çıkan bir transition'da
  bulunmalı (`attachment_action_ref`), kapsam içi tekrar yok (`attachment_action_dup`).
- Varlık kontrolü + yükleme server portal edge'inde: `AttachmentStore` (opendal), storage anahtarı
  `attachments/{wfe_id}/{grup}/{item}`. Rotalar hem direkt `/wfe/*` (X-Actor, portal bunu kullanır) hem
  JWT `/portal/wfe/*` ağacında: `GET .../attachments` (durum), tek dosya `GET/DELETE .../:group/:item`
  (indirme + tekil silme, DURUYOR). Tek dosyalık `PUT .../:group/:item` **yalnız JWT ağacında** kaldı
  (2026-08-11: direkt X-Actor karşılığı + tek kullanıcısı `validate_upload` silindi — JWT'nin bu
  workspace dışında tüketicisi olabileceğinden o duruyor). Çok dosyalı yükleme aksiyonsuz akışta
  aşağıda, aksiyonlu akışta Faz 4'te anlatılır.
- **Depo WFD BAŞINA çözülür** (2026-08-07): `$env`teki `ATTACHMENT_STORAGE_BACKEND` /
  `_PATH` / `_S3_BUCKET` / `_S3_REGION` / `_S3_ENDPOINT` / `_S3_ACCESS_KEY_ID` /
  `_S3_SECRET_ACCESS_KEY` anahtarları okunur (`server/src/attachment_store.rs`, Operator
  önbellekli). Tanımlı değilse RUNTIME'da deployment varsayılanına (`ATTACHMENT_STORAGE_*`
  env) düşülür. Secret'lar yalnız bu katmanda çözülür.
- **Belge TOPLAYAN akış depo ayarı olmadan YAYINLANAMAZ** (2026-08-10,
  `routes::wfd::assert_attachment_storage_env`; publish + submit + approve'un HEPSİNDE).
  Kapı `attachment_storage.missing_env` ile 422 döner. `assert_env_keys_defined`den iki
  farkı var: (1) anahtarlar dokümanda GEÇMEZ, `$env.X` taraması onları hiç görmez —
  ayrı kapı olmak zorunda; (2) varlık değil **DEĞER** aranır ve satırı olmayan ortam
  SESSİZ GEÇİLMEZ, çünkü eksik ayar hata vermez: deployment varsayılanına düşer ve belgeler
  müşterinin bucket'ı yerine sunucu diskine yazılır. Zorunlu küme backend'den türer
  (`attachment_store::required_env_keys`: local → `_PATH`; s3 → `_S3_BUCKET`/`_S3_REGION`/
  `_S3_ENDPOINT`/`_S3_ACCESS_KEY_ID`/`_S3_SECRET_ACCESS_KEY`). **`_S3_ENDPOINT` zorunludur:**
  `build_operator` endpoint'i yalnız verildiğinde uygular ve `disable_config_load()`/
  `disable_ec2_metadata()`'yı da o zaman çağırır → boş endpoint sessizce AWS'e konuşur ve
  ambient AWS credential'larını kullanabilir. AWS kullanan akış adresi açıkça yazar.
  "Belge topluyor" = bir node'un referans verdiği ve İÇİNDE `items` olan katalog grubu
  (`attachment_store::collects_attachments`); editör tarafındaki aynası
  `utils/attachmentStorageEnv.wfdCollectsAttachments` — İKİSİ AYNI SORUYU SORAR.
- **Rezervasyon satırı artık YALNIZ crash ağıdır** (2026-08-11): `wf.wfe_reservation`
  tablosu, `reservation.rs` ve saatlik süpürücü DURUYOR, ama HTTP yüzeyi kalmadı —
  `POST /wfe/reserve` ve `DELETE /wfe/reserve/{wfe_id}` KALDIRILDI (tarama: bu
  workspace'te çağıranı yoktu; tek yol multipart/JSON `POST /wfe` oldu). `wfe_id`'yi artık
  DAİMA engine üretir — `POST /wfe` gövdesindeki `wfe_id` alanı da kaldırıldı, rezerve
  edilmiş id almanın dışarıdan yolu yok. Satırın tek işlevi: sunucu istek ortasında ölürse
  (deploy/OOM) yazılmış baytların sahibini süpürücüye bildirmek — istek başında yazılır,
  başarıda silinir, istemciye hiç görünmez. `assert_can_start` (start kuralının `from`
  node'unun c_a'sı `Engine::start`'ın kural seçimiyle AYNI testten geçer) artık doğrudan
  `POST /wfe` (her iki content-type) içinde koşar; eşleşme yoksa `403` ve bayt hiç
  okunmaz. `WfeExecutor::start_reserved`ın `reserved_wfe_id` parametresi koddadır
  (multipart yolu kullanır) ama dışarıdan id vermenin HTTP karşılığı yok.
- **Tek istekte başlatma** (2026-08-11): `POST /wfe` artık `multipart/form-data` da
  kabul eder — `payload` part'ı İLK olmak ZORUNDA (`400 multipart.payload_first`,
  yetki kararı baytlardan önce). Dosya part adı `{grup}/{slot}`; `filename` yalnız
  metadata, storage anahtarına karışmaz. Dosyalar `AttachmentStore::writer` ile STREAM
  yazılır (bellek dosya sayısı/boyutundan bağımsız); her hata yolunda yazılanlar silinir
  + rezervasyon satırı bırakılır — **istemci telafi çağrısı yapmaz**. `application/json`
  gövdeli `POST /wfe` (dosyasız başlatma) AYNEN çalışır — yalnız `wfe_id` alanı YOK
  artık; eski rezerve→yükle→başlat HTTP ucu tamamen kaldırıldı (yukarıdaki madde). `POST
  /wfe/preflight` gövdesiz ön kontrol verir (yetki + slot kuralları + bildirilen
  boyut/tip); YAN ETKİSİZ ve **KAPI DEĞİLDİR**, gerçek denetim `POST /wfe` içinde
  yeniden koşar. Tasarım: `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md`.
- **Çift başlatma koruması sunucudadır** (`start_dedupe.rs`, tablo
  `wf.wfe_start_dedupe`): parmak izi İSTEKTEN türetilir (actor+wfd+version+action+
  kanonik `input`+`attachments` bildirimi), istemci hiçbir header göndermez. Pencere
  `WFE_START_DEDUPE_WINDOW_SECS` içinde tekrar → ilk `wfe_id` + `Idempotent-Replay: true`;
  hâlâ koşuyorsa `409 conflict.start_in_progress`. Kaçış: `X-Allow-Duplicate: true`.
  Parmak izi YALNIZ `payload`tan türer, baytlardan DEĞİL — tekrar istek dosyaları
  aktarmadan yanıtlansın diye. Hata yolunda satır silinir; fiziksel süpürme mevcut
  saatlik süpürücüde (`reservation::sweep`).
- **Gövde limiti düzeltildi** (gerçek bug'dı): `DefaultBodyLimit` layer'ı hiçbir yerde
  yoktu → axum'un 2 MB varsayılanı katalogdaki `max_size_mb` sözünü yalanlıyordu.
  `ATTACHMENT_MAX_REQUEST_MB` (varsayılan 200) YALNIZ `/wfe`+`/portal` alt ağaçlarına
  uygulanır; diğer uçların 2 MB koruması durur. **İçerik tipi artık SNIFF edilir**
  (`sniff_content_type`/`detect_mismatch` → 415 `TypeMismatch`): istemcinin
  `Content-Type` beyanına güvenilmez (`.exe`nin `application/pdf` diye geçmesi
  kapatıldı); zip ailesi (docx/xlsx/pptx) aynı magic byte'ı paylaştığından allow-list
  ile ayrılır. `Sha256Stream` ile akış halinde özet hesaplanır; `payload.attachments[]
  .sha256` bildirilirse doğrulanır (`checksum_mismatch`).
- **Depo çözümünde YAZMA katı, OKUMA toleranslı** (2026-08-11): katalog belgelerinin tüm
  yazma yolları (`PUT .../attachments/...`, multipart `POST /wfe`, `POST/PUT /uploads`,
  `staging::take`) `store_for_wfd_strict`/`store_for_wfe_strict` kullanır — `$env`de depo
  yoksa `422 attachment_storage.missing_env`, deployment varsayılanına DÜŞMEZ. Sessiz
  fallback müşterinin bucket'ı yerine sunucu diskine yazmak, yani tenant'ların belgelerini
  bizim diskimizde yan yana koymak demekti; publish kapısı bunu önden yakalıyor ama tek
  savunma olamaz (kapıdan önce yayınlanmış akışlar, sonradan silinen `$env` satırı, anahtarı
  eksik yeni ortam). İndirme/durum/silme/süpürme yolları fallback'i KORUR: eski davranışla
  deployment deposuna yazılmış dosyalar erişilebilir ve temizlenebilir kalmalı.
- **Dosya metadata'sı `wf.wfe_attachment`te** (2026-08-11 Faz 2, `wfe_attachment.rs`): ad,
  tip, boyut, sha256, yükleyen, `uploaded_at` + `version`. Aynı slota tekrar yükleme ÜZERİNE
  YAZMAZ, yeni sürüm açar; okuma en yüksek sürümü alır. `wfe_id` FK'sı `ON DELETE CASCADE` —
  **satır varsa WFE vardır** (tasarımdaki "aynı transaction" sözü tutulamadı: o transaction
  `wf_wfe` içinde açılıp kapanıyor, server katılamıyor; değişmez FK ile korunuyor). Satırlar
  start BAŞARILI olduktan sonra yazılır, yazılamazsa `warn` + başarı cevabı yine döner.
  **`uploaded` gerçeğinin kaynağı hâlâ DEPO** (`status_for_node` → `exists`); metadata yalnız
  gösterim için eklenir (`attachments::enrich_with_meta`, iki route ağacı da çağırır) —
  kaynak yapılsaydı tablo eklenmeden önce yüklenmiş her belge "yok" görünürdü. Bu yüzden
  `status_for_node` imzası değişmedi, kapı yolunda DB bağımlılığı YOK.
- **Staging yükleme** (2026-08-11 Faz 3, `staging.rs` + `routes/uploads.rs`, tablo
  `wf.upload_staging`): büyük dosyanın baytları başlatma isteğine hiç girmesin diye ÖNCEDEN
  `POST /uploads` ile depoya konur, başlatmaya yalnız `payload.attachments[].upload_id` girer;
  sunucu sahiplik+varlık doğrulayıp **server-side COPY** ile nihai anahtara taşır. Anahtar
  `staging/{upload_id}` — `attachments/`/`notes/` köklerinden AYRI (karışsaydı henüz hiçbir
  WFE'ye ait olmayan dosya "yüklenmiş" sayılırdı). Depo `store_for_wfd` ile çözülür: staging
  nihai anahtarla AYNI bucket'ta olmalı, yoksa taşıma indir-yükle olurdu. s3'te presigned PUT
  (baytlar engine'e uğramaz), local'de sunucuya stream'li `PUT /uploads/{id}`. Handle'lar
  multipart part'larından SONRA işlenir (aynı slot ikisiyle de gelirse son söz handle'ın).
  GC bucket lifecycle DEĞİL, `staging::sweep_expired` (TTL 24s, saatlik süpürücüde) — lifecycle
  ayrı repoda (agnoflow-infra) ve local backend'de karşılığı yok. AV taraması / KMS / retention
  YAPILMADI.
- **`AppError` opsiyonel `items` taşır** (`error.rs`): çok-dosyalı ret slot bazında
  anlatılabilir (`422 attachment.rejected` + `items[]`); `error`/`code` alanları
  GERİYE UYUMLU, `items` yalnız EKLENİR.
- Gate server-side ve AKSİYON BAZLI: `apply_action`/`submit_action` submit edilen aksiyonu KAPAYAN
  grupların `required` dosyaları eksikse `422 code:"attachment.missing"` döner
  (`status_for_node(..., Some(action))` → `gates` alanı → `satisfied`/`missing_required` yalnız
  kapayanları sayar). `GET /wfe/:id/attachments` aksiyon sormaz: her grup `gates: true` + kapsamı
  `actions` ile döner, süzme istemcidedir. Detay: `docs/spec/decisions.md` Madde 8.
- **Akış ortasında çok dosyalı aksiyon** (2026-08-11 Faz 4): `POST /wfe/{id}/actions` artık
  `multipart/form-data` da kabul eder (`payload` part'ı `ApplyBody`, kalan part'lar
  `{grup}/{slot}`); `application/json` eski yol AYNEN çalışır. Sıra: dosyalar staging'e
  (nihai anahtara DOKUNULMADAN) → kapı depo ∪ staging birleşimine bakar (`pending` yalnız
  kapıyı gevşetir, `uploaded` deponun gerçeği kalır) → aksiyon uygulanır → başarıda staging
  server-side copy ile nihai anahtara taşınır + `wf.wfe_attachment` satırı, hatada staging
  silinir ve nihai anahtar hiç dokunulmaz. Dedupe çapası `expected_rev`tir ("o anki rev"
  olsaydı ilk apply'dan sonra parmak izi ilerler, aksiyon ikinci kez uygulanırdı);
  gönderilmezse dedupe hiç koşmaz. Kabul edilen tek boşluk: commit SONRASI taşıma hatası
  aksiyonu geri almaz, cevaba `attachment_error` eklenir (metadata yazılmaz, sonraki kapı
  durdurur). Detay: `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md`
  Faz 4 / K11-K13.
- **Aksiyonsuz çok dosyalı yükleme** (2026-08-11): `PUT /wfe/{id}/attachments` —
  multipart, `{grup}/{slot}` alanları, `payload` part'ı YOK (aksiyon/girdi taşımaz, salt
  dosya). **Atomik**: staging'e yaz → hepsi doğrulanınca hepsi promote edilir; bir dosya
  reddedilirse hiçbiri yazılmaz. `gates_action` süzmesi YOK — aksiyonsuz yüklemede "bu
  grup burada toplanır" yeterli, kapı ayrı bir sorudur (kapı yalnız `apply_action`/
  `submit_action`'da sorulur). JWT simetriği `PUT /portal/wfe/{wfe_id}/attachments`.
  Ortak mantık `upload_multi_shared` — Faz 4'ün staging altyapısını paylaşır.
- **Portal artık HER yerde toplu multipart kullanır**: başlatma (+preflight), aksiyon+belge,
  aksiyonsuz belge (yukarıdaki madde) — tekil `validate_upload`/`uploadAttachment` yolları
  tüketicisiz kalıp silindi. 180 MB üstü istekte en büyük dosyalar önce `/uploads` ile
  staging'e gider (`upload_id` başlatma isteğine girer, istek yine TEK); portal sabiti
  `MAX_INLINE_REQUEST_BYTES` (`workflows/api.ts`) `ATTACHMENT_MAX_REQUEST_MB` (200) ile
  eşleşmeli tutulur — aradaki pay payload+multipart boundary içindir.

## Görünürlük: projeksiyon + tek SQL predicate (2026-08-13)

Karar kaydı: `docs/spec/decisions.md` "Görünürlük: kural belgede, cevap projeksiyonda".

- **Kural**: `görünür = view_c_a @> viewer OR end_view_c_a @> viewer OR (status='active'
  AND (current_c_a @> viewer OR claimed_by @> viewer OR aktif kol c_a/claim))`. İş bitince
  `current_c_a` boşalır → bitmiş işi YALNIZ kalıcı grant'lar gösterir. "WFAH'ta eylemi
  olmak" (eski `can_view` kriteri (b)) yetki ÜRETMEZ.
- **Terminal listable (2026-08-17)**: `terminals[].listable[]` — "WFE BU terminal'de
  bittiyse görsün". Kök `listable` ile node `listable`ın ÜÇÜNCÜ ekseni: kökten farkı
  SONUCA BAĞLI olması (onaylandı/reddedildi ayrı terminal → ayrı görünürlük, `when`
  guard'ı gerekmez), node'dan farkı KALICI olması (terminal'den çıkış yok). Projeksiyon
  `wf.wfe.end_view_c_a`, `status='active'` kolunun DIŞINDA; varılan terminal
  `wf.wfe.end_terminal`de saklanır (yoksa `reproject` kolonu yeniden üretemez). YALNIZ
  başarılı `Terminal`de yazılır — `Failed`/`Terminated` kapsam dışı. Karar:
  `docs/spec/decisions.md` "Terminal-level `listable`".
- **Kolondan ÖNCE bitmiş satırlar KANITLARDAN kurtarılır** (`wfe_core::v22::end_terminal`,
  saf + birim testli): `end_response`in anahtar kümesi/sabit değerleri + WFAH'ın son gerçek
  aksiyonundan çıkan `wft`in terminal kümesi + değişmez belge. Her kanıt yalnız DARALTIR;
  tek aday kalmazsa kolon NULL bırakılır — **kurtarma asla tahmin etmez** (yanlış
  `end_terminal` = görmemesi gereken kişiye bitmiş işi göstermek). Sürücü
  `visibility_backfill`in ÖN GEÇİŞİ, ayrı komut değil: `reproject` kolon dolmadan
  `end_view_c_a` üretemiyor, sıra zorunlu.
- **Tek yer**: `wf_wfe::visibility::sql`. Liste ucu, detay kapısı (`VisibilityPort` →
  `WfeExecutor::query`) ve portal havuzu AYNI parçayı koşar. Çekirdeğin `can_view`'i
  referans okumadır (sim/testler); eşitliği `visibility_report` ölçer.
  **Havuz 2026-08-14'te bağlandı** (`routes::portal::pool`, İKİ sorgu da): kendi
  `WHERE`'i vardı ve node listable kolonlarını tanımıyordu. Kol başına CANLI adaylık
  çözümü (`authorize` + kök `listable` fold'u) kalktı — karşılığı `wfe_branch.c_a`
  projeksiyonu, parçanın kol EXISTS'i onu sorar. Havuzun kendi süzgeçleri
  (`status='active'`, `deadline`, `current_node IS NOT NULL`) görünürlük DEĞİL,
  "bu satır bir havuz görevi mi" sorusudur; sonuncusu paralel WFE'nin hem node'suz
  WFE satırı hem kol satırlarıyla iki kez listelenmesini önler.
- **Havuzda görünmek claim edebilmek DEĞİLDİR**: claim kapısı node `c_a`'sına bakar
  (`WfeExecutor::can_claim`/`claim`; projeksiyon kolonu okumaz) ve görünürlük filtresi
  genişledikçe gevşemez. **Havuz cevabı artık bu ayrımı TAŞIR** (2026-08-14):
  `PoolTask.can_claim` (alan EKLENDİ, hiçbir mevcut alan değişmedi). Sebep: görünürlük
  tek predicate'e bağlanınca kapsam genişledi (kök `listable`, node `listable`,
  `wf_admin` de satır üretiyor) ve kullanıcı claim edemeyeceği satırı diğerlerinden
  ayırt edemiyor, düğmeye basıp `403` yiyordu. Alan kararı ÜRETMEZ, ÖDÜNÇ ALIR:
  `WfeExecutor::can_claim_many` → `can_claim_loaded`, yani `can_claim`/`claim`
  uçlarının GÖVDESİ (`Engine::can_claim` → matcher → node `c_a`). Havuzda ikinci bir
  claim kuralı YOKTUR; ayrışırlarsa `can_claim_many_matches_can_claim_row_by_row`
  patlar. Kol satırında karar KOLUN node'una göre verilir, WFE seviyesine göre DEĞİL.
  Claim başkasındaysa `false`, sahibi çağıran ise `true` (idempotent re-claim);
  durumu/WFD'si okunamayan satırda `false` (fail-closed). **N+1 yok**: `rev`/not
  sayaçlarıyla aynı desen — tek `WfeStore::load_many` + sürüm başına bir WFD
  (adapter cache'i), karar üretimi saf CPU. Tekil uç
  `GET /portal/pool/{wfe_id}/can-claim` DURUYOR (gerekçe `reason` oradan okunur),
  ama istemcinin satır başına çağırmasına gerek YOK.
- **ORGTRVLANG çapası WFE'nin birimidir** (`wf.wfe.origin_orgu_id`, start'ta donar), soran
  kişinin DEĞİL — `matcher::authorize_anchored`. Görünürlük, aksiyon, claim ve reassign
  kapılarının HEPSİ bu çapayı kullanır. `NULL` = backfill bekliyor → eski davranış.
- `listable`/`wf_admin` guard'ında **`$actor` YASAK** (`grant_when_actor_ref`).
- Alan bazlı gizlilik (`context.*.x-visibility`) AYNI çapayı kullanır (`filter_dynctx`'in
  `anchor` parametresi); `c_r` kanalı birim kısıtı taşımadığı için etkilenmez.
- **Projeksiyonu yazan tek yol**: `WfeExecutor::fill_view_grants` (+ start'ta `create`).
  Yeni bir commit yolu eklendiğinde ORAYA bağlanır; adapter kolonları outcome match'inin
  DIŞINDA, aynı transaction'da yazar.
- **Projeksiyon commit SONRASI durumla çözülür**: ctx `commit.new_dynctx`, WFAH ise
  `wfes.wfah.extended(&commit.wfah_entries)` — `wfes.wfah` bu geçişin kayıtlarını henüz
  içermez. Eskiden ham `wfes.wfah` kullanılıyordu ve `{from:{wfah:"X"}}` çapalı bir
  `listable` kuralı BİR COMMIT GEÇ yazılıyordu: X uygulandığı anda liste/havuz (saf
  projeksiyon) grant'ı görmüyor, referans okuma `can_view` (canlı defter) görüyordu.
  Regresyon testi: `crates/wfe/tests/view_grants_wfah_anchor.rs`.
- Şema/kural değişince **`visibility_backfill --apply`** koşulur, sonra
  **`visibility_report`** ile kontrat doğrulanır (hedef: "KONTRAT SAĞLAM").
- Sayfalama: `GET /wfe?viewable=true&limit&offset` + `X-Total-Count` (CORS'ta expose).
- **`wf.wfe.wfd_id` → `wf.wfd_meta` FK'lidir** (2026-08-13): tarifi silinmiş WFE
  ("öksüz") ARTIK OLUŞAMAZ. Teşhis/temizlik: `orphan_wfe_cleanup` (kuru koşum
  varsayılan; "WFD satırı yok" ile "pasif/yayında değil" durumlarını AYRI raporlar —
  ikincisi silinmez, yayın durumu düzeltilir).
- **Org AĞACI değişince** (birim ekle/güncelle/pasifleştir) uç `wf.visibility_reprojection`
  kuyruğuna yazar (tenant başına tek satır), saatlik süpürücü `visibility_worker::run_once`
  ile 500'lük partiler hâlinde yeniden projelendirir; ilerleme `grants_built_at`te kalıcıdır.
  Rol ATAMASI kuyruğa girmez — satırlar (birim, rol) tuttuğu için yeni atama anında etkilidir.
  Yeniden üretimin TEK kod yolu `wf_wfe::reproject` (backfill komutu da onu çağırır).

## Tip denetimi: engine bilir kişi (2026-08-19)

Karar kayıtları: `docs/spec/decisions.md` → "Adlandırılmış tip: `format` → `$defs`" +
"Runtime tip denetimi — engine bilir kişi". İlke: **bildirilen bir tip varsa ve değer o
tipte gelmiyorsa reddi ENGINE verir** — istemci (editör, portal, üçüncü parti UI) kendi
kuralını icat etmez, yalnız aynı cevabı önden verir.

- **Adlandırılmış tip = `format`** (KIRICI): `{"format": "Tarih"}` = `#/$defs/Tarih`.
  `format` bu belgede **standart JSON Schema formatı DEĞİL** — kuralı kütüphanenin
  tablosunda değil BELGEDE (`$defs` tanımında) durur, motor onu okur ve zorlar. Yeni
  validator kuralları: `context_format_unknown` (tanımsız ad) · `context_format_with_type`
  (`format` yanında tip kuralı olamaz — tip tanımın içinde) · `context_format_cycle` ·
  `context_defs_name` · `context_ref_removed`.
- **`$ref` TAMAMEN KALDIRILDI — okuyucusu da yok** (kullanıcı kararı, 2026-08-19):
  şemadan çıkarıldı (`contextSchemaNode`), `deref_defs`/`ctx_types` yalnız `format`
  çözer, validator `context_ref_removed` ile reddeder. Gerekçe: ürün production'da
  DEĞİL; mevcut tüm WFD/WFE'ler test verisi ve production öncesi sıfırlanacak. **"Wire
  formatı değişince okuyucu kalır" kuralı production'dan SONRA geçerlidir** — şu an
  geriye uyum kodu saf borçtur.
- **Wire formatı DRY**: yayınlanan belgede `format` + `$defs` birlikte durur (editör artık
  inline ETMEZ). Çözücüler: motor `v22::ctx_types`, editör `utils/contextDefs`, portal
  `lib/contextTypes` — üçü AYNI kuralı uygular.
- **Çalışma anı denetimi `wfe_core::v22::ctx_types`** (SAF): yol → alt şema (adlandırılmış
  tip çözülmüş) + değer doğrulama. Zorlananlar: `type`/`enum`/`const`/sayı sınırları/
  `minLength`/`maxLength`/`pattern`/dizi kuralları/iç içe `properties`. **`null` HER tipte
  geçerlidir** (WOR-70b gönderilmeyen `optional` ctx'e `null` yazar — aksi halde golden
  fixture ilk koşuda patlardı). Şemanın tanımlamadığı yol için tip ihlali üretilmez.
- `resolve_schema_path` ve `schema_type_at` artık adlandırılmış tipi ÇÖZER → `$defs`
  arkasındaki alan `effect_type_mismatch`/`input_path` denetimlerinin İÇİNE girdi
  (2026-08-19 öncesi `Opaque` sayılıp sessizce atlanıyordu).
- **Ölçüm aracı `ctx_type_report`** (salt okuma): sahadaki WFE'lerin dynctx'ini kendi WFD
  şemasına karşı tarar + şemada olmayan ctx alanlarını + `$ref` kullanan sürümleri sayar.
  Koşum: `DATABASE_URL=... cargo run -p wf-server --bin ctx_type_report` (WFD JSON deposu
  için `STORAGE_*` da gerekir). **İlk ölçüm: 25 WFE / 0 ihlal / 0 `$ref`.**
- **Kapı A TAMAMLANDI (F2):** `validate_action_input` artık context şemasını da alıyor
  ve bildirilen yolların DEĞERLERİNİ denetliyor → `EngineError::InputTypeMismatch(Vec<
  Violation>)` → `422` + `code: "input.type_mismatch"` + **`items[]`** (alan bazında
  `path`/`expected`/`got`/`message`). `start` ve `apply` (tek-kol + paralel) aynı kapıdan
  geçer; `sim`/senaryo koşucusu aynı fonksiyonu çağırdığı için otomatik kapsanır.
  Sıra: bildirim denetimi ÖNCE, tip SONRA (tanımsız yola gönderilen bozuk değerde
  kullanıcı asıl sorunu görsün). Portal `items[]`i alan bazında forma basar
  (`inputTypeErrorFields` / `applyInputTypeErrorFields`).
- **Senaryoda `expectStartReject`** (yeni): BAŞLATMANIN reddi de test edilebilir
  (yetkisiz başlatan, yanlış tipte/eksik başlangıç girdisi, olmayan start aksiyonu).
  Onsuz start hatası her koşulda senaryoyu kaldırıyordu, yani o kurallar
  test EDİLEMİYORDU.
- **Kapı B TAMAMLANDI (F3):** `pipeline::guard_written_ctx` — commit kurulmadan ÖNCE
  bu geçişin YAZDIĞI kök alanlar denetlenir (`ctx_types::validate_written`, `before`/
  `after` karşılaştırması). Altı çağrı noktası: start · apply (tek-kol) · apply (paralel
  kol) · escalation · claim_timeout · WFC dönüşü. `fire_deadline_timeout` ctx'e YAZMAZ
  (kopyalar), kapı gerekmez. Böylece autoexec sonucu · `$call.result.*` · `$env` ·
  sistem yazımları tek noktadan geçer → `EngineError::CtxTypeMismatch` → `422` +
  `code: "ctx.type_mismatch"` + `items[]`. **Yalnız DEĞİŞEN alanlar** denetlenir:
  enforcement öncesi bozulmuş veri geçişi durdurmaz (o kapı C'nin işi). Ölçüm sıfır
  çıktığı için `warn` fazı atlandı, doğrudan REDDEDİYOR.
- **Kapı C TAMAMLANDI (F3):** `executor::guard_stored_ctx` — bozuk `dynctx` taşıyan
  WFE'de **eylem** reddedilir (apply · claim · escalation fire; `skip_escalation` geçiş
  uygulamadığı için kapsam dışı), **görüntüleme SERBEST** kalır ve ihlaller
  `WfeView.ctx_violations` ile bildirilir (portal kırmızı şerit gösterir). Kayıt görünmez
  olsaydı kullanıcı neyin bozuk olduğunu göremez, düzeltemezdi.
- **F4 TAMAMLANDI:** `ApiError` artık `items[]` taşıyor (`engineApi.typeViolationsOf`),
  editörün `PathFields`i `serverErrors` prop'uyla motorun reddini ALANIN yanında gösteriyor
  (SimulationTab start + apply). Portal iki yolda da aynı şeyi yapıyor
  (`inputTypeErrorFields` / `applyInputTypeErrorFields`) ve bozuk bağlam için kırmızı şerit
  çiziyor. Editörde WFE detay yüzeyi YOK (o portalın işi) — `ctx_violations` gösterimi
  oraya ait.
- **F5 TAMAMLANDI:** `docs/spec/migration-notes.md` **M19** (kırıcı: `$ref` kaldırıldı,
  `format` semantiği, üç kapı, düzeltme reçetesi), `docs/spec/runtime-semantics.md` **§7.5b**
  + pipeline adım listesine kapı B/C, `decisions.md` iki madde.
- Yakalanabilir hata sınıfı (`trigger[].catch.error_equals` ile `WFD.CtxTypeMismatch`)
  BİLİNÇLİ olarak YAPILMADI: tip ihlali akışın tasarım hatasıdır, `catch` ile gizlenmesi
  bozuk verinin sessizce ilerlemesine yol açardı — gerekirse ayrı bir karar maddesiyle
  açılır.

## Simülasyonda belge + not (senaryo testleri)

2026-08-19. Amaç: bir WFD'nin **kaydedilip her seferinde koşturulabilen** test seti
yalnız mutlu yolu değil, portal kullanıcısının gerçekten çarptığı kuralları da
kanıtlayabilsin — "belge yüklenmeden onaylanamaz", "yanlış tip reddedilir",
"bu aktör bu adımı alamaz".

- **Kural tek kaynakta**: `wfe_core::v22::attachments` (gate: `gate_slots` +
  `missing_required`; format/boyut: `check_upload`/`mime_matches`/
  `all_accept_patterns`) ve `wf_wfe::note_rules` (not limitleri + `Audience`).
  `wf_server::attachments`/`notes` bunları `pub use` ile yeniden ihraç eder —
  çağrı yerleri değişmedi, DAVRANIŞ değişmedi. İki kopya olsaydı simülasyonda
  geçen bir senaryo portalda 422 alabilirdi (`check_expectations`ın motora
  taşınmasıyla aynı gerekçe). Not kuralları `wfe-core`'a KONMADI: motor not
  katmanından habersiz kalır (K1) — `wf_wfe` adapter crate'i, `sim`in yaşadığı yer.
- **`SimState` iki alan kazandı** (ikisi de `#[serde(default)]`, eski blob'lar
  parse olur): `attachments: Vec<SimAttachment>` (grup/slot + ad/tip/boyut) ve
  `notes: Vec<SimNote>`. **BAYT TAŞINMAZ** — simülasyonda depo yok; tutulan şey
  metadata, denenen şey DOSYAYA BAĞLI KURALLAR.
- **Kapı `sim::step::apply` içinde, motordan ÖNCE**: node = verilen kol ??
  `current_node`, aksiyon bazlı (gerçek akıştaki `apply_action` ile aynı soru).
  Eksikse `ApplyError::MissingAttachments`; route bunu `422 attachment.missing`
  yapar (gerçek akışla AYNI kod/statü). `EngineError`'a katlanmadı: motor dosyaya
  değmez, kapı portal/edge kuralıdır.
- **Uçlar**: `POST /wfe/simulate/attach` (`422 attachment.rejected`: bilinmeyen
  slot / aktif adımda toplanmıyor / format-boyut), `.../detach`, `.../note`
  (`422 note.rejected`). `detach`/`note` WFD parse ETMEZ (yarım taslakta da
  çalışmalı); `attach` katalog için parse eder.
- **Senaryo adımları** (`wf_wfe::scenario::ScenarioStep`, `untagged`, her varyantın
  benzersiz zorunlu anahtarı var): `action` · `call_return` · **`attach`** ·
  **`note`**. Aksiyon/attach/note adımlarının hepsinde **`expectReject`** (camelCase)
  bayrağı var: `true` ise adım REDDEDİLMELİ — reddedilirse senaryo geçer ve sebep
  `ScenarioResult.rejected_as_expected`'e yazılır, beklenmedik şekilde uygulanırsa
  senaryo KALIR ("kural devrede değil"). Negatif test olmadan senaryo seti yalnız
  mutlu yolu kanıtlıyordu.
- **`expect.active`** (`Option<bool>`): adımlardan sonra akış hâlâ aktif mi olmalı,
  bitmiş mi. Negatif senaryonun asıl kanıtı budur (`expect.terminal` bunu söyleyemez;
  ayrıca `infer_terminal_id` yalnız `terminals[].wfes_effects.set` ayırt ediciyse id
  çözer — `wfe_end_response` id çözmez, o senaryolar `active` ile yazılır).
- `ScenarioResult` üç alan kazandı: `attachments[]` (koşu sonunda yüklü slotlar),
  `notes` (sayı), `rejected_as_expected[]`.
- **Not akışın gidişatını hâlâ DEĞİŞTİRMEZ** (K1): `$ctx`/`$wfah`'a yazılmaz,
  `$notes` yok. Simülasyondaki not yalnız limit testidir. Draft→aksiyonla-yayın
  zinciri DB'ye özgüdür, simülasyonda not eklemek TEK adımdır.
- **Yetki kapısı ARTIK `sim::step::apply` İÇİNDE** (2026-08-19): `sim::step::eligible`
  (gerçek `can_claim` kuralının aynısı) `routes/simulate.rs`ten BURAYA taşındı. Sebep:
  yalnız route'ta yaşadığı için **senaryo koşucusu hiç sormuyordu** — yetkisiz aktörle
  yazılmış senaryo yeşil geçiyor, aynı adım portalda 403 alıyordu. Sıra gerçek akışla
  aynı: yetki (`ApplyError::NotEligible` → 403) → belge kapısı (422) → motor.
- **Kasıtlı HATA senaryoları birinci sınıf vatandaştır.** Motorun reddettikleri
  senaryoyla test edilebilir: eksik/null zorunlu girdi · bildirilmemiş girdi yolu ·
  olmayan aksiyon · yetkisiz aktör · geçersiz kol · geçersiz/eksik GLB hedefi · belge
  kapısı · katalog dışı slot / kabul edilmeyen tip / boyut aşımı · not limitleri.
  **REDDEDİLMEYEN tek şey TİP**: `validate_action_input` varlık ve bildirim denetler,
  tip denetlemez — yanlış tip ctx'e AYNEN yazılır ve etkisi karar anında görülür
  (sayısal `when` çalışırken patlar). Bu sınır editörde de açıkça yazılıdır; iki test
  onu belgeler (`wrong_type_input_passes_the_input_gate`,
  `wrong_type_breaks_a_numeric_condition_at_decision_time`).
- Testler: `crates/wfe-core/src/v22/attachments.rs` (kapı birim testleri),
  `crates/wfe/src/note_rules.rs` (limitler), `crates/wfe/tests/scenario.rs`
  (uçtan uca: kapı bloklar → `attach` açar → kapsamlı grup yalnız kendi aksiyonunu
  kapar → yanlış tip/boyut reddi → not kayda geçer ama ctx'e sızmaz).

## WFE not defteri (ad-hoc not + belge)

Tasarım: `docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md` (K1–K9).
Uygulama: `crates/server/src/notes.rs` (`attachments`'ın kardeşi) + `routes/notes.rs` /
`routes/portal/notes.rs` (iki ince kabuk, aynı ortak mantık).

- **Motor bu katmandan HABERSİZDİR**: not ne `$ctx`'e ne `$wfah`'a girer, `$notes` diye bir
  ZEN namespace'i YOKTUR (K1/K2). Neden: yayınlanmış akışlar `$wfah`'ı **sayarak** karar
  veriyor (`count($wfah, #.action == "x") >= n`); araya insan-üretimi bir not satırı koymak
  bu sayımı ve `$prev`/`$first` kısayollarını kaydırır. Akışın kararını etkileyen her şey
  hâlâ WFD `actions[].input` → `wfes_effects` → `$ctx` yolundan gider.
- **4 tablo**: `wf.wfe_note` / `wf.wfe_note_file` / `wf.wfe_note_read` (Faz 1/2/3) +
  `wf.wfah`'a eklenen `from_node`/`to_node` (Faz 0, K7).
- `from_node`/`to_node` YALNIZ KAYIT VE EKRAN içindir; `$wfah` izdüşümü (`{seq, action,
  actor, input, at}`) DEĞİŞMEDİ. `WfahEntry` core tipine alan eklenmedi (golden fixture'ı
  bozardı) — bilgi `WfeAdapter` seviyesinde türetilir (`commit`'in zaten bildiği
  `to_node`/`current_node`'dan), `GET /wfe/:id` cevabına `WfeView.path` (`Vec<PathStep>`)
  ile sunulur; seam `crates/wfe/src/executor.rs`'deki `WfahPathSource` trait'i + boş
  dönen `NoWfahPath` (store'suz testleri etkilemesin diye).
- **draft → (dosya) → AKSİYONLA publish** deseni (K5): `POST /wfe/:id/notes` draft yaratır
  (yalnız yazarı görür) → istenirse `PUT .../notes/:note_id/files` ile dosya → yayın
  **yalnız aksiyonla** olur (`POST /wfe/:id/actions` gövdesinde `note_id`,
  `ApplyBody.note_id`). Çapa apply'ın ÜRETTİĞİ `wfah_seq`'dir, `node` geçişin
  `from_node`'udur (notun yazıldığı adım). Apply BAŞARILI ama not yayınlanamazsa cevaba
  `note_error` eklenir, **aksiyon geri alınmaz** — not draft kalır, kullanıcı tekrar
  yayınlar (`routes/wfe.rs::apply_action`).
- **Not/dosya EKLEMEK claim ister; SERBEST yayın YOK** (2026-08-11 kuralı): kural sahibinin
  ifadesiyle "not yazılır, dosya eklenir, aksiyon alındığında bunlar yayınlanır; claim
  etmeden, aksiyon almadan not ve dosya eklenemez".
  - Kapı `notes::assert_actor_holds_claim` — create note / draft güncelleme / dosya yükleme
    uçlarında (İKİ route ağacında da), 409 `note.requires_claim`. `Engine::apply` §7.1'in
    sorduğu soruyu sorar (`NotClaimed`/`NotOwner`): not ekleyebilen, aksiyonu da alabilendir.
    Paralel modda WFE-seviyesi `claimed_by` ANLAMSIZDIR — aktif kollardan biri o aktörde
    olmalı. Saf çekirdeği `holds_claim` (birim testli).
  - Okuma / okundu işaretleme / kendi taslağını silme / published notu gizleme claim
    İSTEMEZ: claim düştükten sonra da kendi taslağını temizleyebilmeli.
  - `POST .../notes/:note_id/publish` ucu DURUYOR ama artık serbest yayın değil, yalnız
    `note_error` telafisidir (`notes::republish_after_apply`): WFE'nin EN SON wfah kaydı
    çağıran aktörün olmalı (409 `note.requires_action`), yayın o kayda çapalanır. Böylece
    YAYINLANMIŞ HER NOT bir aksiyona bağlıdır (`wfah_seq` NULL olmaz); `notes::publish`
    modül-içine alındı (`pub` değil) ki çapasız yayın yeni bir çağıran kazanmasın.
  - Portal karşılığı: "Serbest not" kartı ve timeline'daki "Yayınla" düğmesi KALDIRILDI; not
    kutusu yalnız aksiyon composer'ında ve claim yoksa kilitli (`NoteComposer.locked`).
- **Değişmezlik (K3)**: yayınlanmış not `body` üzerinde UPDATE edilmez. Silme yerine
  gizleme: `hidden_at`/`hidden_by` dolar, gövde DB'de kalır, API `{hidden:true}` döner.
  Gizleme YALNIZ yazarı yapabilir (WFE'yi görebilen herkes değil — aksi halde karar delili
  hedefi tarafından ekrandan kaldırılabilirdi). Gizli notta gövde VE dosyalar API'den
  SIZMAZ (dosyalar notun içeriğinin parçasıdır).
- **Gizleme GERİ ALINABİLİR** (2026-08-12): `POST .../notes/{note_id}/unhide`
  (`notes::unhide`, iki route ağacında da), kapı `hide` ile AYNI — yalnız yazarı; zaten
  görünür notta 409 `note.not_hidden`, draft'ta da aynı kod (draft gizlenmez, silinir).
  Claim İSTEMEZ (`delete_note` ile aynı gerekçe). K3 delinmez: değişmez olan GÖVDEDİR,
  görünürlüğün tek yönlü olması değil — gövde hâlâ hiç UPDATE edilmiyor, yalnız bayrak
  çevriliyor ve `hidden_by` her seferinde kimin çevirdiğini yazıyor. Tek yönlü gizleme
  yanlışlıkla basılan bir düğmeyi kalıcı veri kaybına çeviriyordu.
- **Kapsam/IDOR**: `find_note` DAİMA `wfe_id`+`note_id` ile arar (yol parametresi +
  mutasyon hedefi bağlanmazsa bir WFE'yi görebilen aktör başka WFE'nin notunu
  düzenleyebilirdi); dosyalar `(wfe_id, note_id, file_id)` üçlüsüyle. Kapsam dışı = `404`
  (varlığı da sızmaz).
- **Depo**: anahtar `notes/{wfe_id}/{file_id}`, katalog `attachments/{wfe_id}/{grup}/{item}`
  prefiksinden AYRI (ad-hoc dosyanın katalog karşılığı yok, aynı ağaca karışsa
  `status_for_node`/gate mantığını yanıltırdı). `AttachmentStore::remove_all` artık İKİ
  prefiksi de süpürür. Not dosyası DAİMA `attachment_store::store_for_wfe_strict` ile
  çözülür — deployment varsayılanına DÜŞMEZ, `$env`'de depo eksikse `422
  code:"attachment_storage.missing_env"`. Katalog tarafındaki publish kapısı (`assert_
  attachment_storage_env`) buraya UYGULANMAZ: belge iliştirmeyen yüzlerce akışı
  yayınlanamaz hale getirmemek için kapı publish'te değil, yalnız not-dosyası RUNTIME
  rotasındadır (K4).
- Limitler + sanitizasyon + indirme: dosya başı boyut, not başı dosya sayısı, WFE başı
  toplam kota, çalıştırılabilir MIME blocklist, `sanitize_filename` (yol ayracı/`..`/kontrol
  karakteri temizliği), indirmede `Content-Disposition: attachment` + `X-Content-Type-
  Options: nosniff` (`crate::branding`'in SVG servis deseninin aynısı).
- **Dosya adı çözümü SUNUCUDADIR** (`notes::decode_filename`, `sanitize_filename`'den ÖNCE
  koşar): HTTP başlığı ISO-8859-1 dışına çıkamadığı için istemci `X-Filename`'i yüzde-kodlu
  yollar. Çözümü istemciye bırakmak sözleşmeyi istemci geleneğine indirirdi — kodlayanın
  yüklediği adı kodlamayan bozuk görür, DB'de gerçek ad yerine `%C3%B6` yığını kalırdı.
  DB'de daima GERÇEK ad durur; `filename` doğrudan gösterilir, istemcide decode YOKTUR.
  Kodlanmamış ad bozulmadan geçer, geçersiz UTF-8'de ham metne düşülür.
- **`audience`** (`{"kind":"all"}` | `{"kind":"users","ids":[...]}`, K9) hem `list_visible`
  hem `count_by_wfe`/`unread_count_by_wfe` hem çocuk-WFE not sorgusunda AYNI SQL parçasıyla
  (`audience_sql`) süzer; yazar her koşulda kendi notunu görür. Okundu takibi
  `wf.wfe_note_read` — kendi yazdığın not daima okunmuş sayılır. Liste uçlarında (`GET
  /wfe`, portal pool) satır başına `note_count` + `unread_note_count`, N+1 yok
  (`repo::wfah::max_seq_by_wfe` deseniyle TEK sorgu).
- **WFC görünürlüğü (K8)**: `callRef.notes_visible_to_caller` (varsayılan `false`) —
  açıksa çocuğun **published** notları `from_call` (çağrı key'i) etiketiyle çağıranın
  `GET .../notes` listesine girer; draft'lar hiçbir koşulda sızmaz. İKİ SINIR: (1) yalnız
  TEK seviye derinlik (çocuğun kendi çağrıları izlenmez, "torun" notu kapsam dışı), (2)
  çocuk WFE için AYRI `executor.query` koşulmaz — bayrağın kendisi yetkidir (tasarımcı
  bilinçli olarak açtığı bir kapı, çocuğu göremeyen ama ebeveyni gören aktöre de görünür).
- **Süpürücü**: 24 saatlik yetim draft + dosyaları mevcut saatlik süpürücüye
  (`server/src/reservation.rs` → `notes::sweep_expired_drafts`) eklendi; bir WFE'nin deposu
  artık çözülemiyorsa (örn. `$env` sonradan eksildi) o WFE'nin dosyaları warn ile atlanır,
  süpürücü durmaz.
- Detay/reddedilen alternatifler için spec dosyasına bak; kod ile spec çeliştiğinde KOD
  esastır.

## API sözleşmesi v2 — kimlik ile gösterim ayrıdır (2026-08-12, KIRICI)

Amaç: **partner kendi frontend'ini yazarken hiçbir string kodlaması çözmek zorunda
kalmasın.** Geriye uyumluluk YOK; eski biçimi okuyan kod ve migration yolu bilerek
yazılmadı. Sözleşme: `docs/spec/schema.json` + `wfe_core::v22::display`.

- **`Ref { id, label }` her yerde.** `id` motorun kimliğidir — istemci GERİ GÖNDERİR,
  ayrıştırmaz, ekrana basmaz; `label` ekrana basılan tek şeydir ve **asla null/eksik
  dönmez** (belgede yoksa `display::humanize_key` üretir, istemci fallback yazmaz).
  Ref dönen yüzeyler: `PossibleAction` · `WfeView.current_node`/`branches[]`/`path[]`/
  `join_target` · `WfeApplyResult.current_node` · `WfahView`.
- **GLB: `__gt__` anahtar kodlaması KALKTI.** Hedef artık aksiyon ANAHTARINA
  gömülmüyor; tek aksiyon + tek transition, menü `wft: { targets: [{node}, ...] }`
  içinde (`Wft::Targets`, schema `wftGlobalTargets`, `minItems: 1`). Seçim
  `POST /wfe/{id}/actions` gövdesindeki **`target`** ile gelir.
  - `Engine::apply` `target: Option<&str>` alır. Zorunlu olduğu yerde yoksa
    `TargetRequired` (400 `action.target_required`), menüde olmayan hedef
    `TargetInvalid` (400 `action.target_invalid`), GLB olmayan aksiyonda gönderilmişse
    `TargetUnexpected` (400 `action.target_unexpected`). **Sessizce yok saymak yasak** —
    istemci hedef seçtiğini sanıp motor başka yere götürürdü.
  - **`target` bir action input DEĞİLDİR**: `$ctx`'e yazılmaz, `wfes_effects`
    gerektirmez, `$wfah` izdüşümüne girmez. Eski iki şeklin (anahtar ailesi ve
    `$action.input.hedef` fan-out'u) sebebi buydu; ikisi de kalktı.
  - Validator: `global_action_no_targets` · `_target_unknown` · `_target_dup` ·
    `_target_self` · `global_action_placement` (yalnız `transitions[].wft` içinde).
- **Paralel kol seçimi `ApplyBody.node` → `branch`.** Değer kolun node anahtarıdır ama
  istemci için OPAKTIR; sentetik id tablosu AÇILMADI (istemci onu zaten ayrıştırmıyor,
  yeni bir DB kolonu sıfır fayda için karmaşa olurdu).
- **Terminal: `id` makine anahtarı (`^[a-zA-Z0-9_]+$`), `label` kullanıcı metni.**
  Eskiden id'nin kendisi label'dı; bu yüzden label'lara case-insensitive benzersizlik
  kısıtı biniyordu. O kısıt KALKTI (`terminal_id_pattern` / `terminal_id_dup` geldi).
- **`wfah[]` artık sınıflandırılmış geliyor** (`WfahView`): `kind` (14 değerli KAPALI
  liste) + hazır `label` + `action`/`node` Ref'leri + `system` + `from_call` + `step`.
  İstemci bir daha `call:<key>/`, `escalate:<node>:<idx>[:skipped]`, `_branch_*`
  metinlerini AYRIŞTIRMAZ — o iş motora taşındı (portalda `classifyWfahAction` silindi).
  **Motorun İÇİNDEKİ marker adları ve `$wfah` izdüşümü (`{seq, action, actor, input,
  at}`) DEĞİŞMEDİ** — yayınlanmış akışlar `count($wfah, #.action == "...")` ile sayıyor.
  `input` payload'u AYNEN taşınır; ekranda anlam taşıyan alanlar (collapse `reason`)
  etikete ÇEKİLİR ki istemci payload içindeki ham anahtarları basmak zorunda kalmasın.

### `wfe_core::v22::display` (saf, birim testli)

`action_label` · `node_label` · `humanize_key`. Etiketlerin üretildiği TEK yer burasıdır;
`to_possible_action` ile simülasyon rotaları da aynı çeviriyi kullanır (sim ile gerçek
akış aynı şekli döndürmek zorunda). `_` ile başlayan anahtarlar (`_branch_cancelled`)
olduğu gibi kalır. Editör aynası: `utils/globalAction.humanizeKey`.

## WFD taslak kilidi (T‑B4, pessimistic)

Tasarım: `docs/superpowers/specs/2026-08-11-draft-kilidi-design.md`; karar kaydı
`docs/spec/decisions.md` "T‑B4".

- **Kilit `wf.wfd_meta`'da iki kolon** (`lock_user_id` / `lock_acquired_at`), ayrı tablo
  DEĞİL — kilit koşulu mutasyonun kendi `WHERE`'ine girsin (kontrol-sonra-yaz açığı
  olmasın).
- **SÜRE SINIRI YOK** (2026-08-18, KIRICI): kilit, editör taslağı AÇIK TUTTUĞU sürece
  sahibindedir. `lock_expires_at` kolonu, 5 dk TTL, `T-60s` popup'ı ve "süre doldu →
  otomatik kaydet + bırak" yolu KALDIRILDI
  (`migrations/wf/20260818000001_wfd_draft_lock_no_ttl.sql`; göç mevcut kilitleri önce
  serbest bırakır, aksi halde canlı kilitler kalıcı olurdu). Alma tek `UPDATE`, `WHERE`
  cümlesi CAS: `(lock_user_id IS NULL OR lock_user_id = $user)`; kilit zaten bizdeyse
  çağrı ETKİSİZDİR (tazeleme diye bir iş yok). `lock_acquired_at` mevcut değeri KORUR.
- **Bırakma AÇIK bir eylemdir; başka taslağa geçmek kilidi DÜŞÜRMEZ.** Tasarımcı fikir
  almak için komşu akışa gidip gelirken kilidini kaybetmemeli. Sonuç: bir kullanıcı aynı
  anda BİRDEN ÇOK kilit tutabilir (istemci tarafında modül düzeyinde kayıt). Bırakma
  yolları: "Kilidi bırak" düğmesi (önce KAYDEDER sonra bırakır), sayfa kapanışında
  `pagehide` + `keepalive` DELETE (tutulan tüm kilitler), publish/submit (sunucu bırakır)
  ve yönetici "Kilidi kır".
- **Kilit bizdeyken taslak OTOMATİK KAYDEDİLİR** (istemci: 20 sn'de bir, yalnız
  değişiklik varsa + sekme gizlenince). Zorla-açmanın iş kaybettirmemesi buna bağlı:
  kaydedilmemiş iş yalnız sahibinin belleğindedir, sunucu onu kaydedemez ve kilit
  kırıldıktan sonra sahibi de kaydedemez (409). "Kırarken sahibinin işini kaydet"
  sunucuda UYGULANAMAZ — işi önceden taşımak tek çözüm.
- **`DELETE .../lock?force=true` — yönetici zorla açma.** Yetki `require_manage_on_wfd`
  (tenant admin VEYA proje admini); tasarım yetkisi YETMEZ, yoksa proje üyesi olan herkes
  birbirinin kilidini kırabilir ve kilit anlaşma olmaktan çıkardı. Süresiz kilitte bu yol
  ZORUNLU: tarayıcısı çöken kullanıcının kilidi kendiliğinden düşmez. Repo tarafı ayrı
  fonksiyon (`force_release_lock`), `release_lock`e bayrak EKLENMEDİ — birleştirmek,
  yetki kontrolünü atlayan bir çağrının sessizce zorla-açmaya dönüşmesini bir `bool`luk
  mesafeye indirirdi. `GET .../lock` yanıtındaki `can_force` yalnız GÖSTERİM içindir.
- **Tüm taslak mutasyonları kilit ister** (kaydet/yayınla/onaya gönder/sil); onay/ret
  İSTEMEZ (pending düzenlenemez). Başarılı publish/submit kilidi bırakır.
- `publish`/`submit`'te kilit **ROTADA da** sorulur (`require_draft_lock`): o rotaların
  ön kapıları adapter'a girmeden belgeyi parse ediyor, kilit sorulmazsa yetkisiz
  kullanıcı 422 alıp yanlış yola sevk edilir.
- **Kilit durumu draft GET'ine GÖMÜLMEZ** (`GET .../lock` ayrı uç): draft GET'i ham WFD
  belgesi döndürür, kökü `additionalProperties: false`.
- İki kod: `draft.locked` (başkasında, kullanıcıya gösterilir) · `draft.lock_required`
  (kimsede değil/sende değil → istemci kilidi alıp kendiliğinden tekrar dener).
- **Kilitsiz kaydetme REDDEDİLİR** — bilinçli sözleşme kırılması; aksi halde kilit
  almayan iki istemci birbirini yine ezer.

## WF Admin (akış-içi yetkili) — `wfd.wf_admin[]`

Tasarım: `docs/superpowers/specs/2026-08-11-wf-admin-design.md`; karar kaydı
`docs/spec/decisions.md` "T‑A5". **agnoflow PLATFORM admini ile karıştırılmamalıdır:**
platform admini (`X-Admin-Key`) sistemi yönetir, WF Admin tek bir akışın gidişatına
müdahale eder ve yetkisi WFD'den doğar.

- **Şekil `listable[]` ile AYNI** (`CaGrantRule { c_a, when? }`, `$defs/caGrantRule`) ve
  aynı matcher; dizi olması "çoklu grant = çoklu kayıt" (bir C_A kuralında VEYA yok).
  `listable` bu tipin alias'ıdır — kural şekli TEK yerde durur.
- **Üç yetki:** (1) claim devri — kapı `node.reassign eşleşir VEYA wf_admin eşleşir`,
  hedef hâlâ node `c_a`'sına uymak zorunda (uymayan hedef claim'i tutar ama `apply_action`
  c_a'yı yeniden sorar → akış kilitlenir); (2) escalation müdahalesi
  (`POST /wfe/:id/escalation/fire|skip`, YALNIZ `wf_admin` — `node.reassign` açmaz);
  (3) görünürlük (`can_view` (e)).
- **Aksiyon yetkisi VERMEZ.** WF Admin işi yönetir, işi yapmaz; aksiyon için node
  `c_a`'sına uyması gerekir. Akışı bitirme/iptal, rastgele node'a taşıma ve `$ctx`'e yazma
  da YOK.
- **Marker sözleşmesi:** elle tetikleme otomatik yolun AYNI marker'ını yazar
  (`escalate:<node>:<idx>`) — yayınlanmış akışlar `count($wfah, ...)` ile karar veriyor,
  ayrı ad sayımı bozar; ayrım AKTÖRDEDİR. `wfes_effects`'teki `$actor` system KALIR.
  Atlama marker'ı `escalate:<node>:<idx>:skipped` — **`escalate:` öneki ZORUNLU**, yoksa
  `next_escalation`'ın tabanı (son escalation-DIŞI kayıt) kayar ve o node'un tüm
  sayaçları sessizce sıfırlanır.
- **Atlama `append_marker` ile yazılır** (yeni `WfeStore` metodu, **varsayılan
  implementasyon YOK**): atlama geçiş değil audit satırıdır. Varsayılan no-op, hiçbir şey
  yazmayan bir store'a izin verirdi ve adım tekrar ateşlenirdi.
- Adım numarası istemciden ALINMAZ (sıradaki ateşlenmemiş adım işlenir); vade GEREKMEZ.
  Paralel modda kol ipucu (`node`) zorunlu, tek-kol modda yok sayılır.
- `GET /wfe/:id` yanıtı `next_escalation` taşır — görmediği sayacı yönetmek kör karar
  olurdu.

## Tenant permission havuzu (rol = permission grubu)

Tasarım: `docs/superpowers/specs/2026-08-11-tenant-permission-rol-modeli-design.md`
(T‑A1, T‑A2). agnoflow burada tenant'ın **merkezi yetki dizinidir**.

- **Bunlar agnoflow'un yetkileri DEĞİL**, tenant'ın kendi iş yetkileri ("1043",
  `KREDI_ONAY`); motor anlamını BİLMEZ. agnoflow saklar, dağıtır, sorulunca cevaplar.
  agnoflow'un kendi yetkileri (WFD‑Observer / WF Admin / doğrudan claim — T‑A4/A5/A6)
  AYRI bir eksendir, bu havuz onları karşılamaz.
- **Motor bu katmandan habersizdir**: `wfe-core`'a tek satır girmez, `c_a`/`c_r` tek
  kural modeli değişmez, `$p` diye ZEN namespace'i YOKTUR, `schema.json` ve golden
  fixture etkilenmez. (Not defterindeki K1 duruşunun aynısı.)
- **T‑A1 kararı: yeni katman YOK.** "Profil" Rol'ün eş anlamlısıdır; `org.r` TEK
  katalogdur ve iki işi birden yapar — motorun `c_a.c_r` rol kanalı + permission grubu.
- 4 tablo: `org.p` (havuz) · `org.rp` (rol→yetki) · `org.up` (kişisel ıskarta,
  `up_type` şimdilik yalnız `'excluded'`) · `org.orgtnt_api_key` (`/ext` erişimi).
- **Etkin küme**: `⋃ rp(etkin_rol(u)) − up_excluded(u)`; bir rol kullanıcının **en az
  bir** biriminde etkinse sayılır (`check_user_role` semantiği birim başına aynen
  korunur). `org.ur.orgu_id IS NULL` ve `orgu_scope` yetki ÜRETMEZ — `check_user_role`
  da okumuyor. Rol ıskartasına timeslice UYGULANMAZ (motor paritesi), `org.up`
  ıskartasına UYGULANIR.
- **Saf/I/O ayrımı**: karar `crates/org/src/permission.rs`'de (saf, testler orada),
  satır çekme `repo/permission.rs`'de. Süzme (`is_active`, timeslice) SQL `WHERE`'ine
  YAZILMAZ — yazılırsa kural test dışına düşer (bu repoda DB'li test koşulmuyor).
- **`code` ASCII** (`[A-Za-z0-9._:-]{1,128}`), benzersizlik `lower(code)` üzerinde.
  Türkçe harf yasak çünkü PG `lower()` ile Rust `to_lowercase()` `İ`'de ayrışır.
  `code` yeniden adlandırılabilir, atamalar `p_id`'ye bağlıdır.
- **Üç kabuk, tek mantık**: `/org/...` yönetim (X‑Admin-Key), `/ext/permissions/*`
  dış uygulama (X‑Api-Key, SALT OKUMA), `GET /portal/me/permissions` (JWT, yalnız
  kendi kümesi). `/ext` `/org` altında OLAMAZ — `main.rs` tüm `/org` ağacını küresel
  X‑Admin-Key middleware'ine sarıyor.
- Küme uçları **`PUT`** (kutucuk ekranı tek transaction'da diff uygular); kullanımdaki
  yetki silinmez → 409 `permission.in_use`, `is_active=false` kullanılır. Permission
  **JWT'ye gömülmez** (TTL saatlerce; ıskarta anında etkisiz kalırdı).

## DB bağlantı kapsamı (global / lokal)

- `wf.db_connection.scope`: `global` = tenant genelinde, HER projedeki her WFD'de görünür
  (Ayarlar sayfasından yönetilir); `local` = yalnız TEK WFD'de görünür (WFD ayarları
  sekmesinden yönetilir). Global satırlar WFD ekranında SALT-OKUNUR.
- Lokal sahiplik anahtarı `(project_id, wfd_name)` — `wfd_id` DEĞİL (her versiyon ayrı
  `wfd_id` satırıdır). Grup adı değişince `repo::update_group_metadata` lokalleri taşır;
  gruptaki son satır silinince `repo::delete_draft` onları temizler.
- Kapsam create'te belirlenir, **update'te değişmez**. `GET /db/connections` `wfd_id`
  verilirse global + o WFD'nin lokalleri, verilmezse yalnız global döner.
- WFD yazma uçları başka bir WFD'nin lokalini referans eden dokümanı 422 ile reddeder
  (`routes::db::assert_no_foreign_local_connections`). Detay: `docs/spec/decisions.md`.

## Ortam konfigürasyonu (`$env`)

Bir WFD bir kez tasarlanır, şirketin farklı ortamlarında (test/prod/uat) koşar.
Tasarım: `docs/superpowers/specs/2026-08-04-env-config-design.md`.

- **Depolama DB'de, şifreleme anahtarı deployment'ta.** Değerler `wf.wfd_env_var`,
  secret'lar `value_enc` (AES-256-GCM); anahtar `DB_CONN_SECRET` env değişkeninden
  (K8s Secret + SOPS). GitLab'ın `ci_variables` + `gitlab-secrets.json` mimarisiyle
  aynı: DB dump'ı tek başına işe yaramaz.
- **Sahiplik `(project_id, wfd_name)`** — `wfd_id` DEĞİL (lokal `db_connection` ile
  aynı gerekçe). Conf WFD dokümanının DIŞINDA: doküman `(wfd_id, version)` bazında
  immutable, prod domaini değişince yeni versiyon publish etmek gerekmesin.
- **Ortam runtime'da seçilir.** `POST /wfe` body'sinde `environment` **ADI**; verilmezse
  tenant varsayılanı. `wfe.environment_id`'ye yazılır ve **ömür boyu sabit** kalır —
  timer/retry/escalation'da çağıran yoktur, `$env` ancak satırdan çözülebilir. Çağıran
  DEĞER geçiremez: geçirebilseydi prod akışı başkasının sunucusuna yönlendirilirdi.
  WFC'de çocuk ebeveynin ortamını **miras alır**.
- **`$env` ara-değer çözülen TEK namespace'tir**: `"$env.AUTH_API/v1/users"` çalışır.
  Anahtar `[A-Z][A-Z0-9_]*`, ilk küçük harf/`/`/`:` karakterinde biter. Tam eşleşme
  (`"$env.MAX_TUTAR"`) `value_type`'a göre TİPLİ döner.
- **Eksik anahtar `null` DEĞİL, HATADIR** — `$ctx`'in aksine. Null bir domain
  `https://null/v1` üretirdi. Publish kapısı bunu önceden yakalar (`env.missing_key`).
- **Secret'lar `EvalEnv`/`EffectEnv`'e HİÇ girmez** (tip düzeyinde: `PublicEnv`). ZEN ve
  `wfes_effects` secret göremez → ctx'e yazılamaz → portalda görünemez. Secret yalnız
  autoexec config / `db_connection` alanlarında çözülür ve `resolved_config()` ile hata
  metinlerinde `[MASKED]` olur.
- **Secret'lar taslakta da çözülür** (2026-08-04 kararı). Başta GitLab'ın "protected
  variable" kuralı alınmıştı (yalnız published); kaldırıldı çünkü yanlış eksende
  koruyordu — tasarımcı anahtar isteyen bir ucu editörde hiç deneyemiyordu. Erişim
  kontrolü ağ katmanının işi (FW / ortam erişilebilirliği). Koruma kalkmadı, yer
  değiştirdi: secret **kullanılabilir ama okunamaz** (maskeleme + ZEN/effects yasağı).
  Geri getirmek istenirse seam `repo::env::load_run_env(include_secrets)`; doğru eksen
  ortam bazlı olurdu (`wf.environment.is_protected`), taslak bazlı değil.
- `/autoexec/test` ve `/wfe/simulate` gövdelerinde `orgtnt_id` + `wfd_id` + `environment`
  verilirse `$env` bağlanır; verilmezse boş ortam (eski istemciler etkilenmez).
- `env_id IS NULL` = `*` joker kapsam; çözüm **tam eşleşme > joker > hata**.
- `db_connection` alanları (host/port/database/username/secret/options) `$env` ile
  şablonlanır — TEK satır tüm ortamlara hizmet eder, ortam kolonu yoktur. `port` bu
  yüzden `text`; çözümden sonra parse edilir.
- Yeni ifade-yüzeyi kuralı `validator::expression_issues`'a yazılır (editör ile ortak);
  doküman geneli referans toplama `validator::env_references`.

## Tenant metadata + marka varlıkları (logo/favicon)

- `org.orgtnt` kurumsal metadata'yı TİPLİ kolonlarda tutar (display_name, brand_color, legal_name,
  tax_no/tax_office, iletişim, city/country, timezone/locale/currency, external_id) + esnek
  tercihler için `settings jsonb`. DB CHECK'leri biçim garantisi verir; ihlal `error.rs`'te kısıt
  ADINDAN 400/409'a çevrilir (SQL metni sızmaz).
- `PATCH /org/orgtnt/{id}` semantiği: **alan gönderilmezse değişmez, boş string temizler** (NULL).
  Zorunlular (name/code/timezone/locale/currency) boş gönderilirse 400. Okuma+yazma tek
  transaction'da `FOR UPDATE` ile (repo `orgtnt::patch`).
- Logo/favicon BAYT'ları WFD JSON ile AYNI tenant-prefixli bucket'ta, `logo/` dizininde:
  `{orgtnt_id}/logo/{slot}.{ext}` (`wf_wfd::storage::tenant_asset_key`). DB yalnız
  anahtar+mime+zaman damgası tutar; uzantı değişen yeniden yüklemede eski blob silinir.
- Rotalar: admin `PUT/GET/DELETE /org/orgtnt/{id}/logo/{slot}` + `GET .../branding` (X-Admin-Key);
  portal SALT OKUMA `GET /portal/branding` ve `GET /portal/branding/logo/{slot}` (JWT, tenant
  token'dan çözülür). GET yetki ister → istemci `<img src>` yerine blob→objectURL kullanır.
  Doğrulama+servis `crate::branding`'de: logo png/jpeg/webp/svg ≤2 MB, favicon +ico ≤512 KB;
  SVG `nosniff` + katı CSP ile servis edilir.
