//! Context ŞEMASINA göre DEĞER denetimi — saf, I/O'suz.
//!
//! # Neden
//!
//! Motor bugüne kadar tipi yalnız TASARIM zamanında denetliyordu
//! (`validator::check_effect_value_types`, `expr_types`); çalışma anında gelen değer
//! denetimsizdi. `validate_action_input` yalnız "bildirilen yol var mı, `required`
//! dolu mu" soruyordu → sayı beklenen bir yola gönderilen metin reddedilmiyor, ctx'e
//! AYNEN yazılıyor ve etkisi ancak karar anında (sayısal bir `when` çalışırken)
//! çıkıyordu. Kanıt: `wf_wfe::tests::scenario::wrong_type_input_passes_the_input_gate`.
//!
//! Kural artık MOTORDA: engine hangi değeri kabul edeceğini kesin bilir ve reddi
//! sebebiyle birlikte söyler — istemcinin (editör, portal, üçüncü parti UI) kendi
//! kuralını icat etmesi GEREKMEZ. Editör/portal yalnız aynı cevabı önden verir.
//!
//! # Adlandırılmış tip: `format`
//!
//! Bir alan tipini KENDİ içinde ya da `$defs`'teki bir tanıma **ADLA** bağlı olarak
//! taşır:
//!
//! ```json
//! { "$defs": { "Tarih": { "type": "string", "pattern": "^[0-9]{14}$" } },
//!   "properties": { "basvuru_tarihi": { "format": "Tarih" } } }
//! ```
//!
//! `format` bu belgede STANDART JSON Schema anlamında DEĞİL, `#/$defs/<Ad>` kısayolu
//! olarak okunur (karar: 2026-08-19). Sebep: standart `format` yalnız bir İSİMDİR,
//! kuralı doğrulayıcı kütüphanenin tablosunda durur — o tabloyu kabul etmek motorun
//! sözleşmesini crate sürümüne bağlamak olurdu. `$defs` tanımı ise kuralı BELGEDE
//! taşır (`type`, `pattern`, sınırlar) ve motor onu okuyabilir.
//!
//! Eski `{"$ref": "#/$defs/Ad"}` sözdizimi 2026-08-19'da TAMAMEN KALDIRILDI —
//! okuyucusu da yok. Gerekçe: ürün henüz production'da değil, mevcut tüm WFD/WFE'ler
//! test verisi ve production öncesi sıfırlanacak. "Yayınlanmış belge yeniden yazılamaz,
//! okuyucu kalır" kuralı production'dan SONRA geçerlidir; şu an geriye uyum kodu saf
//! borçtur. `$ref` artık şemada da yok (`schema.json` `contextSchemaNode`) ve validator
//! ayrıca `context_ref_removed` ile reddeder.
//!
//! # `null` HER tipte geçerlidir
//!
//! WOR-70b gereği gönderilmeyen `optional` girdi ctx'e KOŞULSUZ `null` yazar (golden
//! fixture `internal_notes`). `type: "string"` null'ı reddederse o fixture ilk koşuda
//! patlar. Bu yüzden denetim "null YA DA bildirilen tip"tir; `required` girdinin null
//! olamaması AYRI bir kuraldır ve `validate_action_input`ta durur.

use serde_json::{Map, Value};
use std::fmt;

/// Tanım zinciri sınırı — `format: A` → `format: B` → … Döngü bu sınırla kesilir
/// (tasarım zamanı ayrıca `context_format_cycle` ile reddeder).
const MAX_DEREF_HOPS: usize = 16;

/// Bir değerin şema ihlali. `path` context yolu (`"credit_info.amount"`),
/// `expected` şemanın istediği, `got` gelenin özeti.
///
/// `Serialize`: hem HTTP hata gövdesinde alan bazında taşınır (`422` + `items[]`) hem de
/// `WfeView.ctx_violations`ta bildirilir — istemci hata METNİNİ ayrıştırmaz. `message`
/// alanı `Display` çıktısıdır: aynı bilgiyi tek satır hâlinde de verir ki basit
/// istemciler alanları birleştirmek zorunda kalmasın.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Violation {
    pub path: String,
    pub expected: String,
    pub got: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' alanı {} olmalı, gelen: {}",
            self.path, self.expected, self.got
        )
    }
}

