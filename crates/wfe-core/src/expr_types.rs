//! ZEN `when` ifadelerinin TİP denetimi — motorun kendi AST'si üzerinden.
//!
//! # Neden motorda
//!
//! Editörün ürettiği JSON ile elle yazılan JSON AYNI kurallara uymak zorundadır. Editör
//! bu kuralları koşul kurucusunda uyguluyordu (`agnoflow-frontend`
//! `src/utils/whenFields.ts`), ama upload kapısı bakmıyordu: elle hazırlanmış bir dosya
//! `some($wfah, #.actor == "ali")` ile yayınlanabiliyordu. Kural setinin sahibi MOTOR
//! olmalı — editör yalnız aynı cevabı önden verir.
//!
//! Bir bilgi editörde çıkarılabiliyorsa JSON'dan da çıkarılabilir: `#.input.<yol>`un tipi
//! girdiyi context'e yazan `wfes_effects` üzerinden bilinir (aksiyon girdisi context'e
//! yazılmak ZORUNDADIR — `unused_action_input`). Bulunamıyorsa dosya eksik hazırlanmıştır
//! ve o eksik zaten kendi kuralıyla reddedilir.
//!
//! # Kuralların dayanağı: `zen-expression`'ın VM'i
//!
//! Kurallar keyfi değil, motorun `vm.rs` davranışının birebir karşılığıdır:
//!
//! * `Opcode::Equal` yalnız (Number,Number) · (String,String) · (Bool,Bool) · (Null,Null) ·
//!   (Date,Date) çiftlerini eşler; **diğer her kombinasyon `false` döner**. Yani obje==obje
//!   ve metin==sayı hata değil, SESSİZCE YANLIŞtır.
//! * `!=` derleyicide `Equal` + `Not` olarak üretilir → aynı çiftler sessizce **`true`**.
//! * `Opcode::Compare` (`> < >= <=`) yalnız (Number,Number) ve (Date,Date) bilir; metinde
//!   bile `Err("Compare: Unsupported type")` → çalışma anında HTTP 500.
//!
//! Bu üç davranış yayında hiçbir iz bırakmaz (koşul "çalışır", yalnız yanlış cevap verir),
//! dolayısıyla tek yakalama noktası tasarım-zamanı doğrulamasıdır.

use std::collections::{HashMap, HashSet};

use bumpalo::Bump;
use serde_json::Value;
use zen_expression::lexer::{ComparisonOperator, Lexer, LogicalOperator, Operator};
use zen_expression::parser::{Node, Parser};

use crate::validator::{schema_type_at, types_compatible};

/// `$wfah` girdisinin motordaki izdüşümü (`v22/eval.rs::project_entry`) — `{seq, action,
/// actor, input, at}`. Editördeki `WFAH_FIELDS` ile AYNI küme olmak zorundadır.
const WFAH_SCALARS: &[(&str, &str)] = &[
    ("seq", "number"),
    ("action", "string"),
    ("at", "string"),
    ("actor.orgu_id", "string"),
    ("actor.user_id", "string"),
    ("actor.role", "string"),
];

/// `$wfah` izdüşümünün ZAMAN DAMGASI alanları.
///
/// Tipi `string`dir ve karşılaştırmaları da STRING temellidir (ayrı bir tarih tipi ve `d()`
/// sarmalı YOK) — ama BİÇİMİ sabittir: UTC `yyyyMMddHHmmss`, 14 rakam. Dolayısıyla
/// `#.at == "onay"` ya da `#.at startsWith "2026-01"` hiçbir kayıtla eşleşmez ve satır
/// sessizce hep-false kalır; `zen_type_mismatch` görmez, iki taraf da `string`.
///
/// Editördeki `zenFunctions.WFAH_TIMESTAMP_FIELDS` ile AYNI küme.
const WFAH_TIMESTAMP_FIELDS: &[&str] = &["at"];

/// Metin fonksiyonu biçiminde yazılan operatörler — editörün `ZEN_TEXT_OPS`'u.
/// Motorda `contains(a, b)` gibi çağrılırlar; sayı/bool tarafta anlamsızdır
/// (`contains` sayıda `Err`, `startsWith` sayıda sessizce yanlış).
const TEXT_OPS: &[&str] = &["contains", "startsWith", "endsWith", "matches"];

