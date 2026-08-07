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

- C_A **TEK KURALDIR**: `{c_orgu, c_r?, c_u?}`; match = `resolved(c_orgu) AND (rol OR c_u)`; verilmeyen alan **false** (wildcard değil); c_u rol-agnostik.
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
  önbellekli). Tanımlı değilse deployment varsayılanına (`ATTACHMENT_STORAGE_*` env)
  düşülür. Secret'lar yalnız bu katmanda çözülür.
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
