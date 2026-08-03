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
- **Editör ifade doğrulaması `POST /wfd/validate-expression`** ile motora sorulur — `validator::expression_issues` (WFD validator'ının kullandığı fonksiyonun aynısı). Yeni bir ifade-yüzeyi kuralı eklenirken O fonksiyona yazılır, iki tüketici birlikte güncellenir.
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
Ek-belge deposu (attachments, WFD JSON storage'ından AYRI): `ATTACHMENT_STORAGE_BACKEND=local|s3`
(+`ATTACHMENT_STORAGE_PATH`, local default `../work-pool-portal/storage`; `ATTACHMENT_STORAGE_S3_BUCKET/REGION`).

## Attachments (ek-belge) sözleşmesi

- WFD şeması: root `attachments` katalogu (adlandırılmış gruplar → `items[]`) + `nodes.<key>.attachments`
  (grup key referansları). Engine core dosya I/O YAPMAZ — yalnız katalog+referansı metadata tutar.
- Varlık kontrolü + yükleme server portal edge'inde: `AttachmentStore` (opendal), storage anahtarı
  `attachments/{wfe_id}/{grup}/{item}`. Rotalar hem direkt `/wfe/*` (X-Actor, portal bunu kullanır) hem
  JWT `/portal/wfe/*` ağacında: `GET /wfe/:id/attachments` (durum), `PUT/GET/DELETE .../:group/:item`.
- Gate server-side: `apply_action`/`submit_action` hedef node'un `required` dosyaları eksikse
  `422 code:"attachment.missing"` döner. Detay: `docs/spec/decisions.md` Madde 8.

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