/// Zaman damgası sabiti geçerli mi. `exact` = TAM damga şart (`==`/`!=`/`in`) yoksa geçerli
/// bir ÖNEK yeterli (`startsWith`). Önek uzunlukları anlamlı sınırlardır: yıl(4) · ay(6) ·
/// gün(8) · saat(10) · dakika(12) · saniye(14) — `2026011` yarım bir alandır, hiçbir şey
/// ifade etmez. Editördeki `isTimestampLiteral` ile AYNI kümeyi tanır (regex crate'i
/// çekmemek için elle yazıldı).
fn is_timestamp_literal(s: &str, exact: bool) -> bool {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let len = s.len();
    if exact {
        if len != 14 {
            return false;
        }
    } else if ![4, 6, 8, 10, 12, 14].contains(&len) {
        return false;
    }
    let part = |from: usize, lo: u32, hi: u32| -> bool {
        if len < from + 2 {
            return true; // o alan yazılmamış — önekte geçerli
        }
        matches!(s[from..from + 2].parse::<u32>(), Ok(v) if (lo..=hi).contains(&v))
    };
    part(4, 1, 12) && part(6, 1, 31) && part(8, 0, 23) && part(10, 0, 59) && part(12, 0, 59)
}

/// Bir ifadenin tip denetimi için gereken WFD bilgisi.
pub struct ExprEnv<'a> {
    /// `wfd.context` — JSON şeması (`$ctx.<yol>` tipleri buradan).
    pub context: &'a Value,
    /// `$action.input.<yol>` → değerin YAZILDIĞI context yolu. Girdi yollarının tipi
    /// bundan çözülür (bkz. modül dokümanı).
    pub input_ctx_map: HashMap<String, String>,
    /// Dokümandaki TÜM aksiyonların bildirdiği input yolları — `#.input.<yol>`un var olup
    /// olmadığı bununla denetlenir (bir WFAH satırı tek aksiyona bağlı değildir).
    pub declared_inputs: HashSet<String>,
}

/// Çözülebilen JSON tipleri. `Unknown` = bilinmiyor → kural SESSİZ kalır (tahmine dayalı
/// hata üretmek, doğru kurulmuş bir akışı yayınlanamaz yapardı).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Str,
    Num,
    Bool,
    Obj,
    Arr,
    Null,
    Unknown,
}

impl Ty {
    fn from_schema(t: &str) -> Ty {
        match t {
            "string" => Ty::Str,
            "number" | "integer" => Ty::Num,
            "boolean" => Ty::Bool,
            "object" => Ty::Obj,
            "array" => Ty::Arr,
            _ => Ty::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Ty::Str => "string",
            Ty::Num => "number",
            Ty::Bool => "boolean",
            Ty::Obj => "object",
            Ty::Arr => "array",
            Ty::Null => "null",
            Ty::Unknown => "?",
        }
    }

    fn is_container(self) -> bool {
        matches!(self, Ty::Obj | Ty::Arr)
    }
}

/// Bir üye zincirinin kökü — hangi ad alanına bakıyor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    /// `#` (closure) / `$prev` / `$first` — hepsi AYNI `$wfah` izdüşümüne bakar.
    WfahEntry,
    Ctx,
    ActionInput,
    /// Tipi bilinen sistem kökleri.
    Known(Ty),
    Other,
}

