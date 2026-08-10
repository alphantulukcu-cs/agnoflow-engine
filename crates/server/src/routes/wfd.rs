use utoipa_axum::router::OpenApiRouter;
use super::auth::{require_can_design, require_can_manage_project, AppAuth, MaybeAppAuth};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(upload_wfd, list_wfd))
        .routes(routes!(validate_wfd))
        .routes(routes!(validate_expression))
        .routes(routes!(usage_summary))
        .routes(routes!(execution_stats))
        .routes(routes!(unit_workload))
        .routes(routes!(node_load))
        .routes(routes!(aging_executions))
        .routes(routes!(escalation_forecast))
        .routes(routes!(dashboard_summary))
        .routes(routes!(create_draft))
        .routes(routes!(get_draft, save_draft, delete_draft))
        .routes(routes!(publish_draft))
        .routes(routes!(submit_draft))
        .routes(routes!(approve_draft))
        .routes(routes!(reject_draft))
        .routes(routes!(update_wfd_meta))
        .routes(routes!(get_wfd))
        .routes(routes!(new_draft))
        .routes(routes!(wfe_usage))
        .routes(routes!(get_layout, put_layout))
        .routes(routes!(get_scenarios, put_scenarios))
        .routes(routes!(run_scenarios))
        .routes(routes!(run_one_scenario))
        .with_state(state)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    orgtnt_id: Uuid,
    /// Verilirse liste bu projeyle sınırlanır.
    project_id: Option<Uuid>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[utoipa::path(get, path = "/", tag = "wfd", params(ListQuery),
    responses((status = 200, description = "WFD meta listesi", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn list_wfd(
    State(s): State<AppState>,
    MaybeAppAuth(auth): MaybeAppAuth,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    // Token'lı istekte tenant token'dan doğrulanır; üye yalnız atandığı
    // projelerin akışlarını görür. Token'sız okuma (sim/araçlar) eski davranış.
    if let Some(auth) = &auth {
        if auth.orgtnt_id != q.orgtnt_id {
            return Err(AppError("Tenant uyuşmuyor".into(), StatusCode::FORBIDDEN));
        }
    }
    let mut rows = wf_wfd::repo::list(&s.pool, q.orgtnt_id, q.project_id, limit, offset)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    if let Some(auth) = &auth {
        if auth.role != "admin" {
            let member_of: Vec<Uuid> =
                sqlx::query_scalar("SELECT project_id FROM wf.project_member WHERE user_id = $1")
                    .bind(auth.user_id)
                    .fetch_all(&s.pool)
                    .await
                    .map_err(internal_error)?;
            rows.retain(|w| member_of.contains(&w.project_id));
        }
    }
    Ok(Json(rows))
}

/// Yazma uçlarının ortak kapısı: hedef WFD'nin projesinde tasarım yetkisi.
async fn require_design_on_wfd(
    s: &AppState,
    auth: &AppAuth,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    let meta = wf_wfd::repo::get_meta_any(&s.pool, wfd_id, version)
        .await
        .map_err(map_wfd_err)?;
    if meta.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_design(&s.pool, auth, meta.project_id).await
}

/// Onay/yayın kapısı: tenant admin veya hedef projenin admini.
async fn require_approver_on_wfd(
    s: &AppState,
    auth: &AppAuth,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    let meta = wf_wfd::repo::get_meta_any(&s.pool, wfd_id, version)
        .await
        .map_err(map_wfd_err)?;
    if meta.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_manage_project(&s.pool, auth, meta.project_id).await
}

/// Yeni doküman yaratırken proje çözümü + yetki: body'de proje verilmişse o,
/// verilmemişse tenant'ın varsayılanı. Dönen id adapter'a AYNEN geçilir ki
/// yetki verilen proje ile yazılan proje ayrışamasın.
async fn resolve_project_for_write(
    s: &AppState,
    auth: &AppAuth,
    body_tenant: Uuid,
    project_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if body_tenant != auth.orgtnt_id {
        return Err(AppError("Tenant uyuşmuyor".into(), StatusCode::FORBIDDEN));
    }
    let project_id = match project_id {
        Some(id) => {
            wf_wfd::project::assert_in_tenant(&s.pool, id, auth.orgtnt_id)
                .await
                .map_err(map_wfd_err)?;
            id
        }
        None => wf_wfd::project::resolve_default(&s.pool, auth.orgtnt_id)
            .await
            .map_err(map_wfd_err)?,
    };
    require_can_design(&s.pool, auth, project_id).await?;
    Ok(project_id)
}

#[derive(Deserialize, ToSchema)]
struct UploadBody {
    orgtnt_id: Uuid,
    /// Verilmezse tenant'ın varsayılan projesi kullanılır (eski istemci uyumu).
    #[serde(default)]
    project_id: Option<Uuid>,
    /// v2.2 WFD dokümanı — yükleme kapısı + custom validator uygulanır (M14).
    wfd: Value,
}

#[utoipa::path(post, path = "/", tag = "wfd",
    request_body = UploadBody,
    responses((status = 200, description = "wfd_id + version", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn upload_wfd(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(body): Json<UploadBody>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_project_for_write(&s, &auth, body.orgtnt_id, body.project_id).await?;
    // Lokal DB bağlantıları WFD'ye aittir: doküman başkasının lokalini taşıyamaz.
    if let Some(name) = body.wfd.get("name").and_then(Value::as_str) {
        super::db::assert_no_foreign_local_connections(&s.pool, project_id, name, &body.wfd).await?;
    }
    let (wfd_id, version) = s
        .wfd
        .upload(body.orgtnt_id, Some(project_id), &body.wfd)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    Ok(Json(
        serde_json::json!({ "wfd_id": wfd_id, "version": version }),
    ))
}

/// Editör için: kaydetmeden doğrula — hata/uyarı listesi döner.
#[utoipa::path(post, path = "/validate", tag = "wfd",
    request_body = serde_json::Value,
    responses((status = 200, description = "valid/errors/warnings", body = serde_json::Value)))]
async fn validate_wfd(Json(wfd_json): Json<Value>) -> Result<Json<Value>, AppError> {
    // Şema ihlalleri AYRI kod (`schema`) ile ve tek tek raporlanır — yayın kapısı (upload/
    // publish) aynı şemayı reddederek durdurur, editör de aynı listeyi burada görür.
    // Parse hatası şemayı gölgelemesin diye şema ÖNCE koşar: serde `"c_r": []`'i sessizce
    // kabul ettiği için parse'a bakarak şema ihlalini hiç öğrenemezdik.
    let schema_errors: Vec<Value> = match wfe_core::schema::validate_document(&wfd_json) {
        Ok(()) => Vec::new(),
        Err(errs) => errs
            .iter()
            .map(|m| serde_json::json!({"code": "schema", "path": "$", "message": m}))
            .collect(),
    };
    let wfd = match wfe_core::types::wfd_v22::Wfd::from_value(wfd_json) {
        Ok(w) => w,
        Err(e) => {
            let mut errors = schema_errors;
            errors.push(serde_json::json!({"code": "parse", "path": "$", "message": e.to_string()}));
            return Ok(Json(serde_json::json!({
                "valid": false,
                "errors": errors,
                "warnings": [],
            })));
        }
    };
    let report = wfe_core::validator::validate(&wfd);
    let issue = |i: &wfe_core::validator::ValidationIssue| serde_json::json!({"code": i.code, "path": i.path, "message": i.message});
    let mut errors = schema_errors;
    errors.extend(report.errors.iter().map(issue));
    Ok(Json(serde_json::json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": report.warnings.iter().map(issue).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize, ToSchema)]
struct ValidateExpressionRequest {
    /// Doğrulanacak ZEN ifadeleri. Sıra korunur; yanıt aynı indekslerle döner.
    expressions: Vec<String>,
    /// İfadelerin ait olduğu WFD (wire biçimi). Verilirse TİP kuralları da koşar
    /// (`expr_types`: obje karşılaştırması, tip uyuşmazlığı, izdüşüm dışı `$wfah` alanı,
    /// kapısız sıralama) — tip bilgisi context şemasından ve `wfes_effects` eşlemesinden
    /// çıkarılır, ikisi de bu belgede durur.
    ///
    /// Verilmezse (ya da belge v2.2 olarak parse edilemezse — kurucu yarım bir taslakta
    /// da çalışır) yalnız yüzey kuralları koşar ve yanıt `typed: false` döner. Yayın kapısı
    /// tip kurallarını her hâlükârda uygular; buradaki eksiklik yalnız "hatayı ne kadar
    /// erken görüyorsun" farkıdır.
    #[serde(default)]
    wfd: Option<Value>,
}

/// Koşul kurucusu için: TEK TEK ZEN ifadelerini motorun kendi parser'ıyla doğrula.
///
/// Neden gerekli: editör ifadeleri JS'te değerlendirir, yani zen grameriyle ayrışabilir
/// (WOR-84: `every(...)` ve `count(filter(...))` aylarca editörde YEŞİL göründü, motorda
/// parse hatasıydı). Bu rota `validator::expression_issues` ile WFD validator'ıyla
/// **aynı** verdiği döner — kurucu artık yayınlamayı beklemeden gerçek cevabı görür.
///
/// Boş/whitespace ifade `ok: true` döner: "boş satır" kuralı editörün kendi işidir
/// (orada `true` üretir), burada parse hatası saymak mesajı ikiye bölerdi.
#[utoipa::path(post, path = "/validate-expression", tag = "wfd",
    request_body = ValidateExpressionRequest,
    responses((status = 200, description = "İfade başına hata/uyarı listesi", body = serde_json::Value)))]
async fn validate_expression(
    Json(req): Json<ValidateExpressionRequest>,
) -> Result<Json<Value>, AppError> {
    // Kötüye kullanım/kazara dev payload koruması — kurucuda en fazla birkaç ifade olur.
    if req.expressions.len() > MAX_VALIDATED_EXPRESSIONS {
        return Err(AppError(
            format!("en fazla {MAX_VALIDATED_EXPRESSIONS} ifade doğrulanabilir"),
            StatusCode::BAD_REQUEST,
        ));
    }
    Ok(Json(expression_report(&req.expressions, req.wfd.as_ref())))
}

const MAX_VALIDATED_EXPRESSIONS: usize = 64;

/// Rotanın saf gövdesi — DB'siz test edilebilsin diye ayrı (handler yalnız sınır kontrolü
/// yapar). Yanıt sırası girdi sırasıyla birebir hizalıdır; editör indeksle eşler.
///
/// `wfd` verilmişse tip kuralları da koşar; parse edilemeyen taslak SESSİZCE yüzey
/// kurallarına düşer (`typed: false`) — kurucu yarım belgede de çalışabilmeli, orada
/// "belgeniz geçersiz" demek ifade doğrulamasının işi değil.
fn expression_report(expressions: &[String], wfd: Option<&Value>) -> Value {
    let parsed = wfd.and_then(|v| wfe_core::types::wfd_v22::Wfd::from_value(v.clone()).ok());
    let env = parsed.as_ref().map(wfe_core::validator::expr_env);
    let results: Vec<Value> = expressions
        .iter()
        .map(|expr| {
            // Boş ifade `ok` döner: "boş satır" kuralı editörün kendi işidir (orada
            // `true` üretir), burada parse hatası saymak mesajı ikiye bölerdi.
            if expr.trim().is_empty() {
                return serde_json::json!({ "ok": true, "errors": [], "warnings": [] });
            }
            let mut issues = wfe_core::validator::expression_issues(expr);
            if let Some(env) = &env {
                issues.extend(wfe_core::expr_types::expression_type_issues(expr, env));
            }
            let pick = |want_error: bool| -> Vec<Value> {
                issues
                    .iter()
                    .filter(|(_, is_error, _)| *is_error == want_error)
                    .map(|(code, _, message)| serde_json::json!({"code": code, "message": message}))
                    .collect()
            };
            let errors = pick(true);
            serde_json::json!({
                "ok": errors.is_empty(),
                "errors": errors,
                "warnings": pick(false),
            })
        })
        .collect();
    // `typed`: tip kuralları koştu mu. Editör bunu göstermek zorunda değil ama "neden
    // yeşil?" sorusunun cevabı burada — belge gönderilmediyse/parse edilemediyse false.
    serde_json::json!({ "results": results, "typed": env.is_some() })
}

#[cfg(test)]
mod validate_expression_tests {
    use super::*;

    fn report(exprs: &[&str]) -> Vec<Value> {
        report_with(exprs, None)
    }

    /// Belge VERİLİRSE tip kuralları da koşar (bkz. `expression_report`).
    fn report_with(exprs: &[&str], wfd: Option<&Value>) -> Vec<Value> {
        let owned: Vec<String> = exprs.iter().map(|s| s.to_string()).collect();
        expression_report(&owned, wfd)["results"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// EDİTÖR SÖZLEŞMESİ: sıra korunur, `ok` yalnız HATA yokluğunu anlatır.
    #[test]
    fn report_is_index_aligned_and_ok_ignores_warnings() {
        let out = report(&[
            r#"count($wfah, #.action == "x") >= 1"#, // 0: temiz
            r#"every($wfah, #.action == "x")"#,      // 1: parse hatası
            r#"$wfah[len($wfah) - 1].action == "x""#, // 2: yalnız UYARI
            "",                                      // 3: boş → ok
            r#"$wfah[-1].action == "x""#,            // 4: negatif indeks hatası
        ]);
        assert_eq!(out.len(), 5);

        assert_eq!(out[0]["ok"], true);
        assert!(out[0]["errors"].as_array().unwrap().is_empty());

        assert_eq!(out[1]["ok"], false);
        assert_eq!(out[1]["errors"][0]["code"], "zen_parse");

        // Uyarı `ok`'u BOZMAZ — Kaydet kapısı yalnız hatalara bakar.
        assert_eq!(out[2]["ok"], true);
        assert_eq!(out[2]["warnings"][0]["code"], "wfah_index_unguarded");

        assert_eq!(out[3]["ok"], true);

        assert_eq!(out[4]["ok"], false);
        assert_eq!(out[4]["errors"][0]["code"], "zen_negative_index");
    }

    /// Editörün ürettiği tüm WFAH biçimleri bu rotadan temiz geçer
    /// (`wfe-core/tests/editor_zen_contract.rs` ile aynı küme).
    #[test]
    fn editor_generated_forms_pass_the_route() {
        for expr in [
            r#"count($wfah, #.action == "x") >= 2"#,
            r#"count($wfah, #.action == "x") == 1"#,
            r#"some($wfah, #.action == "x")"#,
            r#"all($wfah, #.actor.role != "x")"#,
            r#"none($wfah, #.action == "x")"#,
            r#"one($wfah, #.action == "x")"#,
            r#"$prev.action == "x""#,
            r#"$first.actor.role != "x""#,
            r#"contains(#.action, "inc")"#,
            r#"some($wfah, #.action in ["a", "b"])"#,
        ] {
            let out = report(&[expr]);
            assert_eq!(out[0]["ok"], true, "{expr}: {:?}", out[0]["errors"]);
        }
    }

    #[test]
    fn empty_request_is_empty_report() {
        assert!(report(&[]).is_empty());
    }

    // TİP kuralları belge verilince koşar. Editörün SERBEST ZEN satırı bu rotayı kullanır;
    // belge gönderilmeden `#.actor == "ali"` satırı yeşil görünüyordu ve hata ancak
    // Yayınla'da çıkıyordu. Kural setinin sahibi motor — cevabı satır yazılırken verir.
    fn golden() -> Value {
        serde_json::from_str(include_str!(
            "../../../../docs/spec/examples/kredi-basvuru.golden.json"
        ))
        .unwrap()
    }

    #[test]
    fn type_rules_run_when_the_document_is_supplied() {
        let wfd = golden();
        let out = report_with(&[r#"some($wfah, #.actor == "ali")"#], Some(&wfd));
        assert_eq!(out[0]["ok"], false, "{:?}", out[0]);
        assert_eq!(out[0]["errors"][0]["code"], "zen_object_compare");
    }

    #[test]
    fn type_rules_are_skipped_without_the_document() {
        // Belge yoksa YÜZEY kuralları koşar: ifade parse ediliyor, `ok` kalır.
        let out = report(&[r#"some($wfah, #.actor == "ali")"#]);
        assert_eq!(out[0]["ok"], true);
    }

    #[test]
    fn typed_flag_reports_whether_type_rules_ran() {
        let wfd = golden();
        let owned = vec!["true".to_string()];
        assert_eq!(expression_report(&owned, Some(&wfd))["typed"], true);
        assert_eq!(expression_report(&owned, None)["typed"], false);
        // Yarım/geçersiz taslak: kurucu çalışmaya devam eder, yalnız tip kuralları düşer.
        let draft = serde_json::json!({ "wfd_version": "2.2" });
        assert_eq!(expression_report(&owned, Some(&draft))["typed"], false);
    }

    /// Editörün TOPLAMA satırının ürettiği ifade de bu rotadan geçer (kurucu artık ZEN
    /// kutusundaki metnin TAMAMINI gönderiyor, satır türüne bakmıyor). `#.action` bir
    /// metindir; `avg` sayı dizisi ister — satır yazılırken reddedilmeli, Yayınla'ya
    /// kalmamalı.
    #[test]
    fn numeric_agg_over_text_field_is_rejected_by_the_route() {
        let wfd = golden();
        let out = report_with(&["avg(map($wfah, #.action)) > 0"], Some(&wfd));
        assert_eq!(out[0]["ok"], false, "{:?}", out[0]);
        assert_eq!(out[0]["errors"][0]["code"], "zen_agg_not_numeric");
    }

    #[test]
    fn valid_expression_stays_clean_with_the_document() {
        let wfd = golden();
        let out = report_with(
            &[r#"some($wfah, #.actor.role == "creditAnalyst")"#],
            Some(&wfd),
        );
        assert_eq!(out[0]["ok"], true, "{:?}", out[0]["errors"]);
    }
}

#[utoipa::path(get, path = "/{id}/{version}", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "WFD v2.2 dokümanı", body = serde_json::Value)))]
async fn get_wfd(
    State(s): State<AppState>,
    Path((wfd_id, version)): Path<(Uuid, i32)>,
) -> Result<Json<wfe_core::types::wfd_v22::Wfd>, AppError> {
    s.wfd
        .fetch(wfd_id, version)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::NOT_FOUND))
}

#[derive(Deserialize, ToSchema)]
struct CreateDraftBody {
    orgtnt_id: Uuid,
    /// Verilmezse tenant'ın varsayılan projesi kullanılır (eski istemci uyumu).
    #[serde(default)]
    project_id: Option<Uuid>,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    /// Editörün ürettiği başlangıç dokümanı; yoksa engine iskelet yazar.
    #[serde(default)]
    wfd: Option<Value>,
    /// Taslağın türetildiği predefined şablon versiyonu (galeri akışı doldurur).
    #[serde(default)]
    source_template_id: Option<Uuid>,
}

#[utoipa::path(post, path = "/draft", tag = "wfd",
    request_body = CreateDraftBody,
    responses((status = 200, description = "wfd_id + version", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn create_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(b): Json<CreateDraftBody>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_project_for_write(&s, &auth, b.orgtnt_id, b.project_id).await?;
    if let Some(tid) = b.source_template_id {
        // İz güvenilir olsun: şablon var ve aynı tenant'ta olmalı.
        let tpl = wf_wfd::template::get(&s.pool, tid)
            .await
            .map_err(map_wfd_err)?;
        if tpl.orgtnt_id != auth.orgtnt_id {
            return Err(AppError("Şablon bulunamadı".into(), StatusCode::NOT_FOUND));
        }
    }
    let (wfd_id, version) = s
        .wfd
        .create_draft(
            b.orgtnt_id,
            Some(project_id),
            &b.name,
            b.description.as_deref(),
            &b.tags,
            b.wfd.as_ref(),
            b.source_template_id,
        )
        .await
        .map_err(map_wfd_err)?;
    Ok(Json(
        serde_json::json!({ "wfd_id": wfd_id, "version": version }),
    ))
}

#[utoipa::path(get, path = "/draft/{id}/{version}", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "Taslak WFD json", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn get_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .fetch_draft_json(id, ver)
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

#[derive(Deserialize, ToSchema)]
struct SaveDraftBody {
    wfd: Value,
    #[serde(default)]
    description: Option<String>,
    /// Verilmezse (None) mevcut tags korunur; boş `[]` gönderilirse temizlenir.
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[utoipa::path(put, path = "/draft/{id}/{version}", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")), request_body = SaveDraftBody,
    responses((status = 204, description = "Kaydedildi")),
    security(("bearer_jwt" = [])))]
async fn save_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(b): Json<SaveDraftBody>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let meta = wf_wfd::repo::get_meta_any(&s.pool, id, ver)
        .await
        .map_err(map_wfd_err)?;
    super::db::assert_no_foreign_local_connections(&s.pool, meta.project_id, &meta.name, &b.wfd)
        .await?;
    s.wfd
        .save_draft(id, ver, &b.wfd, b.description.as_deref(), b.tags.as_deref())
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

/// Doğrudan yayın: onaycı (tenant admin | proje admini) VEYA admin'in
/// "doğrudan yayınlayabilir" bayrağını verdiği proje üyesi. Diğerleri
/// /submit ile onaya gönderir.
async fn require_can_publish_wfd(
    s: &AppState,
    auth: &AppAuth,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    if require_approver_on_wfd(s, auth, wfd_id, version)
        .await
        .is_ok()
    {
        return Ok(());
    }
    // Onaycı değil: tasarım yetkisi + kullanıcı bayrağı gerekir.
    require_design_on_wfd(s, auth, wfd_id, version).await?;
    let flag: Option<bool> =
        sqlx::query_scalar("SELECT can_publish FROM wf.app_user WHERE user_id = $1")
            .bind(auth.user_id)
            .fetch_optional(&s.pool)
            .await
            .map_err(internal_error)?;
    if flag == Some(true) {
        return Ok(());
    }
    Err(AppError(
        "Doğrudan yayın yetkiniz yok — taslağı onaya gönderin".into(),
        StatusCode::FORBIDDEN,
    ))
}

#[utoipa::path(post, path = "/draft/{id}/{version}/publish", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn publish_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_can_publish_wfd(&s, &auth, id, ver).await?;
    assert_env_keys_defined(&s, id, ver).await?;
    assert_attachment_storage_env(&s, id, ver).await?;
    s.wfd
        .publish_draft(id, ver)
        .await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "published" })))
        .map_err(map_wfd_err)
}

/// Yayın kapılarının doküman okuma yolu — `WfdStore::fetch` DEĞİL.
///
/// `fetch` yalnız `status='published' AND is_active` satırı görür (`repo::get_meta`);
/// kapılar ise publish/submit/approve'un ÖNÜNDE, satır hâlâ `draft` ya da
/// `pending_approval` iken koşar → her çağrı `wfd port error: wfd not found: <id> v<n>`
/// ile 422 dönüyordu. Ham JSON status'e bakmadan okunur ve aynı şema kapısından
/// (`from_value_checked`) geçirilir: kapı, yayınlanacak belgenin ta kendisini görür.
async fn fetch_wfd_for_gate(
    s: &AppState,
    wfd_id: Uuid,
    version: i32,
) -> Result<wfe_core::types::wfd_v22::Wfd, AppError> {
    let json = s
        .wfd
        .fetch_json_any(wfd_id, version)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    wfe_core::types::wfd_v22::Wfd::from_value_checked(json)
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))
}

/// Yayın kapısı: dokümandaki her `$env.X` referansı, WFD'nin SATIRI OLAN her ortamda
/// tanımlı olmalı.
///
/// Çekirdek yalnız referansları çıkarır (`validator::env_references`, I/O yok); DB'yle
/// karşılaştırma burada yapılır. Bu, ortam başına ayrı doküman modelinin tek zayıf noktası
/// olan **drift**'in karşılığıdır: test'e eklenip prod'a eklenmeyen anahtar, runtime'da
/// değil publish anında yakalanır.
///
/// Hiç satırı olmayan ortam SESSİZ geçilir — bir WFD'nin her ortamda koşması zorunlu
/// değildir; zorunlu tutmak yeni bir ortam açan herkesin tüm WFD'lerini bozardı.
async fn assert_env_keys_defined(s: &AppState, wfd_id: Uuid, version: i32) -> Result<(), AppError> {
    let wfd = fetch_wfd_for_gate(s, wfd_id, version).await?;
    let refs = wfe_core::validator::env_references(&wfd)
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    if refs.is_empty() {
        return Ok(());
    }

    let Some((project_id, wfd_name)) = sqlx::query_as::<_, (Option<Uuid>, String)>(
        "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(wfd_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .and_then(|(p, n)| p.map(|p| (p, n))) else {
        return Ok(());
    };

    // Anahtar × ortam: joker (`env_id IS NULL`) satır TÜM ortamları karşılar.
    let rows = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT e.name, v.key FROM wf.wfd_env_var v            LEFT JOIN wf.environment e ON e.id = v.env_id           WHERE v.project_id = $1 AND v.wfd_name = $2",
    )
    .bind(project_id)
    .bind(&wfd_name)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let mut wildcard: std::collections::BTreeSet<String> = Default::default();
    let mut per_env: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();
    for (env_name, key) in rows {
        match env_name {
            None => {
                wildcard.insert(key);
            }
            Some(name) => {
                per_env.entry(name).or_default().insert(key);
            }
        }
    }

    let mut missing: Vec<String> = Vec::new();
    for (env_name, keys) in &per_env {
        for key in &refs {
            if !keys.contains(key) && !wildcard.contains(key) {
                missing.push(format!("{env_name}: $env.{key}"));
            }
        }
    }
    // Hiç ortam satırı yoksa joker tek başına yeterli olmalı.
    if per_env.is_empty() {
        for key in &refs {
            if !wildcard.contains(key) {
                missing.push(format!("*: $env.{key}"));
            }
        }
    }

    if !missing.is_empty() {
        return Err(AppError(
            format!("env.missing_key — tanımsız ortam değişkenleri: {}", missing.join(", ")),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    Ok(())
}

/// Yayın kapısı: belge TOPLAYAN bir akış yayınlanırken ek-belge deposunun `$env` ayarları
/// TENANT'IN HER ORTAMINDA dolu olmalı.
///
/// `assert_env_keys_defined`den iki farkı var ve ikisi de bilinçli:
///
/// 1. **Anahtarlar dokümanda GEÇMEZ.** Depo konfigürasyonu doküman dışıdır
///    (`attachment_store`), dolayısıyla `$env.X` referans taraması bu anahtarları hiç
///    görmez — kendi kapısı olmak zorunda.
/// 2. **Varlık değil DEĞER aranır ve boş ortam SESSİZ GEÇİLMEZ.** `$env` referansları için
///    "hiç satırı olmayan ortam" meşrudur (WFD her ortamda koşmak zorunda değil); depo
///    ayarında değil: eksik ayar hata vermez, deployment varsayılanına düşer ve belgeler
///    müşterinin bucket'ı yerine sunucu diskine yazılır. Sessizce yanlış yere yazmak,
///    yayını durdurmaktan pahalıdır.
///
/// Kapı publish + submit + approve'un HEPSİNDE koşar: yalnız publish'te olsaydı yayın
/// yetkisi olmayan tasarımcı onaya gönderir, kapıya onaylayan çarpar.
async fn assert_attachment_storage_env(
    s: &AppState,
    wfd_id: Uuid,
    version: i32,
) -> Result<(), AppError> {
    let wfd = fetch_wfd_for_gate(s, wfd_id, version).await?;
    if !crate::attachment_store::collects_attachments(&wfd) {
        return Ok(());
    }

    let Some((project_id, wfd_name, orgtnt_id)) =
        sqlx::query_as::<_, (Option<Uuid>, String, Uuid)>(
            "SELECT project_id, name, orgtnt_id FROM wf.wfd_meta WHERE wfd_id = $1",
        )
        .bind(wfd_id)
        .fetch_optional(&s.pool)
        .await
        .map_err(internal_error)?
        .and_then(|(p, n, t)| p.map(|p| (p, n, t)))
    else {
        // Projesi olmayan (eski/serbest) WFD'nin `$env` sahipliği yoktur — değer
        // giremeyeceği bir kapıyla yayını kilitlemiyoruz (mevcut env kapısıyla aynı davranış).
        return Ok(());
    };

    let envs = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM wf.environment WHERE orgtnt_id = $1 ORDER BY name",
    )
    .bind(orgtnt_id)
    .fetch_all(&s.pool)
    .await
    .map_err(internal_error)?;
    if envs.is_empty() {
        return Err(AppError(
            "attachment_storage.missing_env — ek-belge deposu ayarlanamadı: tenant'ta hiç ortam tanımlı değil".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }

    // Satırlar: (ortam, anahtar) → (değer, dolu mu). Joker (`env_id IS NULL`) tüm ortamları
    // karşılar; tam eşleşme joker'i EZER (`repo::env::load_run_env` ile aynı öncelik).
    // "Dolu" tanımı satırın türüne göre değişir: secret'ta şifreli değerin VARLIĞI, düz
    // değerde boşluk kırpıldıktan sonra kalan metin.
    let rows = sqlx::query_as::<_, (Option<Uuid>, String, String, bool)>(
        "SELECT env_id, key, btrim(coalesce(value, '')) AS value, \
                (CASE WHEN is_secret THEN value_enc IS NOT NULL ELSE btrim(coalesce(value, '')) <> '' END) AS filled \
           FROM wf.wfd_env_var \
          WHERE project_id = $1 AND wfd_name = $2",
    )
    .bind(project_id)
    .bind(&wfd_name)
    .fetch_all(&s.pool)
    .await
    .map_err(internal_error)?;

    let mut wildcard: HashMap<String, (String, bool)> = HashMap::new();
    let mut per_env: HashMap<Uuid, HashMap<String, (String, bool)>> = HashMap::new();
    for (env_id, key, value, filled) in rows {
        match env_id {
            None => {
                wildcard.insert(key, (value, filled));
            }
            Some(id) => {
                per_env.entry(id).or_default().insert(key, (value, filled));
            }
        }
    }

    let mut problems: Vec<String> = Vec::new();
    for (env_id, env_name) in &envs {
        let own = per_env.get(env_id);
        let effective = |key: &str| -> Option<&(String, bool)> {
            own.and_then(|m| m.get(key)).or_else(|| wildcard.get(key))
        };
        let filled = |key: &str| effective(key).is_some_and(|(_, f)| *f);

        let backend = effective(crate::attachment_store::KEY_BACKEND)
            .filter(|(_, f)| *f)
            .map(|(v, _)| v.clone());
        let Some(backend) = backend else {
            problems.push(format!(
                "{env_name}: $env.{}",
                crate::attachment_store::KEY_BACKEND
            ));
            continue;
        };
        let Some(keys) = crate::attachment_store::required_env_keys(&backend) else {
            problems.push(format!(
                "{env_name}: $env.{} = '{backend}' (local|s3 olmalı)",
                crate::attachment_store::KEY_BACKEND
            ));
            continue;
        };
        for key in keys {
            if !filled(key) {
                problems.push(format!("{env_name}: $env.{key}"));
            }
        }
    }

    if !problems.is_empty() {
        return Err(AppError(
            format!(
                "attachment_storage.missing_env — belge toplayan akış: ek-belge deposu ayarları eksik: {}",
                problems.join(", ")
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    Ok(())
}

/// Taslağı yayın onayına gönderir (tasarım yetkisi yeter). Validator kapısı
/// yayınla AYNIDIR — geçersiz doküman onaya giremez.
#[utoipa::path(post, path = "/draft/{id}/{version}/submit", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn submit_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    assert_attachment_storage_env(&s, id, ver).await?;
    // Token minimal kimlik taşır — gönderenin görünen adı DB'den çözülür.
    let submitted_by: String =
        sqlx::query_scalar("SELECT display_name FROM wf.app_user WHERE user_id = $1")
            .bind(auth.user_id)
            .fetch_optional(&s.pool)
            .await
            .map_err(internal_error)?
            .unwrap_or_else(|| auth.user_id.to_string());
    s.wfd
        .submit_draft(id, ver, &submitted_by)
        .await
        .map(|_| {
            Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "pending_approval" }))
        })
        .map_err(map_wfd_err)
}

#[utoipa::path(post, path = "/draft/{id}/{version}/approve", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn approve_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_approver_on_wfd(&s, &auth, id, ver).await?;
    assert_attachment_storage_env(&s, id, ver).await?;
    s.wfd
        .approve_draft(id, ver)
        .await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "published" })))
        .map_err(map_wfd_err)
}

#[derive(Deserialize, ToSchema)]
struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

#[utoipa::path(post, path = "/draft/{id}/{version}/reject", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")), request_body = RejectBody,
    responses((status = 200, body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn reject_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(b): Json<RejectBody>,
) -> Result<Json<Value>, AppError> {
    require_approver_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .reject_draft(id, ver, b.reason.as_deref())
        .await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "draft" })))
        .map_err(map_wfd_err)
}