/// Bir yolun şemadaki karşılığı.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Şema bu yolu tanımlıyor — düğüm TAM ÇÖZÜLMÜŞ hâlde (adlandırılmış tip inline).
    Found(Value),
    /// Şemada `properties` var ama bu ad yok — belgede tanımlı OLMAYAN alan.
    Missing,
    /// Şema bu derinliği kısıtlamıyor (`properties` yok, çözülemeyen tanım adı, …).
    /// Denetim yapılmaz — motor bilmediği şeyi reddetmez.
    Opaque,
}

fn as_object(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()
}

/// Bir düğümün taşıdığı tanım adı — TEK sözdizimi `format: "Ad"`.
fn def_name(node: &Map<String, Value>) -> Option<&str> {
    node.get("format").and_then(Value::as_str)
}

/// `format` taşıyan düğümü tanımıyla birleştirir. Kullanım yerindeki KARDEŞ anahtarlar
/// kazanır (`description`, `x-visibility` üzerine yazılabilir); tanımın kendisi de
/// tanım adı taşıyabilir (zincir).
///
/// Tanım bulunamazsa `None`: motor çözemediği adı ZORLAMAZ (tasarım zamanı kapısı
/// `context_format_unknown` bunu yayına bırakmaz), o yol `Opaque` sayılır.
fn deref(defs: Option<&Map<String, Value>>, node: &Value) -> Option<Value> {
    let mut current = node.clone();
    for _ in 0..MAX_DEREF_HOPS {
        let Some(map) = as_object(&current) else {
            return Some(current);
        };
        let Some(name) = def_name(map) else {
            return Some(current);
        };
        let def = defs?.get(name)?;
        let mut merged = as_object(def)?.clone();
        for (k, v) in map {
            if k == "format" {
                continue;
            }
            merged.insert(k.clone(), v.clone());
        }
        current = Value::Object(merged);
    }
    None // zincir çok uzun → döngü kabul edilir
}