/// `Member{Member{root, "a"}, "b"}` zincirini `(kök, "a.b")` hâline düzleştirir.
/// Sayısal indeks (`[0]`) ya da hesaplanmış property varsa `None` — yol değildir.
fn flatten_path<'a>(node: &'a Node<'a>) -> Option<(Root, String)> {
    match node {
        Node::Member { node, property } => {
            let Node::String(name) = property else {
                return None;
            };
            let (root, prefix) = flatten_path(node)?;
            Some((
                root,
                if prefix.is_empty() {
                    (*name).to_string()
                } else {
                    format!("{prefix}.{name}")
                },
            ))
        }
        Node::Parenthesized(inner) => flatten_path(inner),
        // Closure içinde `#` kökü `Pointer` olarak gelir, dışında `Identifier("#")`.
        Node::Pointer => Some((Root::WfahEntry, String::new())),
        Node::Identifier(name) => Some((
            match *name {
                "#" | "$prev" | "$first" => Root::WfahEntry,
                "$ctx" => Root::Ctx,
                "$action" => Root::ActionInput,
                "$actor" => Root::Known(Ty::Obj),
                "$wfah" => Root::Known(Ty::Arr),
                "$timestamp" | "$wfe_id" | "$node" => Root::Known(Ty::Str),
                _ => Root::Other,
            },
            String::new(),
        )),
        _ => None,
    }
}

/// Bir aksiyon girdi yolunun tipi — girdiyi context'e yazan effects üzerinden.
/// En uzun eşleşen önek kullanılır: `musteri` → ctx `musteri` eşlemesi varsa
/// `musteri.ad`ın tipi ctx `musteri.ad`ın tipidir.
fn input_path_type(path: &str, env: &ExprEnv) -> Ty {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    for i in (1..=segs.len()).rev() {
        let Some(target) = env.input_ctx_map.get(&segs[..i].join(".")) else {
            continue;
        };
        let mut ctx_path = target.clone();
        for seg in &segs[i..] {
            ctx_path.push('.');
            ctx_path.push_str(seg);
        }
        return schema_type_at(env.context, &ctx_path)
            .map(|t| Ty::from_schema(&t))
            .unwrap_or(Ty::Unknown);
    }
    Ty::Unknown
}

/// `$wfah` izdüşümündeki bir alanın tipi. Çıplak girdi ve `actor`/`input` NESNEDİR.
fn wfah_field_type(field: &str, env: &ExprEnv) -> Ty {
    if field.is_empty() || field == "actor" || field == "input" {
        return Ty::Obj;
    }
    if let Some((_, t)) = WFAH_SCALARS.iter().find(|(name, _)| *name == field) {
        return Ty::from_schema(t);
    }
    match field.strip_prefix("input.") {
        Some(rest) => input_path_type(rest, env),
        None => Ty::Unknown,
    }
}

fn type_of<'a>(node: &'a Node<'a>, env: &ExprEnv) -> Ty {
    match node {
        Node::Null => Ty::Null,
        Node::Bool(_) => Ty::Bool,
        Node::Number(_) => Ty::Num,
        Node::String(_) | Node::TemplateString(_) => Ty::Str,
        Node::Array(_) => Ty::Arr,
        Node::Object(_) => Ty::Obj,
        Node::Parenthesized(inner) => type_of(inner, env),
        // Fonksiyon sonuçları BİLİNMEZ sayılır: `d(#.at) > d($ctx.son)` gibi meşru tarih
        // karşılaştırmaları yanlış yere hata almasın (motor Dynamic/Dynamic çiftini bilir).
        Node::FunctionCall { .. } | Node::MethodCall { .. } => Ty::Unknown,
        _ => match flatten_path(node) {
            Some((Root::WfahEntry, path)) => wfah_field_type(&path, env),
            Some((Root::Ctx, path)) if !path.is_empty() => schema_type_at(env.context, &path)
                .map(|t| Ty::from_schema(&t))
                .unwrap_or(Ty::Unknown),
            Some((Root::ActionInput, path)) => match path.strip_prefix("input.") {
                Some(rest) => input_path_type(rest, env),
                None => Ty::Unknown,
            },
            Some((Root::Known(t), path)) if path.is_empty() => t,
            _ => Ty::Unknown,
        },
    }
}

