//! OpenAPI belge kökü — `utoipa_axum::OpenApiRouter` ile route'lardan toplanan
//! path'lerin üstüne info/servers/tags/security şemalarını ekler.
//!
//! Path'ler her route modülünde `#[utoipa::path]` + `routes!()` ile bildirilir;
//! burada YALNIZCA belge meta verisi ve `components.securitySchemes` vardır.
use utoipa::{
    openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

/// Belge kökü. Gerçek path listesi `OpenApiRouter::with_openapi(ApiDoc::openapi())`
/// üzerinden route'lardan gelir; bu struct'ın kendi `paths()`'i boştur.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "agnoflow-engine API",
        description = "WFD v2.2 (Named Nodes, Single-Rule C_A) çok-tenant workflow engine HTTP API.",
        version = env!("CARGO_PKG_VERSION"),
    ),
    modifiers(&SecurityAddon),
    servers(
        (url = "http://localhost:3000", description = "Yerel geliştirme"),
        (url = "http://agnoflow.staging.cs.com.tr", description = "Staging"),
    ),
    tags(
        (name = "wfd", description = "WFD tanımları: upload/validate/draft/publish + dashboard insight'ları"),
        (name = "wfe", description = "WFE örnekleri: start/apply/claim/query/possible-actions"),
        (name = "simulate", description = "Store'suz simülasyon"),
        (name = "autoexec", description = "Autoexec adım testleri (rest/sql/calc)"),
        (name = "attachments", description = "Ek-belge yükleme/durum (portal edge)"),
        (name = "notes", description = "WFE not defteri: draft/publish/gizle + dosya iliştirme (Faz 1-2)"),
        (name = "auth", description = "Uygulama JWT login/kimlik"),
        (name = "users", description = "Kullanıcı yönetimi"),
        (name = "project", description = "Proje yönetimi"),
        (name = "templates", description = "Predefined WFD şablon galerisi"),
        (name = "delegation", description = "Yetki devri"),
        (name = "portal", description = "Portal JWT ağacı: login/pool/wfd/wfe"),
        (name = "org", description = "Organizasyon admin API (X-Admin-Key)"),
        (name = "db", description = "DB admin/bakım API (X-Admin-Key)"),
        (name = "ext", description = "Tenant'ın dış uygulamaları: permission sorgulama (X-Api-Key, salt okuma)"),
    ),
)]
pub struct ApiDoc;

/// Üç auth şemasını `components.securitySchemes`'e ekler. Her endpoint kendi
/// `#[utoipa::path(security(...))]` bildirimiyle bunlardan uygun olan(lar)ı seçer.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        // components daima vardır (derive üretir); yine de güvenli tarafta kalalım.
        let components = openapi.components.get_or_insert_with(Default::default);

        // Uygulama + portal JWT'si (Authorization: Bearer <token>).
        components.add_security_scheme(
            "bearer_jwt",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("Uygulama/portal JWT — /auth veya /portal/auth login döner."))
                    .build(),
            ),
        );

        // Portal-edge aktör başlıkları (token'sız direkt /wfe/* çağrıları).
        // extract_actor üç ayrı header okur; her biri ayrı apiKey şemasıdır ki
        // Swagger "Authorize" diyaloğu üçünü de doldurabilsin.
        components.add_security_scheme(
            "x_actor_orgu",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Actor-Orgu",
                "Aktörün org unit id'si (UUID) — direkt /wfe/* için.",
            ))),
        );
        components.add_security_scheme(
            "x_actor_user",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Actor-User",
                "Aktörün kullanıcı id'si (UUID) — direkt /wfe/* için.",
            ))),
        );
        components.add_security_scheme(
            "x_actor_role",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Actor-Role",
                "Aktörün rol slug'ı — direkt /wfe/* için.",
            ))),
        );

        // Admin API anahtarı (/org, /db).
        components.add_security_scheme(
            "x_admin_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Admin-Key",
                "Admin API anahtarı — /org ve /db için (ADMIN_API_KEY).",
            ))),
        );

        // Tenant kapsamlı SALT OKUMA anahtarı (/ext). X-Admin-Key'in aksine tek
        // tenant'a bağlıdır ve yazma rotası yoktur.
        components.add_security_scheme(
            "x_api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Api-Key",
                "Tenant API anahtarı (agp_…) — /ext salt-okuma uçları için. \
                 /org/orgtnt/{id}/api-keys ile üretilir.",
            ))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Modifier üç auth ailesini de components'e ekliyor mu (DB gerektirmez).
    #[test]
    fn security_schemes_present() {
        let doc = ApiDoc::openapi();
        let schemes = doc
            .components
            .expect("components")
            .security_schemes;
        for name in [
            "bearer_jwt",
            "x_actor_orgu",
            "x_actor_user",
            "x_actor_role",
            "x_admin_key",
            "x_api_key",
        ] {
            assert!(schemes.contains_key(name), "eksik güvenlik şeması: {name}");
        }
    }

    #[test]
    fn info_and_servers_set() {
        let doc = ApiDoc::openapi();
        assert_eq!(doc.info.title, "agnoflow-engine API");
        let servers = doc.servers.unwrap_or_default();
        assert!(
            servers.iter().any(|s| s.url.contains("localhost")),
            "localhost server bekleniyordu"
        );
    }
}