/// Bir düğümün İÇİNDEKİ tüm tanım adlarını çözerek tamamen inline bir şema üretir.
/// jsonschema'ya verilen şey budur: `$defs` enjekte etmek gerekmez, `format`
/// anahtarı da geride kalmaz (standart format assertion'ı yanlışlıkla devreye
/// girmesin — anlamını biz eziyoruz).
fn inline(defs: Option<&Map<String, Value>>, node: &Value, depth: usize) -> Option<Value> {
    if depth > MAX_DEREF_HOPS {
        return None;
    }
    let resolved = deref(defs, node)?;
    let Some(map) = as_object(&resolved) else {
        return Some(resolved);
    };
    let mut out = Map::new();
    for (k, v) in map {
        let child = match k.as_str() {
            "properties" | "patternProperties" => {
                let Some(props) = as_object(v) else { continue };
                let mut inlined = Map::new();
                for (name, sub) in props {
                    inlined.insert(name.clone(), inline(defs, sub, depth + 1)?);
                }
                Value::Object(inlined)
            }
            "items" | "contains" | "additionalProperties" | "not" | "if" | "then" | "else" => {
                match v {
                    Value::Array(arr) => Value::Array(
                        arr.iter()
                            .map(|x| inline(defs, x, depth + 1))
                            .collect::<Option<Vec<_>>>()?,
                    ),
                    Value::Object(_) => inline(defs, v, depth + 1)?,
                    other => other.clone(),
                }
            }
            "oneOf" | "anyOf" | "allOf" => {
                let Some(arr) = v.as_array() else { continue };
                Value::Array(
                    arr.iter()
                        .map(|x| inline(defs, x, depth + 1))
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            // Tanım adı çözüldü; kalıntı bırakılmaz (bkz. fonksiyon dokümanı).
            "format" => continue,
            _ => v.clone(),
        };
        out.insert(k.clone(), child);
    }
    Some(Value::Object(out))
}

/// Context şemasında bir yolun (`"a.b.c"`) karşılığını çözer.
///
/// `context` kök şema düğümüdür (`wfd.context`); `$defs` oradan okunur.
pub fn field_schema(context: &Value, path: &str) -> Resolved {
    let defs = context.get("$defs").and_then(Value::as_object);
    let mut current = match deref(defs, context) {
        Some(v) => v,
        None => return Resolved::Opaque,
    };
    for segment in path.split('.') {
        let Some(props) = current.get("properties").and_then(Value::as_object) else {
            return Resolved::Opaque;
        };
        let Some(next) = props.get(segment) else {
            return Resolved::Missing;
        };
        current = match deref(defs, next) {
            Some(v) => v,
            None => return Resolved::Opaque,
        };
    }
    match inline(defs, &current, 0) {
        Some(schema) => Resolved::Found(schema),
        None => Resolved::Opaque,
    }
}

/// Şemanın istediğinin okunur özeti (`"number"`, `"string (enum: a, b)"`).
fn expected_summary(schema: &Value) -> String {
    let ty = match schema.get("type") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|"),
        _ => String::new(),
    };
    let mut parts: Vec<String> = Vec::new();
    if !ty.is_empty() {
        parts.push(ty);
    }
    if let Some(Value::Array(en)) = schema.get("enum") {
        let list = en
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("seçenekler: {list}"));
    }
    if let Some(p) = schema.get("pattern").and_then(Value::as_str) {
        parts.push(format!("desen: {p}"));
    }
    for (key, label) in [
        ("minimum", "en az"),
        ("maximum", "en çok"),
        ("minLength", "en az uzunluk"),
        ("maxLength", "en çok uzunluk"),
    ] {
        if let Some(v) = schema.get(key) {
            parts.push(format!("{label} {v}"));
        }
    }
    if parts.is_empty() {
        "şemadaki kurala uygun".into()
    } else {
        parts.join(" · ")
    }
}

/// Gelen değerin okunur özeti — tip + kısaltılmış değer.
fn got_summary(value: &Value) -> String {
    let ty = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let mut text = value.to_string();
    if text.chars().count() > 60 {
        text = text.chars().take(57).collect::<String>() + "…";
    }
    format!("{ty} {text}")
}

/// TEK bir yolun değerini denetler.
///
/// `null` DAİMA geçer (bkz. modül dokümanı). Şema o yolu tanımlamıyorsa (`Missing`/
/// `Opaque`) ihlal ÜRETİLMEZ: "bildirilmemiş yol" ayrı bir kuralın işidir
/// (`validate_action_input`), motor burada yalnız TİP konuşur.
pub fn validate_at(context: &Value, path: &str, value: &Value) -> Vec<Violation> {
    if value.is_null() {
        return Vec::new();
    }
    let Resolved::Found(schema) = field_schema(context, path) else {
        return Vec::new();
    };
    violations_against(&schema, path, value)
}