/// İfadenin metinsel karşılığı — hata mesajında hangi tarafın kastedildiği görünsün.
fn describe<'a>(node: &'a Node<'a>) -> String {
    match node {
        Node::Null => "null".into(),
        Node::Bool(b) => b.to_string(),
        Node::Number(n) => n.to_string(),
        Node::String(s) => format!("\"{s}\""),
        Node::Parenthesized(inner) => describe(inner),
        _ => match flatten_path(node) {
            Some((root, path)) => {
                let prefix = match root {
                    Root::WfahEntry => "#.",
                    Root::Ctx => "$ctx.",
                    Root::ActionInput => "$action.",
                    _ => "",
                };
                if path.is_empty() {
                    prefix.trim_end_matches('.').to_string()
                } else {
                    format!("{prefix}{path}")
                }
            }
            None => "ifade".into(),
        },
    }
}

type Issue = (&'static str, bool, String);

/// `and` zincirini düz listeye açar (`a and b and c` sağa yatık ağaç gelebilir).
fn flatten_and<'a>(node: &'a Node<'a>, out: &mut Vec<&'a Node<'a>>) {
    match node {
        Node::Binary {
            left,
            operator: Operator::Logical(LogicalOperator::And),
            right,
        } => {
            flatten_and(left, out);
            flatten_and(right, out);
        }
        Node::Parenthesized(inner) => flatten_and(inner, out),
        other => out.push(other),
    }
}

/// Bu düğüm bir AKSİYON KAPISI mı — kendisinden sonraki `#.input.*` sıralama
/// karşılaştırmalarını güvenli kılar mı? Yalnız `#.action == …` ve `#.action in […]`:
/// `!=` girdinin VARLIĞINI garanti etmez (bkz. `tests/editor_zen_contract.rs`).
fn is_action_gate<'a>(node: &'a Node<'a>) -> bool {
    let Node::Binary {
        left,
        operator,
        right: _,
    } = node
    else {
        return false;
    };
    let gate_op = matches!(
        operator,
        Operator::Comparison(ComparisonOperator::Equal) | Operator::Comparison(ComparisonOperator::In)
    );
    gate_op && matches!(flatten_path(left), Some((Root::WfahEntry, p)) if p == "action")
}

/// Bu düğüm bir `$wfah` ZAMAN DAMGASI alanı mı (`#.at` / `$prev.at` / `$first.at`).
fn is_wfah_timestamp<'a>(node: &'a Node<'a>) -> bool {
    matches!(
        flatten_path(node),
        Some((Root::WfahEntry, p)) if WFAH_TIMESTAMP_FIELDS.contains(&p.as_str())
    )
}

struct Checker<'e, 'a> {
    env: &'e ExprEnv<'a>,
    out: Vec<Issue>,
    seen_fields: HashSet<String>,
}

impl<'e, 'a> Checker<'e, 'a> {
    /// `#.<alan>` motorun izdüşümünde var mı — yoksa koşul sessizce `null` okur ve
    /// hep-false olur. Editörün `wfahFieldVerdict`i ile AYNI küme.
    fn check_wfah_field(&mut self, path: &str, origin: &str) {
        if path.is_empty()
            || path == "actor"
            || path == "input"
            || WFAH_SCALARS.iter().any(|(name, _)| *name == path)
        {
            return;
        }
        let key = format!("{origin}.{path}");
        if !self.seen_fields.insert(key.clone()) {
            return;
        }
        let Some(rest) = path.strip_prefix("input.") else {
            self.out.push((
                "zen_wfah_field_unknown",
                true,
                format!(
                    "'{key}' motorun $wfah izdüşümünde yok (yalnız seq, action, actor.role, \
                     actor.orgu_id, actor.user_id, at) — koşul sessizce null okur, hep-false olur"
                ),
            ));
            return;
        };
        // Kapsama İKİ YÖNLÜDÜR (`paths_overlap` ile aynı kural): `input.kredi`,
        // `kredi.tutar` bildirilmişse geçerlidir (nesnenin tamamı); `input.basvuran.ad` da
        // `basvuran` bildirilmişse geçerlidir (bütün olarak bildirilen nesnenin alt alanı —
        // tipi ctx şemasından okunur).
        let declared = self
            .env
            .declared_inputs
            .iter()
            .any(|d| d == rest || d.starts_with(&format!("{rest}.")) || rest.starts_with(&format!("{d}.")));
        if !declared {
            self.out.push((
                "zen_wfah_field_unknown",
                true,
                format!(
                    "'{key}' hiçbir aksiyonun input listesinde bildirilmiyor — geçmişte böyle \
                     bir girdi yolu hiç oluşmaz, koşul hep-false olur"
                ),
            ));
        }
    }