#[utoipa::path(delete, path = "/draft/{id}/{version}", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 204, description = "Silindi")),
    security(("bearer_jwt" = [])))]
async fn delete_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .delete_draft(id, ver)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

#[utoipa::path(post, path = "/{id}/{version}/new-draft", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "Yeni taslak wfd_id + version", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn new_draft(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let (wfd_id, version) = s.wfd.new_draft_from(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(
        serde_json::json!({ "wfd_id": wfd_id, "version": version }),
    ))
}

/// Editör layout companion'ı — şema-VALID WFD dokümanından AYRI, opaque JSON (pozisyon +
/// edge path + reject/collapse). GET auth'suz (get_wfd gibi; readonly reload da auth'suz
/// çeker), blob yoksa `null` döner. Editör publish sonrası PUT'lar (design yetkisi gerekli).
#[utoipa::path(get, path = "/{id}/{version}/layout", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "Editör layout blob (null olabilir)", body = serde_json::Value)))]
async fn get_layout(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    let layout = s.wfd.fetch_layout(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(layout.unwrap_or(Value::Null)))
}

#[utoipa::path(put, path = "/{id}/{version}/layout", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")), request_body = serde_json::Value,
    responses((status = 204, description = "Kaydedildi")),
    security(("bearer_jwt" = [])))]