/// Şemayı HER SEVİYEDE null kabul edecek hâle getirir.
///
/// `null` yalnız kökte değil İÇ alanlarda da geçerli olmak zorundadır: WOR-70b
/// gönderilmeyen `optional` girdiyi ctx'e koşulsuz `null` yazar ve bu bir ALT alan da
/// olabilir (`applicant.income`). Yalnız kökü muaf tutmak, golden fixture'ın
/// `required: ["applicant"]` + `applicant.income = null` vakasını reddediyordu.
///
/// Yöntem: `type`e `"null"` eklenir. JSON Schema'da tip-özel anahtarlar (`minimum`,
/// `pattern`, `minItems`…) null değerde HİÇ koşmaz, dolayısıyla başka bir gevşetme
/// gerekmez. `enum`/`const` ise değer listesidir; onlara `null` EKLENİR.
fn relax_nulls(node: &Value) -> Value {
    let Some(map) = as_object(node) else {
        return node.clone();
    };
    let mut out = Map::new();
    for (k, v) in map {
        let child = match k.as_str() {
            "type" => match v {
                Value::String(t) => Value::Array(vec![Value::String(t.clone()), "null".into()]),
                Value::Array(arr) if !arr.iter().any(|x| x == "null") => {
                    let mut list = arr.clone();
                    list.push("null".into());
                    Value::Array(list)
                }
                other => other.clone(),
            },
            "enum" => match v {
                Value::Array(arr) if !arr.iter().any(Value::is_null) => {
                    let mut list = arr.clone();
                    list.push(Value::Null);
                    Value::Array(list)
                }
                other => other.clone(),
            },
            // `const` bir değer listesine çevrilir; `enum` ile aynı anlam, null da kabul.
            "const" => {
                out.insert("enum".into(), Value::Array(vec![v.clone(), Value::Null]));
                continue;
            }
            "properties" | "patternProperties" => {
                let Some(props) = as_object(v) else { continue };
                let mut relaxed = Map::new();
                for (name, sub) in props {
                    relaxed.insert(name.clone(), relax_nulls(sub));
                }
                Value::Object(relaxed)
            }
            "items" | "contains" | "additionalProperties" | "then" | "else" => match v {
                Value::Array(arr) => Value::Array(arr.iter().map(relax_nulls).collect()),
                Value::Object(_) => relax_nulls(v),
                other => other.clone(),
            },
            "oneOf" | "anyOf" | "allOf" => match v {
                Value::Array(arr) => Value::Array(arr.iter().map(relax_nulls).collect()),
                other => other.clone(),
            },
            // `not`/`if` gevşetilmez: olumsuzlama/koşul dalını gevşetmek anlamı TERSİNE
            // çevirebilir. Bu anahtarlar context şemasında pratikte kullanılmıyor.
            _ => v.clone(),
        };
        out.insert(k.clone(), child);
    }
    Value::Object(out)
}

/// `"/a/0/b"` biçimindeki instance yolunu şemada takip eder — ihlalin KENDİ alt şeması
/// bulunabilsin (mesajda "ne bekleniyordu" doğru düğümden okunsun).
fn schema_at<'a>(schema: &'a Value, instance_path: &str) -> &'a Value {
    let mut current = schema;
    for segment in instance_path.split('/').filter(|s| !s.is_empty()) {
        let next = if segment.chars().all(|c| c.is_ascii_digit()) {
            current.get("items")
        } else {
            current.get("properties").and_then(|p| p.get(segment))
        };
        match next {
            Some(node) => current = node,
            None => return current,
        }
    }
    current
}