    /// Tüm alt ağaçtaki `$wfah` alan referanslarını denetler (karşılaştırma dışındaki
    /// konumlar dahil: `map(...)`, `filter(...)`, fonksiyon argümanı…).
    fn collect_fields<'n>(&mut self, node: &'n Node<'n>) {
        if let Some((Root::WfahEntry, path)) = flatten_path(node) {
            self.check_wfah_field(&path, "#");
            return;
        }
        match node {
            Node::Member { node, .. } => self.collect_fields(node),
            Node::Parenthesized(inner) => self.collect_fields(inner),
            Node::Unary { node, .. } => self.collect_fields(node),
            Node::Binary { left, right, .. } => {
                self.collect_fields(left);
                self.collect_fields(right);
            }
            Node::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                self.collect_fields(condition);
                self.collect_fields(on_true);
                self.collect_fields(on_false);
            }
            Node::Closure { body, .. } => self.collect_fields(body),
            Node::FunctionCall { arguments, .. } => {
                for a in *arguments {
                    self.collect_fields(a);
                }
            }
            Node::MethodCall { this, arguments, .. } => {
                self.collect_fields(this);
                for a in *arguments {
                    self.collect_fields(a);
                }
            }
            Node::Array(items) => {
                for i in *items {
                    self.collect_fields(i);
                }
            }
            Node::Object(pairs) => {
                for (k, v) in *pairs {
                    self.collect_fields(k);
                    self.collect_fields(v);
                }
            }
            Node::Slice { node, from, to } => {
                self.collect_fields(node);
                if let Some(f) = from {
                    self.collect_fields(f);
                }
                if let Some(t) = to {
                    self.collect_fields(t);
                }
            }
            _ => {}
        }
    }

    /// `x in [...]` listesinin ÖĞELERİ sol tarafla aynı tipte mi. Sağ taraf dizi
    /// LİTERALİ değilse (bir ctx alanı, bir fonksiyon sonucu) tip bilinmez → sessiz.
    fn check_list_elements<'n>(&mut self, left: &'n Node<'n>, lt: Ty, right: &'n Node<'n>) {
        if let Node::Parenthesized(inner) = right {
            return self.check_list_elements(left, lt, inner);
        }
        // Sol taraf obje/dizi ise `in` de anlamsızdır — `Equal` onu hiçbir öğeyle
        // eşleştirmez. Bu daldan geçtiği için obje kuralı (madde 1) görmüyordu.
        if lt.is_container() {
            self.out.push((
                "zen_object_compare",
                true,
                format!(
                    "'{}' bir {} — motor obje/dizi karşılaştırmasını desteklemez, 'in' hiçbir \
                     öğeyle eşleşmez. Skaler bir alt alanı karşılaştırın",
                    describe(left),
                    lt.label()
                ),
            ));
            return;
        }
        let Node::Array(items) = right else {
            return;
        };
        if !matches!(lt, Ty::Str | Ty::Num | Ty::Bool) {
            return;
        }
        // `#.at in [...]` listesinin öğeleri TAM damga olmalı (biçim kuralı, bkz.
        // WFAH_TIMESTAMP_FIELDS): yarım bir önek `In`in öğe eşitliğinde asla tutmaz.
        if is_wfah_timestamp(left) {
            for item in *items {
                if let Node::String(s) = item {
                    if !is_timestamp_literal(s, true) {
                        self.out.push((
                            "zen_timestamp_format",
                            true,
                            format!(
                                "'{}' bir zaman damgasıdır (yyyyMMddHHmmss, 14 rakam) — listedeki \
                                 \"{s}\" hiçbir kayıtla eşleşmez",
                                describe(left)
                            ),
                        ));
                        return;
                    }
                }
            }
        }
        for item in *items {
            let it = type_of(item, self.env);
            if matches!(it, Ty::Unknown | Ty::Null) || types_compatible(it.label(), lt.label()) {
                continue;
            }
            self.out.push((
                "zen_list_type_mismatch",
                true,
                format!(
                    "'{}' {} tipinde ama listedeki {} bir {} — motor öğe öğe eşitlik arar, \
                     farklı tipli öğe hiçbir zaman eşleşmez (koşul sessizce hep-false olur)",
                    describe(left),
                    lt.label(),
                    describe(item),
                    it.label()
                ),
            ));
            return;
        }
    }

    /// Metin fonksiyonu biçimli operatörler (`contains`/`startsWith`/`endsWith`/`matches`)
    /// METİN ister. Motorda sayı/bool tarafta ya `Err` ya sessizce yanlış sonuç verir;
    /// `type_of` fonksiyon çağrısını `Unknown` saydığı için hiçbir kural görmüyordu.
    ///
    /// `startsWith(#.at, …)` ayrıca BİÇİM denetlenir: `at` sabit biçimli bir damga olduğu
    /// için önek anlamlı bir alan sınırında bitmeli (bkz. `is_timestamp_literal`).
    fn check_text_op<'n>(&mut self, name: &str, arguments: &'n [&'n Node<'n>]) {
        if !TEXT_OPS.contains(&name) || arguments.len() != 2 {
            return;
        }
        let (subject, arg) = (arguments[0], arguments[1]);
        // `matches` regex alır — biçim kuralı uygulanamaz.
        if is_wfah_timestamp(subject) && name != "matches" {
            if let Node::String(s) = arg {
                // `startsWith` ÖNEK ister; `contains`/`endsWith` yalnız rakam (damgada harf yok).
                let ok = if name == "startsWith" {
                    is_timestamp_literal(s, false)
                } else {
                    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
                };
                if !ok {
                    self.out.push((
                        "zen_timestamp_format",
                        true,
                        format!(
                            "'{}' bir zaman damgasıdır (yyyyMMddHHmmss) — \"{s}\" hiçbir kayıtla \
                             eşleşmez. Önek anlamlı bir sınırda bitmeli: \"2026\" (yıl), \
                             \"202601\" (ay), \"20260115\" (gün), \"2026011510\" (saat), \
                             \"202601151030\" (dakika), \"20260115103000\" (saniye)",
                            describe(subject)
                        ),
                    ));
                }
            }
            return;
        }
        let st = type_of(subject, self.env);
        if matches!(st, Ty::Num | Ty::Bool | Ty::Obj | Ty::Arr) {
            self.out.push((
                "zen_text_op_not_string",
                true,
                format!(
                    "'{}' {} tipinde — '{name}' yalnız metinde çalışır, motor diğer tiplerde \
                     hata verir ya da sessizce yanlış sonuç döner",
                    describe(subject),
                    st.label()
                ),
            ));
        }
    }

    /// Bir karşılaştırmanın iki tarafını denetler.
    fn check_comparison<'n>(
        &mut self,
        left: &'n Node<'n>,
        op: ComparisonOperator,
        right: &'n Node<'n>,
        gated: bool,
    ) {
        let (lt, rt) = (type_of(left, self.env), type_of(right, self.env));
        // `in` / `not in` liste semantiğidir: İKİ TARAFIN tipi eşit olmak zorunda değil,
        // ama listenin ÖĞELERİ sol tarafla aynı tipte olmalı — `In` opcode'u öğe öğe
        // `Equal` yapar, yani `#.seq in ["a"]` hiçbir öğeyle eşleşmez ve koşul sessizce
        // hep-false olur. Editör tarafındaki karşılığı `listValueIssues`.
        if matches!(op, ComparisonOperator::In | ComparisonOperator::NotIn) {
            self.check_list_elements(left, lt, right);
            return;
        }
        let ordering = matches!(
            op,
            ComparisonOperator::GreaterThan
                | ComparisonOperator::LessThan
                | ComparisonOperator::GreaterThanOrEqual
                | ComparisonOperator::LessThanOrEqual
        );
        // `x == null` / `x != null` VARLIK SORGUSUDUR ve motorun `Equal`ı (Null,Null)
        // çiftini bilir — obje alanda bile meşrudur (editörün "var"/"yok" operatörü tam
        // olarak buna derlenir). Tip kuralları bu biçime karışmaz.
        if !ordering && (lt == Ty::Null || rt == Ty::Null) {
            return;
        }

        // 0. ZAMAN DAMGASI eşitliği TAM damga ister (`yyyyMMddHHmmss`, 14 rakam): biçime
        //    uymayan sabit hiçbir kayıtla eşleşmez ve iki taraf da `string` olduğu için
        //    `zen_type_mismatch` bunu görmez. Sıralama `zen_ordering_not_number`ın işi
        //    (metinde `Compare` runtime patlar) — o yüzden yalnız eşitlikte bakılır.
        if !ordering {
            for (node, other) in [(left, right), (right, left)] {
                if !is_wfah_timestamp(node) {
                    continue;
                }
                if let Node::String(s) = other {
                    if !is_timestamp_literal(s, true) {
                        self.out.push((
                            "zen_timestamp_format",
                            true,
                            format!(
                                "'{}' bir zaman damgasıdır (yyyyMMddHHmmss, 14 rakam, örn. \
                                 20260115103000) — \"{s}\" bu biçimde değil, hiçbir kayıtla \
                                 eşleşmez. Yalnız yıl/ay/gün eşleştirmek için \
                                 startsWith({}, \"20260115\") kullanın",
                                describe(node),
                                describe(node),
                            ),
                        ));
                        return;
                    }
                }
            }
        }

        // 1. Obje/dizi karşılaştırması — obje==obje dahil DAİMA yanlış cevap.
        for (ty, node) in [(lt, left), (rt, right)] {
            if ty.is_container() {
                self.out.push((
                    "zen_object_compare",
                    true,
                    format!(
                        "'{}' bir {} — motor obje/dizi karşılaştırmasını desteklemez ({} daima \
                         yanlış sonuç verir). Skaler bir alt alanı karşılaştırın ya da varlığını \
                         `!= null` ile sorun",
                        describe(node),
                        ty.label(),
                        if ordering { "sıralama" } else { "eşitlik" }
                    ),
                ));
                return;
            }
        }

        // 2. Sıralama SAYI ister (tarih için `d()` sarmalı gerekir — o Unknown'dır, muaf).
        if ordering {
            for (ty, node) in [(lt, left), (rt, right)] {
                if matches!(ty, Ty::Str | Ty::Bool | Ty::Null) {
                    self.out.push((
                        "zen_ordering_not_number",
                        true,
                        format!(
                            "'{}' {} tipinde — '{}' yalnız sayı (ve d() ile tarih) üzerinde \
                             çalışır, motor diğer tiplerde runtime hatası verir",
                            describe(node),
                            ty.label(),
                            op_symbol(op)
                        ),
                    ));
                    return;
                }
            }
            // Kapısız `#.input.*` sıralaması: girdi yoksa `null > 5` RUNTIME patlar.
            if !gated {
                if let Some((Root::WfahEntry, path)) = flatten_path(left) {
                    if path.starts_with("input.") {
                        self.out.push((
                            "zen_input_needs_action_gate",
                            true,
                            format!(
                                "'#.{path}' sıralama karşılaştırması bir aksiyon kapısı ister: \
                                 aynı `and` zincirinde ve ÖNCE `#.action == \"…\"` olmalı — aksi \
                                 halde girdisi olmayan bir geçmiş kaydında motor runtime hatası \
                                 verir (Compare: Unsupported type)"
                            ),
                        ));
                        return;
                    }
                }
            }
        }

        // 3. İki taraf da biliniyor ve tipleri farklı → `==` sessizce false, `!=` sessizce
        //    true. `null` karşılaştırması meşrudur (varlık sorgusu) — muaf.
        if lt != Ty::Unknown
            && rt != Ty::Unknown
            && lt != Ty::Null
            && rt != Ty::Null
            && !types_compatible(lt.label(), rt.label())
        {
            self.out.push((
                "zen_type_mismatch",
                true,
                format!(
                    "'{}' {} tipinde ama '{}' bir {} — motor farklı tipleri eşleştirmez, koşul \
                     sessizce hep aynı sonucu verir",
                    describe(left),
                    lt.label(),
                    describe(right),
                    rt.label()
                ),
            ));
        }
    }

    /// Boolean ağacı gezer; `and` zincirinde aksiyon kapısını biriktirir.
    /// Closure'a girerken kapı SIFIRLANIR: kapı aynı geçmiş kaydını kısıtlamak zorundadır.
    fn visit<'n>(&mut self, node: &'n Node<'n>, gated: bool) {
        match node {
            Node::Binary {
                operator: Operator::Logical(LogicalOperator::And),
                ..
            } => {
                let mut items = Vec::new();
                flatten_and(node, &mut items);
                let mut running = gated;
                for item in items {
                    self.visit(item, running);
                    if is_action_gate(item) {
                        running = true;
                    }
                }
            }
            Node::Binary {
                left,
                operator: Operator::Logical(_),
                right,
            } => {
                // `or` kapı DEĞİLDİR — iki dal da dışarıdan gelen kapıyla devam eder.
                self.visit(left, gated);
                self.visit(right, gated);
            }
            Node::Binary {
                left,
                operator: Operator::Comparison(op),
                right,
            } => {
                self.check_comparison(left, *op, right, gated);
                self.collect_fields(left);
                self.collect_fields(right);
            }
            Node::Binary { left, right, .. } => {
                self.collect_fields(left);
                self.collect_fields(right);
            }
            Node::Parenthesized(inner) => self.visit(inner, gated),
            Node::Unary { node, .. } => self.visit(node, gated),
            Node::Closure { body, .. } => self.visit(body, false),
            Node::FunctionCall { kind, arguments } => {
                // Metin fonksiyonu biçimli OPERATÖR mü (`startsWith(#.at, …)`) — editör
                // bunları düz operatör gibi gösterir, kural seti de öyle davranmalı.
                self.check_text_op(&kind.to_string(), arguments);
                for a in *arguments {
                    self.visit(a, gated);
                }
            }
            Node::MethodCall { this, arguments, .. } => {
                self.visit(this, gated);
                for a in *arguments {
                    self.visit(a, gated);
                }
            }
            Node::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                self.visit(condition, gated);
                self.visit(on_true, gated);
                self.visit(on_false, gated);
            }
            other => self.collect_fields(other),
        }
    }
}

