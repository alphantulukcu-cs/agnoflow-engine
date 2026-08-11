# CLAUDE.md — agnoflow-engine

Bu repo **WFD v2.2** (Named Nodes, Single-Rule C_A) modelini çalıştıran çok-tenant'lı
workflow engine'dir. Spec ile kod çelişirse SPEC kazanır: kanonik dosyalar
`docs/spec/` altındadır (kaynak: WFD-EDITOR reposu `docs/spec/`; senkron tutulur).
Alınan tasarım kararları: `docs/spec/decisions.md`.

## Crate haritası

| Crate | Sorumluluk |
|---|---|
| `wfe-core` | Saf engine. `types/wfd_v22` (model+slug), `validator` (cross-ref/slug/graf/expression), `v22/` runtime: `eval` (ZEN namespace'leri), `resolver` (c_orgu), `matcher` (§3 authorize), `visibility` (§4 AYRI matcher), `effects` ($-string), `pipeline` (§7 atomik transition + trigger retry/catch/timeout + escalation), `ports` (WfdStore/WfeStore/AutoexecRunner). I/O YOK. |
| `org` | ORGT/ORGU/ORGTNT/UR repo + ORGTRVLANG parser/executor (ltree SQL). |
| `wfd` | WFD depolama: meta PostgreSQL, JSON OpenDAL. Upload/fetch'te v2.2 kapısı + validator; (wfd_id,version) immutable cache. |
| `wfe` | Adapter'lar: `WfeAdapter` (WfeStore — create/commit TEK transaction, claim CAS), `OrgAdapter`, `LiveAutoexecRunner` (rest/sql/calc), `WfeExecutor` (orkestrasyon + `tick_timers`), `sim` (store'suz simülasyon durumu). |
| `server` | Axum API: `/wfd` (upload/validate/list/get), `/wfe` (start/apply/claim/query/possible-actions/list), `/wfe/simulate`, `/autoexec/test`, `/portal` (JWT: login, pool, wfd, wfe), `/org` (X-Admin-Key). 60s timer sweeper. |

## Değişmezler (spec'ten)

- C_A **TEK KURALDIR**, iki biçim: **çapalı** `{c_orgu, c_r?, c_u?}` → match = `resolved(c_orgu) AND (rol OR c_u)`; **çapasız** `{c_u}` (c_orgu HİÇ yok) → match = `c_u`, kişi tenant genelinde eşleşir. Çapasızda `c_u` zorunlu, **`c_r` YASAK** (şema `oneOf` + validator `c_a_anchorless_role` + matcher rol kanalını hiç sormaz). Verilmeyen alan **false** (wildcard değil); c_u rol-agnostik. Çapasız aday cache girdisi birim taşımaz (`any_orgu: true`), havuz sorgusunda ayrı filtre.
- Node key = `slug(c_a)` (§2a); aynı canonical c_a ikinci node'da OLAMAZ.
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
  JWT `/portal/wfe/*` ağacında: `GET /wfe/:id/attachments` (durum), `PUT/GET/DELETE .../:group/:item`.
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
- **Başlatma aksiyonu için REZERVASYON**: `POST /wfe/reserve` → wfe_id (DB'de wfe satırı
  yok, `wf.wfe_reservation` defterinde kayıt var) → dosyalar o id'nin altına yüklenir →
  `POST /wfe {…, wfe_id}` kapıyı kontrol eder, eksikse WFE HİÇ oluşmaz. Yükleme rotaları
  rezerve edilmiş id'yi de kabul eder (yetki: rezervasyonun sahibi). Başlatılmayan
  rezervasyonlar saatlik süpürücüyle dosyalarıyla silinir (TTL 24 saat,
  `server/src/reservation.rs`). Belge istemeyen akışta rezervasyon gerekmez.
- Gate server-side ve AKSİYON BAZLI: `apply_action`/`submit_action` submit edilen aksiyonu KAPAYAN
  grupların `required` dosyaları eksikse `422 code:"attachment.missing"` döner
  (`status_for_node(..., Some(action))` → `gates` alanı → `satisfied`/`missing_required` yalnız
  kapayanları sayar). `GET /wfe/:id/attachments` aksiyon sormaz: her grup `gates: true` + kapsamı
  `actions` ile döner, süzme istemcidedir. Detay: `docs/spec/decisions.md` Madde 8.

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
- **draft → (dosya) → publish** deseni (K5): `POST /wfe/:id/notes` draft yaratır (yalnız
  yazarı görür) → istenirse `PUT .../notes/:note_id/files` ile dosya → yayınlama iki
  yoldan: aksiyonla (`POST /wfe/:id/actions` gövdesinde `note_id`, `ApplyBody.note_id`) ya
  da serbest (`POST .../notes/:note_id/publish`). Aksiyonla yayınlamada çapa apply'ın
  ÜRETTİĞİ `wfah_seq`'dir, `node` geçişin `from_node`'udur (notun yazıldığı adım). Apply
  BAŞARILI ama not yayınlanamazsa cevaba `note_error` eklenir, **aksiyon geri alınmaz** —
  not draft kalır, kullanıcı tekrar yayınlar (`routes/wfe.rs::apply_action`).
- **Değişmezlik (K3)**: yayınlanmış not `body` üzerinde UPDATE edilmez. Silme yerine
  gizleme: `hidden_at`/`hidden_by` dolar, gövde DB'de kalır, API `{hidden:true}` döner.
  Gizleme YALNIZ yazarı yapabilir (WFE'yi görebilen herkes değil — aksi halde karar delili
  hedefi tarafından ekrandan kaldırılabilirdi). Gizli notta gövde VE dosyalar API'den
  SIZMAZ (dosyalar notun içeriğinin parçasıdır).
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