/// İnline edilmiş bir şemaya karşı doğrulama. Şema derlenemezse (bozuk belge)
/// ihlal üretilmez — belge kapısı (`schema::validate_document`) onun yeri.
fn violations_against(schema: &Value, path: &str, value: &Value) -> Vec<Violation> {
    let relaxed = relax_nulls(schema);
    let Ok(validator) = jsonschema::validator_for(&relaxed) else {
        return Vec::new();
    };
    let mut out: Vec<Violation> = Vec::new();
    for err in validator.iter_errors(value) {
        // Hatanın işaret ettiği ALT yol da yola eklenir: `properties` taşıyan bir
        // alanda ihlal içteki bir alanda olabilir.
        let inner = err.instance_path.to_string();
        let full = if inner.is_empty() {
            path.to_string()
        } else {
            format!("{path}{}", inner.replace('/', "."))
        };
        // Mesaj İHLALİN KENDİ düğümünden kurulur: kök şema/değeri raporlamak
        // ("`applicant.income` object olmalı, gelen: object {...}") yanıltıcıydı.
        out.push(Violation {
            path: full,
            expected: expected_summary(schema_at(schema, &inner)),
            got: got_summary(err.instance.as_ref()),
        });
    }
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Bir aksiyonun BİLDİRDİĞİ yolların değerlerini denetler — girdi kapısının çekirdeği.
/// `paths` = `input.required ∪ input.optional`; girdide olmayan yol atlanır (varlık
/// denetimi `validate_action_input`ın işi).
pub fn validate_input(context: &Value, paths: &[String], input: &Value) -> Vec<Violation> {
    let mut out = Vec::new();
    for path in paths {
        let Some(value) = value_at(input, path) else {
            continue;
        };
        out.extend(validate_at(context, path, value));
    }
    out
}

/// `"a.b"` yolundaki değeri okur (yok → `None`).
fn value_at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// TÜM dynctx'i denetler — rapor aracı ve "bozuk ctx" kapısı için.
///
/// Kök seviyedeki her alan için şema çözülür ve ALT AĞAÇ birlikte doğrulanır
/// (`properties` taşıyan tanım iç alanları kendisi denetler). Şemada olmayan kök
/// alanlar `undeclared` listesine düşer: bugün effects bildirilmemiş bir ctx yoluna
/// yazabiliyor (`check_effect_value_types` şemasız hedefi atlıyor) — bu bir İHLAL
/// değil, ölçülmesi gereken bir olgu.
pub fn validate_dynctx(context: &Value, dynctx: &Value) -> DynctxReport {
    let mut report = DynctxReport::default();
    let Some(fields) = dynctx.as_object() else {
        return report;
    };
    for (name, value) in fields {
        if value.is_null() {
            continue;
        }
        match field_schema(context, name) {
            Resolved::Found(schema) => {
                report
                    .violations
                    .extend(violations_against(&schema, name, value));
            }
            Resolved::Missing => report.undeclared.push(name.clone()),
            Resolved::Opaque => {}
        }
    }
    report
}

/// Bir commit'in YAZDIĞI yolların denetimi — "kapı B".
///
/// `before`/`after` karşılaştırılır ve YALNIZ DEĞİŞEN kök alanlar doğrulanır. Neden
/// değişen alanlar:
///   · Her effect kaynağını (istek girdisi · `$env` · autoexec sonucu · WFC dönüşü ·
///     sistem · sabit) TEK noktada yakalar; dokuz `apply_effects` çağrısını tek tek
///     sarmak gerekmez.
///   · Enforcement'tan ÖNCE bozulmuş eski veri akışı DURDURMAZ: o alan bu geçişte
///     yazılmadıysa hiç sorulmaz. Bozuk veriyle iş yapmayı engellemek ayrı bir kapının
///     ("kapı C", `validate_dynctx`) işidir.
pub fn validate_written(context: &Value, before: &Value, after: &Value) -> Vec<Violation> {
    let Some(fields) = after.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, value) in fields {
        if value.is_null() {
            continue; // null her tipte geçerli (bkz. modül dokümanı)
        }
        if before.get(name) == Some(value) {
            continue; // bu geçişte yazılmadı
        }
        if let Resolved::Found(schema) = field_schema(context, name) {
            out.extend(violations_against(&schema, name, value));
        }
    }
    out
}

#[derive(Debug, Default, Clone)]
pub struct DynctxReport {
    pub violations: Vec<Violation>,
    /// Context şemasında karşılığı olmayan ctx alanları (ihlal değil, olgu).
    pub undeclared: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Value {
        json!({
            "type": "object",
            "$defs": {
                "Tarih": { "type": "string", "pattern": "^[0-9]{14}$" },
                "Para": { "type": "number", "minimum": 0 },
                "Musteri": { "type": "object", "properties": {
                    "ad": { "type": "string" },
                    "bakiye": { "format": "Para" }
                }}
            },
            "properties": {
                "tutar": { "format": "Para" },
                "basvuru_tarihi": { "format": "Tarih", "description": "kullanım yerinde ezilen açıklama" },
                "musteri": { "format": "Musteri" },
                "karar": { "type": "string", "enum": ["onay", "ret"] },
                "adet": { "type": "integer" },
                "aktif": { "type": "boolean" },
                "serbest": { "type": "object" },
                "etiketler": { "type": "array", "items": { "type": "string" } }
            }
        })
    }

    #[test]
    fn format_resolves_to_a_named_definition() {
        let Resolved::Found(schema) = field_schema(&ctx(), "tutar") else {
            panic!("çözülmedi");
        };
        assert_eq!(schema["type"], "number");
        assert_eq!(schema["minimum"], 0);
        // `format` kalıntı bırakmaz — standart format assertion'ı devreye girmesin.
        assert!(schema.get("format").is_none());
    }