async fn put_layout(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(layout): Json<Value>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    s.wfd
        .save_layout(id, ver, &layout)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

// ── Senaryolar (kaydedilmiş simülasyon koşuları) ─────────────────────────────
//
// Layout ile aynı desende bir sidecar: doküman `additionalProperties:false` ve
// `(wfd_id, version)` immutable olduğundan senaryolar gövdeye giremez.

#[utoipa::path(get, path = "/{id}/{version}/scenarios", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "Senaryo seti (blob yoksa boş set)", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn get_scenarios(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    // Layout'un aksine GET de yetki ister: senaryolar aktör kimlikleri ve iş
    // girdileri taşır.
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let set = s.wfd.fetch_scenarios(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(set.unwrap_or_else(
        || json!({ "scenarios_version": "1", "scenarios": [] }),
    )))
}

#[utoipa::path(put, path = "/{id}/{version}/scenarios", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    request_body = serde_json::Value,
    responses((status = 204, description = "Kaydedildi")),
    security(("bearer_jwt" = [])))]
async fn put_scenarios(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(body): Json<Value>,
) -> Result<StatusCode, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    // Şekli burada doğrula ki bozuk set koşu anına kadar saklanmasın.
    serde_json::from_value::<wf_wfe::scenario::ScenarioSet>(body.clone()).map_err(|e| {
        AppError(
            format!("senaryo seti geçersiz: {e}"),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    })?;
    s.wfd
        .save_scenarios(id, ver, &body)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

#[derive(Deserialize, ToSchema)]
struct RunScenariosBody {
    /// Verilirse BU doküman koşar (editördeki kaydedilmemiş hâl); verilmezse
    /// depodaki `(id, version)` dokümanı.
    #[serde(default)]
    wfd: Option<Value>,
    /// Senaryo/adım aktörü eksikse kullanılacak yedek aktör.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    fallback_actor: Option<wfe_core::types::actor::Actor>,
    /// Yalnız bu yol önekindeki senaryolar koşar (`"Onaylar"` → `"Onaylar/..."` dahil).
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
struct RunScenariosResponse {
    #[schema(value_type = Vec<Object>)]
    results: Vec<wf_wfe::scenario::ScenarioResult>,
}

/// Setten senaryoları yükler, dokümanı çözer ve koşar. `only` verilirse yalnız
/// o id'li senaryo koşar (tek-senaryo ucu bunu kullanır).
///
/// Koşu HİÇBİR ŞEY YAZMAZ: `sim` durumsuzdur, WFE yaratılmaz, WFAH'a iz düşmez.
async fn run_scenarios_inner(
    s: &AppState,
    auth: &AppAuth,
    id: Uuid,
    ver: i32,
    body: RunScenariosBody,
    only: Option<&str>,
) -> Result<Json<RunScenariosResponse>, AppError> {
    require_design_on_wfd(s, auth, id, ver).await?;

    // Doküman: gövdeden ya da depodan. İki yol da AYNI kapıdan geçer.
    let wfd_json = match body.wfd {
        Some(v) => v,
        None => s.wfd.fetch_json_any(id, ver).await.map_err(map_wfd_err)?,
    };
    let wfd = wfe_core::types::wfd_v22::Wfd::from_value_checked(wfd_json.clone())
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    let report = wfe_core::validator::validate(&wfd);
    if !report.is_valid() {
        let summary = report
            .errors
            .iter()
            .map(|e| format!("[{}] {}: {}", e.code, e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(AppError(
            format!("WFD geçersiz: {summary}"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }

    let raw = s.wfd.fetch_scenarios(id, ver).await.map_err(map_wfd_err)?;
    let set: wf_wfe::scenario::ScenarioSet = match raw {
        Some(v) => serde_json::from_value(v).map_err(|e| {
            AppError(
                format!("senaryo seti geçersiz: {e}"),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        })?,
        None => Default::default(),
    };

    let selected: Vec<_> = set
        .scenarios
        .iter()
        .filter(|sc| only.is_none_or(|o| sc.id == o))
        .filter(|sc| {
            body.path_prefix
                .as_ref()
                .is_none_or(|p| sc.path == *p || sc.path.starts_with(&format!("{p}/")))
        })
        .collect();

    if let Some(o) = only {
        if selected.is_empty() {
            return Err(AppError(
                format!("senaryo bulunamadı: {o}"),
                StatusCode::NOT_FOUND,
            ));
        }
    }

    let org = Arc::new(wf_wfe::OrgAdapter::new(s.pool.clone()));
    let runner = wf_wfe::LiveAutoexecRunner::new(Some(s.pool.clone()));
    let mut results = Vec::with_capacity(selected.len());
    for sc in selected {
        // $env senaryo başına çözülür — her senaryo kendi ortamını söyleyebilir.
        let engine = wfe_core::v22::pipeline::Engine {
            org: &*org,
            exec: &runner,
            env: crate::routes::env::resolve_run_env(
                &s.pool,
                Some(auth.orgtnt_id),
                Some(id),
                sc.environment.as_deref(),
            )
            .await?,
        };
        results.push(
            wf_wfe::scenario::run(&engine, &wfd, &wfd_json, sc, body.fallback_actor.as_ref()).await,
        );
    }
    Ok(Json(RunScenariosResponse { results }))
}

#[utoipa::path(post, path = "/{id}/{version}/scenarios/run", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    request_body = RunScenariosBody,
    responses((status = 200, description = "Her senaryo için sonuç", body = RunScenariosResponse)),
    security(("bearer_jwt" = [])))]
async fn run_scenarios(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(body): Json<RunScenariosBody>,
) -> Result<Json<RunScenariosResponse>, AppError> {
    run_scenarios_inner(&s, &auth, id, ver, body, None).await
}

#[utoipa::path(post, path = "/{id}/{version}/scenarios/{sid}/run", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon"),
           ("sid" = String, Path, description = "Senaryo id")),
    request_body = RunScenariosBody,
    responses((status = 200, description = "Tek senaryonun sonucu", body = RunScenariosResponse)),
    security(("bearer_jwt" = [])))]
async fn run_one_scenario(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver, sid)): Path<(Uuid, i32, String)>,
    Json(body): Json<RunScenariosBody>,
) -> Result<Json<RunScenariosResponse>, AppError> {
    run_scenarios_inner(&s, &auth, id, ver, body, Some(&sid)).await
}

#[derive(Deserialize, ToSchema)]
struct UpdateWfdMetaBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(patch, path = "/{id}/{version}/meta", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")), request_body = UpdateWfdMetaBody,
    responses((status = 200, description = "Güncel WFD meta listesi", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn update_wfd_meta(
    State(s): State<AppState>,
    auth: AppAuth,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(body): Json<UpdateWfdMetaBody>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    require_design_on_wfd(&s, &auth, id, ver).await?;
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if body.name.is_some() && name.is_none() {
        return Err(AppError(
            "Workflow adı boş olamaz".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let description = body.description.as_deref();
    wf_wfd::repo::update_group_metadata(&s.pool, id, ver, name, description)
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

/// Bu published versiyonu kullanan WFE örneklerinin durum dağılımı.
/// `active` = anlık çalışan örnek sayısı.
#[utoipa::path(get, path = "/{id}/{version}/usage", tag = "wfd",
    params(("id" = Uuid, Path, description = "WFD id"), ("version" = i32, Path, description = "Versiyon")),
    responses((status = 200, description = "Bu versiyonu kullanan WFE durum dağılımı", body = serde_json::Value)))]
async fn wfe_usage(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, count(*)::bigint FROM wf.wfe \
         WHERE wfd_id = $1 AND wfd_version = $2 GROUP BY status",
    )
    .bind(id)
    .bind(ver)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let (mut active, mut terminal, mut error, mut terminated) = (0i64, 0i64, 0i64, 0i64);
    for (status, n) in rows {
        match status.as_str() {
            "active" => active = n,
            "terminal" => terminal = n,
            "error" => error = n,
            "terminated" => terminated = n,
            _ => {}
        }
    }
    Ok(Json(serde_json::json!({
        "active": active,
        "terminal": terminal,
        "error": error,
        "terminated": terminated,
        "total": active + terminal + error + terminated,
    })))
}

/// Tenant genelinde wfd_id başına anlık aktif WFE sayısı — dashboard özeti için
/// tek istekte tüm sayımları döner (satır başına /usage çağırmaya gerek kalmaz).
#[utoipa::path(get, path = "/usage-summary", tag = "wfd", params(ListQuery),
    responses((status = 200, body = serde_json::Value)))]
async fn usage_summary(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let rows = load_usage_summary(&s.pool, q.orgtnt_id).await?;

    let arr: Vec<Value> = rows
        .into_iter()
        .map(|(wfd_id, active)| serde_json::json!({ "wfd_id": wfd_id, "active": active }))
        .collect();
    Ok(Json(serde_json::json!(arr)))
}

/// Tenant genelinde WFE execution durum dağılımı (dashboard grafiği için).
#[utoipa::path(get, path = "/execution-stats", tag = "wfd", params(ListQuery),
    responses((status = 200, body = ExecutionStatsRow)))]
async fn execution_stats(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let stats = load_execution_stats(&s.pool, q.orgtnt_id).await?;
    Ok(Json(serde_json::json!(stats)))
}

#[derive(Default, serde::Serialize, ToSchema)]
struct ExecutionStatsRow {
    active: i64,
    terminal: i64,
    error: i64,
    /// SLA ihlali sonlanması (2026-07-16) — `terminal`/`error`'dan AYRI bucket.
    terminated: i64,
    total: i64,
}

async fn load_usage_summary(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
) -> Result<Vec<(Uuid, i64)>, AppError> {
    sqlx::query_as(
        "SELECT wfd_id, count(*)::bigint FROM wf.wfe \
         WHERE orgtnt_id = $1 AND status = 'active' GROUP BY wfd_id",
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)
}

async fn load_execution_stats(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
) -> Result<ExecutionStatsRow, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, count(*)::bigint FROM wf.wfe WHERE orgtnt_id = $1 GROUP BY status",
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let (mut active, mut terminal, mut error, mut terminated) = (0i64, 0i64, 0i64, 0i64);
    for (status, n) in rows {
        match status.as_str() {
            "active" => active = n,
            "terminal" => terminal = n,
            "error" => error = n,
            "terminated" => terminated = n,
            _ => {}
        }
    }
    Ok(ExecutionStatsRow {
        active,
        terminal,
        error,
        terminated,
        total: active + terminal + error + terminated,
    })
}

