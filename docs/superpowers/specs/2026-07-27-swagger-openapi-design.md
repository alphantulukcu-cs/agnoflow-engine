# Eksiksiz OpenAPI + Swagger UI — Tasarım

**Tarih:** 2026-07-27
**Repo:** agnoflow-engine (`crates/server`, Axum 0.7)
**Hedef:** Tüm HTTP API'yi (~150 endpoint / ~78 route) kapsayan, kod ile senkron kalan,
interaktif "Try it out" destekli, güncel Swagger UI.

## Kararlar

| Konu | Karar |
|---|---|
| Spec üretimi | **utoipa (kod-anotasyonlu)** — tip-güvenli, drift etmez |
| Kapsam | **Tüm endpoint'ler** — admin `/org` `/db` dahil |
| Auth | **Üç şema da** — JWT bearer, `X-Actor`, `X-Admin-Key` |
| axum sürümü | **0.7'de kal** (Seçenek A). axum 0.8 migration ayrı iş |
| Prod geçidi | **`ENABLE_SWAGGER`** env, default açık; `=false` ile kapatılır |

## Bağımlılık yığını (axum 0.7 uyumlu)

- `utoipa = "5.5"` (features: `axum_extras`, `chrono`, `uuid`, `preserve_order`)
- `utoipa-axum = "0.1.3"` (`OpenApiRouter`, axum ^0.7)
- `utoipa-swagger-ui = "8.1"` (features: `axum`) — güncel Swagger UI 5.x bundle

> Not: en yeni `utoipa-swagger-ui 9.x` / `utoipa-axum 0.2` axum 0.8 ister. 8.1 de aynı
> modern Swagger UI frontend'ini serve eder; fark yalnız Rust binding API'sindedir.

## Mimari

Her `routes::<mod>::router()` bugün `axum::Router` döndürüyor →
`utoipa_axum::OpenApiRouter` döndürecek. Böylece route + doküman kaydı tek yerde,
drift olmaz.

```rust
// route modülü
pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(upload_wfd, list_wfd))   // #[utoipa::path] olan handler'lar
        .route("/legacy", get(legacy))            // henüz anote olmayanlar (kademeli)
        .with_state(state)
}
```

```rust
// main.rs
let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
    .nest("/wfd", routes::wfd::router(state.clone()))
    // ... tüm nest'ler ...
    .split_for_parts();
let app = router.merge(SwaggerUi::new("/swagger-ui")
    .url("/api-docs/openapi.json", api));
```

`admin_key` guard'ı (`.layer(from_fn(...))`) OpenApiRouter üzerinde de uygulanır;
`/org` ve `/db` için security şeması `x_admin_key` bildirir.

## Auth şemaları

`Modify` impl'i `components.security_schemes`'e ekler:

- `bearer_jwt` → HTTP bearer (JWT), `/portal/*`
- `x_actor` → apiKey, header `X-Actor`, doğrudan `/wfe/*` portal-edge
- `x_admin_key` → apiKey, header `X-Admin-Key`, `/org` `/db`

Her `#[utoipa::path]` ilgili `security(("bearer_jwt" = []))` bildirir.

## Şema kapsama

- **Tipli** request/response struct'ları → `#[derive(ToSchema)]` (tam şema).
  Paylaşılan model tipleri (`wf_wfd::models::WfdMeta` vb.) için ToSchema eklenir;
  başka crate'teki tiplerde ToSchema türetilemezse `schema(value_type = Object)` ya da
  yerel DTO ile sarılır.
- **Dinamik** `Json<serde_json::Value>` uçları → response `object` (serbest form) +
  açıklama/örnek. Bu uçlarda gövde şeması opak kalır (kod dinamik, kaçınılmaz).

## Mount & serve

- `GET /swagger-ui` → Swagger UI
- `GET /api-docs/openapi.json` → ham spec
- `ENABLE_SWAGGER=false` ise ikisi de mount edilmez.
- `servers`: `http://localhost:{PORT}` + `http://agnoflow.staging.cs.com.tr`.

## Doğrulama

- `cargo build -p wf-server` + `cargo test --workspace` (golden fixture değişmez).
- Generation testi: `ApiDoc::openapi()` üretilir; path sayısı > 0, üç güvenlik şeması
  mevcut, örnek birkaç path (`/wfd`, `/wfe/{id}`, `/org/...`) var.
- Canlı: `wf-server` çalışırken `curl /api-docs/openapi.json` + tarayıcıda `/swagger-ui`.

## İş sırası

1. Deps + `ENABLE_SWAGGER` config.
2. Foundation: `ApiDoc`, security `Modify`, `split_for_parts` + SwaggerUi mount.
3. Şablon modül (`wfd.rs`) tam anote → uçtan uca serve doğrula.
4. Kalan modüller anote (paralel, şablona bağlı kalarak).
5. Paylaşılan model tiplerine ToSchema.
6. Generation testi + tam `cargo test --workspace`.
7. Lokal canlı doğrulama.

## Kapsam dışı

- axum 0.8 migration (Seçenek B).
- Client SDK üretimi (openapi-generator vb.).
- Response örneklerinin elle zenginleştirilmesi (dinamik uçlar için).