    #[test]
    fn use_site_siblings_win_over_the_definition() {
        let Resolved::Found(schema) = field_schema(&ctx(), "basvuru_tarihi") else {
            panic!("çözülmedi");
        };
        assert_eq!(schema["pattern"], "^[0-9]{14}$");
        assert_eq!(schema["description"], "kullanım yerinde ezilen açıklama");
    }

    /// Eski `$ref` sözdizimi ÇÖZÜLMEZ (okuyucu 2026-08-19'da kaldırıldı): düğüm
    /// tanıma bağlanmaz, olduğu gibi kalır — şema kapısı onu ayrıca reddeder.
    #[test]
    fn legacy_ref_is_not_resolved_anymore() {
        let c = json!({
            "type": "object",
            "$defs": { "Tarih": { "type": "string" } },
            "properties": { "eski": { "$ref": "#/$defs/Tarih" } }
        });
        let Resolved::Found(schema) = field_schema(&c, "eski") else {
            panic!("düğüm bulunmalı (çözülmemiş olsa da)");
        };
        assert!(schema.get("type").is_none(), "tanıma bağlanmamalı: {schema}");
    }

    #[test]
    fn nested_format_inside_a_definition_resolves() {
        let Resolved::Found(schema) = field_schema(&ctx(), "musteri") else {
            panic!("çözülmedi");
        };
        assert_eq!(schema["properties"]["bakiye"]["type"], "number");
        // İç yol da doğrudan çözülebilir.
        let Resolved::Found(inner) = field_schema(&ctx(), "musteri.bakiye") else {
            panic!("iç yol çözülmedi");
        };
        assert_eq!(inner["type"], "number");
    }

    #[test]
    fn unknown_definition_name_is_opaque_not_a_violation() {
        let c = json!({ "type": "object", "properties": { "x": { "format": "Yok" } } });
        assert_eq!(field_schema(&c, "x"), Resolved::Opaque);
        assert!(validate_at(&c, "x", &json!("her şey")).is_empty());
    }

    #[test]
    fn cyclic_definitions_do_not_hang() {
        let c = json!({
            "type": "object",
            "$defs": { "A": { "format": "B" }, "B": { "format": "A" } },
            "properties": { "x": { "format": "A" } }
        });
        assert_eq!(field_schema(&c, "x"), Resolved::Opaque);
    }

    #[test]
    fn missing_path_is_missing_and_deep_path_under_opaque_is_opaque() {
        assert_eq!(field_schema(&ctx(), "yok"), Resolved::Missing);
        // `serbest` şemasız bir obje: altı kısıtlanmamıştır.
        assert_eq!(field_schema(&ctx(), "serbest.her_sey"), Resolved::Opaque);
    }

    // ── değer denetimi ──────────────────────────────────────────────────────

    #[test]
    fn wrong_type_is_a_violation_with_path_expected_got() {
        let v = validate_at(&ctx(), "tutar", &json!("yüz bin"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].path, "tutar");
        assert!(v[0].expected.contains("number"), "{:?}", v[0]);
        assert!(v[0].got.contains("string"), "{:?}", v[0]);
    }

    #[test]
    fn correct_value_passes() {
        assert!(validate_at(&ctx(), "tutar", &json!(1000)).is_empty());
        assert!(validate_at(&ctx(), "basvuru_tarihi", &json!("20260819103000")).is_empty());
        assert!(validate_at(&ctx(), "karar", &json!("onay")).is_empty());
        assert!(validate_at(&ctx(), "etiketler", &json!(["a", "b"])).is_empty());
    }

    /// `null` HER tipte geçer — WOR-70b gönderilmeyen `optional` ctx'e null yazar.
    #[test]
    fn null_is_always_accepted() {
        assert!(validate_at(&ctx(), "tutar", &Value::Null).is_empty());
        assert!(validate_at(&ctx(), "karar", &Value::Null).is_empty());
        assert!(validate_at(&ctx(), "musteri", &Value::Null).is_empty());
    }