#[derive(serde::Serialize, ToSchema)]
struct UnitWorkloadRow {
    orgu_id: Uuid,
    orgu_name: String,
    active: i64,
    unclaimed: i64,
}

/// Tenant genelinde current_c_a'ya göre birim başına anlık iş yükü — en çok
/// işi olan org unit'ler (dashboard insight).
#[utoipa::path(get, path = "/unit-workload", tag = "wfd", params(ListQuery),
    responses((status = 200, body = Vec<UnitWorkloadRow>)))]
async fn unit_workload(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<UnitWorkloadRow>>, AppError> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    load_unit_workload(&s.pool, q.orgtnt_id, limit)
        .await
        .map(Json)
}

async fn load_unit_workload(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<UnitWorkloadRow>, AppError> {
    let rows: Vec<(Uuid, String, i64, i64)> = sqlx::query_as(
        "SELECT ou.orgu_id, ou.name,
                count(*)::bigint AS active,
                count(*) FILTER (WHERE w.claimed_by IS NULL)::bigint AS unclaimed
         FROM wf.wfe w
         CROSS JOIN LATERAL jsonb_array_elements(w.current_c_a) AS ca(elem)
         JOIN org.orgu ou ON ou.orgu_id = (ca.elem->>'orgu_id')::uuid
         WHERE w.orgtnt_id = $1 AND w.status = 'active'
         GROUP BY ou.orgu_id, ou.name
         ORDER BY active DESC
         LIMIT $2",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    Ok(rows
        .into_iter()
        .map(|(orgu_id, orgu_name, active, unclaimed)| UnitWorkloadRow {
            orgu_id,
            orgu_name,
            active,
            unclaimed,
        })
        .collect())
}

#[derive(serde::Serialize, ToSchema)]
struct NodeLoadRow {
    wfd_id: Uuid,
    node: String,
    active: i64,
}

/// Tenant genelinde (workflow, node) çifti başına aktif WFE sayısı — hangi
/// duraklarda yığılma var (dashboard insight).
#[utoipa::path(get, path = "/node-load", tag = "wfd", params(ListQuery),
    responses((status = 200, body = Vec<NodeLoadRow>)))]
