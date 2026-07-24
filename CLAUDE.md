# CLAUDE.md — agnoflow-engine

Bu repo **WFD v2.2** (Named Nodes, Single-Rule C_A) modelini çalıştıran çok-tenant'lı
workflow engine'dir. Spec ile kod çelişirse SPEC kazanır: kanonik dosyalar
`docs/spec/` altındadır (kaynak: WFD-EDITOR reposu `docs/spec/`; senkron tutulur).
Alınan tasarım kararları: `docs/spec/DECISIONS_v2_2.md`.

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
- ZEN namespace'leri: `$ctx $wfah $node $actor $timestamp $wfe_id $action.input.* $exec.result.*` (`$exec.response.*` = hata). Not: zen-expression'da `count()` yok, `len()` var.
- `wfd_version: "2.2"` zorunlu; eski format hem upload hem fetch'te reddedilir.

## Çalışma kuralları

- Her değişiklikten sonra `cargo test --workspace`; golden fixture (`docs/spec/example-wfd_kredi-basvuru_v2_2.json`) DEĞİŞTİRİLMEZ — kod fixture'a uyar.
- Zamana bağlı testlerde `#[tokio::test(start_paused = true)]` kullan (retry/timeout gerçek beklemeden koşar).
- Migration'lar psql ile manuel uygulanır (`migrations/org`, `migrations/wf` sırasıyla); sqlx migrate kullanılmıyor.
- Çok elemanlı eski c_a array'i ile karşılaşırsan OTOMATİK dönüştürme — dur ve sor (M10).

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
  `422 code:"attachment.missing"` döner. Detay: `docs/spec/DECISIONS_v2_2.md` Madde 8.