    /// null toleransı YALNIZ kökte değil, İÇ alanlarda da geçerli olmalı: golden
    /// fixture `required: ["applicant"]` bildirir ve `applicant.income` null gelebilir
    /// (bu vaka ilk uygulamada REDDEDİLİYORDU — `pipeline` testi yakaladı).
    #[test]
    fn null_is_accepted_in_nested_fields_too() {
        let v = validate_at(&ctx(), "musteri", &json!({ "ad": "Ay", "bakiye": null }));
        assert!(v.is_empty(), "{v:?}");
    }

    /// enum ve dizi öğeleri de null kabul eder (aynı gerekçe).
    #[test]
    fn null_passes_enum_and_array_items() {
        assert!(validate_at(&ctx(), "karar", &Value::Null).is_empty());
        assert!(validate_at(&ctx(), "etiketler", &json!(["a", null])).is_empty());
    }

    /// Mesaj İHLALİN KENDİ düğümünden kurulur — kök şemayı/değeri raporlamak
    /// ("`musteri.bakiye` object olmalı, gelen: object {…}") yanıltıcıydı.
    #[test]
    fn nested_violation_message_uses_the_inner_schema_and_value() {
        let v = validate_at(&ctx(), "musteri", &json!({ "ad": "Ay", "bakiye": "çok" }));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].path, "musteri.bakiye");
        assert!(v[0].expected.contains("number"), "{:?}", v[0]);
        assert_eq!(v[0].got, "string \"çok\"", "{:?}", v[0]);
    }

    #[test]
    fn definition_constraints_are_enforced_not_just_the_type() {
        // `Para` minimum 0
        assert_eq!(validate_at(&ctx(), "tutar", &json!(-5)).len(), 1);
        // `Tarih` deseni: 14 rakam
        assert_eq!(
            validate_at(&ctx(), "basvuru_tarihi", &json!("2026-08-19T10:30:00Z")).len(),
            1
        );
        // enum dışı değer
        assert_eq!(validate_at(&ctx(), "karar", &json!("belki")).len(), 1);
        // integer'a ondalık
        assert_eq!(validate_at(&ctx(), "adet", &json!(1.5)).len(), 1);
        // boolean'a metin
        assert_eq!(validate_at(&ctx(), "aktif", &json!("true")).len(), 1);
        // Dizi öğesi tipi — İHLAL ÖĞE BAŞINA raporlanır (yol `etiketler.0`,
        // `etiketler.1`): kullanıcı hangi öğenin bozuk olduğunu görsün.
        let items = validate_at(&ctx(), "etiketler", &json!([1, 2]));
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0].path, "etiketler.0");
        assert_eq!(items[1].path, "etiketler.1");
    }

    #[test]
    fn nested_object_violation_reports_the_inner_path() {
        let v = validate_at(&ctx(), "musteri", &json!({ "ad": "Ay", "bakiye": "çok" }));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].path, "musteri.bakiye", "{v:?}");
    }

    #[test]
    fn input_validation_skips_absent_paths() {
        let paths = vec!["tutar".to_string(), "karar".to_string()];
        // `karar` gönderilmedi → varlık denetimi burada YAPILMAZ.
        let v = validate_input(&ctx(), &paths, &json!({ "tutar": 5 }));
        assert!(v.is_empty(), "{v:?}");
        let bad = validate_input(&ctx(), &paths, &json!({ "tutar": "beş" }));
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn dynctx_report_separates_violations_from_undeclared_fields() {
        let report = validate_dynctx(
            &ctx(),
            &json!({ "tutar": "yüz bin", "bilinmeyen_alan": 1, "karar": "onay" }),
        );
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].path, "tutar");
        assert_eq!(report.undeclared, vec!["bilinmeyen_alan".to_string()]);
    }

    #[test]
    fn violation_message_is_readable() {
        let v = validate_at(&ctx(), "tutar", &json!("x"));
        let text = v[0].to_string();
        assert!(text.contains("tutar"), "{text}");
        assert!(text.contains("number"), "{text}");
    }
}