async fn node_load(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<NodeLoadRow>>, AppError> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    load_node_load(&s.pool, q.orgtnt_id, limit).await.map(Json)
}

async fn load_node_load(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<NodeLoadRow>, AppError> {
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT wfd_id, current_node, count(*)::bigint AS active
         FROM wf.wfe
         WHERE orgtnt_id = $1 AND status = 'active' AND current_node IS NOT NULL
         GROUP BY wfd_id, current_node
         ORDER BY active DESC
         LIMIT $2",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    Ok(rows
        .into_iter()
        .map(|(wfd_id, node, active)| NodeLoadRow {
            wfd_id,
            node,
            active,
        })
        .collect())
}

#[derive(serde::Serialize, ToSchema)]
struct AgingRow {
    wfe_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    node: String,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// En uzun süredir güncellenmeyen aktif execution'lar — hareketsiz/"stuck"
/// iş akışları (dashboard insight).
#[utoipa::path(get, path = "/aging-executions", tag = "wfd", params(ListQuery),
    responses((status = 200, body = Vec<AgingRow>)))]
async fn aging_executions(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<AgingRow>>, AppError> {
    let limit = q.limit.unwrap_or(8).clamp(1, 30);
    load_aging_executions(&s.pool, q.orgtnt_id, limit)
        .await
        .map(Json)
}

async fn load_aging_executions(
    pool: &sqlx::PgPool,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<AgingRow>, AppError> {
    let rows: Vec<(Uuid, Uuid, i32, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT wfe_id, wfd_id, wfd_version, current_node, updated_at
         FROM wf.wfe
         WHERE orgtnt_id = $1 AND status = 'active' AND current_node IS NOT NULL
         ORDER BY updated_at ASC
         LIMIT $2",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    Ok(rows
        .into_iter()
        .map(|(wfe_id, wfd_id, wfd_version, node, updated_at)| AgingRow {
            wfe_id,
            wfd_id,
            wfd_version,
            node,
            updated_at,
        })
        .collect())
}

#[derive(serde::Serialize, ToSchema)]
struct EscalationRow {
    wfe_id: Uuid,
    wfd_id: Uuid,
    wfd_version: i32,
    node: String,
    current_c_a: Value,
    claimed_by: Option<Value>,
    step_idx: usize,
    deadline: chrono::DateTime<chrono::Utc>,
    overdue: bool,
}

/// Yaklaşan/geciken escalation deadline'ları — en yakın vadeden başlayarak
/// (dashboard insight). Node giriş anı gerçek WFAH kaydından hesaplanır
/// (yaklaşık değer değil); bkz. `Engine::next_escalation`.
#[utoipa::path(get, path = "/escalation-forecast", tag = "wfd", params(ListQuery),
    responses((status = 200, body = Vec<EscalationRow>)))]