fn op_symbol(op: ComparisonOperator) -> &'static str {
    match op {
        ComparisonOperator::Equal => "==",
        ComparisonOperator::NotEqual => "!=",
        ComparisonOperator::LessThan => "<",
        ComparisonOperator::GreaterThan => ">",
        ComparisonOperator::LessThanOrEqual => "<=",
        ComparisonOperator::GreaterThanOrEqual => ">=",
        ComparisonOperator::In => "in",
        ComparisonOperator::NotIn => "not in",
    }
}

/// TEK bir ZEN ifadesinin TİP kontrolleri — `(kod, hata_mı, mesaj)` üçlüleri.
///
/// `expression_issues` ile aynı sözleşme, farkı WFD bilgisi istemesi (context şeması +
/// girdi eşlemesi). Parse edilemeyen ifade sessiz geçer: onu `expression_issues`
/// (`zen_parse`) zaten reddeder, iki kez bağırmak gerekmez.
pub fn expression_type_issues(expr: &str, env: &ExprEnv) -> Vec<Issue> {
    let bump = Bump::new();
    let source = bump.alloc_str(expr);
    let mut lexer = Lexer::new();
    let Ok(tokens) = lexer.tokenize(source) else {
        return Vec::new();
    };
    let Ok(parser) = Parser::try_new(tokens, &bump) else {
        return Vec::new();
    };
    let result = parser.standard().parse();
    if result.error().is_err() {
        return Vec::new();
    }
    let mut checker = Checker {
        env,
        out: Vec::new(),
        seen_fields: HashSet::new(),
    };
    checker.visit(result.root, false);
    checker.out
}