async fn escalation_forecast(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<EscalationRow>>, AppError> {
    let limit = q.limit.unwrap_or(8).clamp(1, 30);
    load_escalation_forecast(&s, q.orgtnt_id, limit)
        .await
        .map(Json)
}

async fn load_escalation_forecast(
    s: &AppState,
    orgtnt_id: Uuid,
    limit: i64,
) -> Result<Vec<EscalationRow>, AppError> {
    let now = chrono::Utc::now();
    // Escalation hesabı WFE başına wfah+wfd yükler; taban havuzu makul bir
    // tavanla sınırlanır (en son güncellenen 300 aktif WFE).
    let candidates = wf_wfe::repo::wfe::list_active_by_tenant(&s.pool, orgtnt_id, 300)
        .await
        .map_err(internal_error)?;

    let mut out = Vec::new();
    for row in candidates {
        let forecast = match s.executor.escalation_forecast(row.wfe_id, now).await {
            Ok(f) => f,
            // Bozuk/eksik WFD dokümanı olan tek bir WFE tüm insight'ı düşürmesin.
            Err(_) => continue,
        };
        if let Some(f) = forecast {
            out.push(EscalationRow {
                wfe_id: row.wfe_id,
                wfd_id: row.wfd_id,
                wfd_version: row.wfd_version,
                node: row.current_node.unwrap_or_default(),
                current_c_a: row.current_c_a,
                claimed_by: row.claimed_by,
                step_idx: f.step_idx,
                deadline: f.deadline,
                overdue: f.overdue,
            });
        }
    }
    out.sort_by_key(|r| r.deadline);
    out.truncate(limit as usize);
    Ok(out)
}

#[derive(Default, serde::Serialize, ToSchema)]
struct OrgSummaryRow {
    tree_count: i64,
    unit_count: i64,
    user_count: i64,
    role_count: i64,
    leaf_count: i64,
    max_depth: i64,
    branch_count: i64,
    region_count: i64,
}

#[derive(serde::Serialize, ToSchema)]
struct DashboardSummary {
    #[schema(value_type = Vec<Object>)]
    wfds: Vec<wf_wfd::models::WfdMeta>,
    active_by_wfd: HashMap<String, i64>,
    exec_stats: ExecutionStatsRow,
    units: Vec<UnitWorkloadRow>,
    node_load_rows: Vec<NodeLoadRow>,
    aging: Vec<AgingRow>,
    escalations: Vec<EscalationRow>,
    org_summary: OrgSummaryRow,
}

#[utoipa::path(get, path = "/dashboard-summary", tag = "wfd", params(ListQuery),
    responses((status = 200, body = DashboardSummary)))]
async fn dashboard_summary(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<DashboardSummary>, AppError> {
    let wfds = wf_wfd::repo::list(&s.pool, q.orgtnt_id, q.project_id, 1_000, 0)
        .await
        .map_err(map_wfd_err)?;
    let active_by_wfd = load_usage_summary(&s.pool, q.orgtnt_id)
        .await?
        .into_iter()
        .map(|(wfd_id, active)| (wfd_id.to_string(), active))
        .collect();

    Ok(Json(DashboardSummary {
        wfds,
        active_by_wfd,
        exec_stats: load_execution_stats(&s.pool, q.orgtnt_id)
            .await
            .unwrap_or_default(),
        units: load_unit_workload(&s.pool, q.orgtnt_id, 10)
            .await
            .unwrap_or_default(),
        node_load_rows: load_node_load(&s.pool, q.orgtnt_id, 10)
            .await
            .unwrap_or_default(),
        aging: load_aging_executions(&s.pool, q.orgtnt_id, 8)
            .await
            .unwrap_or_default(),
        escalations: load_escalation_forecast(&s, q.orgtnt_id, 8)
            .await
            .unwrap_or_default(),
        org_summary: load_org_summary(&s.pool, q.orgtnt_id)
            .await
            .unwrap_or_default(),
    }))
}

async fn load_org_summary(pool: &sqlx::PgPool, orgtnt_id: Uuid) -> Result<OrgSummaryRow, AppError> {
    let (
        tree_count,
        unit_count,
        user_count,
        role_count,
        leaf_count,
        max_depth,
        branch_count,
        region_count,
    ): (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "WITH primary_tree AS (
             SELECT orgt_id
             FROM org.orgt
             WHERE orgtnt_id = $1
             ORDER BY name
             LIMIT 1
         ),
         tree_units AS (
             SELECT oo.orgu_id, oo.path, o.orgu_type
             FROM org.orgt_orgu oo
             JOIN org.orgu o ON o.orgu_id = oo.orgu_id
             JOIN primary_tree pt ON pt.orgt_id = oo.orgt_id
             WHERE o.is_active = true AND oo.is_active = true
         )
         SELECT
             (SELECT count(*)::bigint FROM org.orgt WHERE orgtnt_id = $1) AS tree_count,
             (SELECT count(*)::bigint FROM tree_units) AS unit_count,
             (SELECT count(*)::bigint FROM org.u WHERE orgtnt_id = $1 AND is_active = true) AS user_count,
             (SELECT count(*)::bigint FROM org.r WHERE orgtnt_id = $1 AND is_active = true) AS role_count,
             (SELECT count(*)::bigint
              FROM tree_units tu
              WHERE NOT EXISTS (
                  SELECT 1 FROM tree_units child
                  WHERE child.path <@ tu.path AND child.path <> tu.path
              )) AS leaf_count,
             COALESCE((SELECT max(nlevel(path))::bigint FROM tree_units), 0) AS max_depth,
             (SELECT count(*)::bigint
              FROM tree_units
              WHERE lower(orgu_type->>'type') IN ('sube', 'branch')) AS branch_count,
             (SELECT count(*)::bigint
              FROM tree_units
              WHERE lower(orgu_type->>'type') IN ('bolge', 'region')) AS region_count",
    )
    .bind(orgtnt_id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(OrgSummaryRow {
        tree_count,
        unit_count,
        user_count,
        role_count,
        leaf_count,
        max_depth,
        branch_count,
        region_count,
    })
}

fn internal_error(e: impl std::fmt::Display) -> AppError {
    AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}

/// WfdError → HTTP kodu eşlemesi.
fn map_wfd_err(e: wf_wfd::error::WfdError) -> AppError {
    use wf_wfd::error::WfdError as E;
    let code = match e {
        E::NotFound(_) => StatusCode::NOT_FOUND,
        E::Conflict(_) => StatusCode::CONFLICT,
        E::InvalidJson(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    AppError(e.to_string(), code)
}
