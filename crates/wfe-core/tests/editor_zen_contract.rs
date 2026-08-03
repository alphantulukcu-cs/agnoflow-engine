//! EDİTÖR ↔ MOTOR SÖZLEŞMESİ (WOR-84).
//!
//! Buradaki her string, WFD editörünün koşul kurucusunun (`agnoflow-frontend`
//! `src/utils/zenUtils.ts::generateZen`) ÜRETTİĞİ tam metindir. Test, üretilen her biçimin
//! zen-expression'da hem parse edildiğini hem doğru değerlendiğini doğrular.
//!
//! Neden ayrı bir test: editörün kendi önizlemesi ve simülatörü ifadeleri JS'te
//! değerlendirir, yani bozuk bir üretim orada YEŞİL görünür. `count(filter($wfah, …))` ve
//! `every($wfah, …)` tam bu yüzden aylarca fark edilmedi — zen'de ikisi de parse hatası
//! (closure fonksiyonları iki argümanlıdır; `every` diye bir fonksiyon yok, karşılığı
//! `all`). Editörün ürettiği yeni bir biçim varsa BURAYA da eklenir.
use serde_json::json;

#[test]
fn editor_generated_zen_parses_and_evaluates() {
    let ctx = json!({"$wfah":[
      {"seq":1,"action":"basvuru","actor":{"role":"clerk"},"input":null,"at":"2026-01-01T00:00:00Z"},
      {"seq":2,"action":"incele","actor":{"role":"mudur"},"input":{"tutar":1500},"at":"2026-01-02T00:00:00Z"}
    ],
    "$prev":{"seq":2,"action":"incele","actor":{"role":"mudur"},"input":{"tutar":1500},"at":"2026-01-02T00:00:00Z"},
    "$first":{"seq":1,"action":"basvuru","actor":{"role":"clerk"},"input":null,"at":"2026-01-01T00:00:00Z"}});

    // Her satır: (ifade, beklenen sonuç)
    let cases: &[(&str, bool)] = &[
        (r#"count($wfah, #.action == "incele") >= 1"#, true),
        (r#"count($wfah, #.action == "incele") == 1"#, true),
        (r#"some($wfah, #.action == "incele")"#, true),
        (r#"all($wfah, #.actor.role != "x")"#, true),
        (r#"none($wfah, #.action == "reddet")"#, true),
        (r#"one($wfah, #.action == "incele")"#, true),
        (r#"$prev.action == "incele""#, true),
        (r#"$first.actor.role != "memur""#, true),
        (r#"($prev.action == "incele" and $prev.actor.role == "mudur")"#, true),
        ("$prev.seq >= 3", false),
        ("$prev.seq >= 2", true),
        ("$prev.input.onay == true", false),
        (r#"some($wfah, #.action in ["basvuru", "incele"])"#, true),
        ("some($wfah, #.seq not in [1, 2])", false),
        (r#"some($wfah, contains(#.action, "incel"))"#, true),
        (r#"$prev.action != "baslat""#, true),
        (r#"some($wfah, (#.action == "incele" and #.actor.role == "mudur"))"#, true),
    ];
    for (expr, want) in cases {
        zen_expression::validate::validate_expression(expr)
            .unwrap_or_else(|e| panic!("PARSE FAIL {expr}: {e:?}"));
        let got = zen_expression::evaluate_expression(expr, ctx.clone().into())
            .unwrap_or_else(|e| panic!("EVAL FAIL {expr}: {e:?}"));
        assert_eq!(got.as_bool(), Some(*want), "{expr}");
    }
}

/// `#.input.*` üzerinde SIRALAMA operatörü: zen'de `null` ile karşılaştırma
/// `Compare: Unsupported type` hatasıdır ve girdisi olmayan geçmiş satırı DAİMA vardır
/// (start aksiyonu, sistem marker'ları). Editörün kurucusu bu yüzden aksiyon kapısı
/// zorunlu kılar — kuralın ÜÇ dayanağı burada sabitlenir:
///
///   1. kapı `and` ile ve karşılaştırmadan ÖNCE olmalı (kısa devre soldan sağa),
///   2. `or` kapı DEĞİLDİR,
///   3. dış `and`'deki kapı iç gruba da geçer.
#[test]
fn ordering_on_wfah_input_requires_a_preceding_and_gate() {
    let ctx = json!({"$wfah":[
      {"action":"basvuru","input":null},
      {"action":"skor","input":{"tutar":1500}}
    ],
    "$prev":{"action":"basvuru","input":null}});

    let eval = |e: &str| zen_expression::evaluate_expression(e, ctx.clone().into());

    // Kapısız → motor patlar. Editörün engellemesi gereken tam biçim.
    assert!(eval("some($wfah, #.input.tutar > 1000)").is_err());
    // Kapı SONRA → yine patlar: sıra önemlidir.
    assert!(eval("some($wfah, #.input.tutar > 1000 and #.action == 'skor')").is_err());
    // `or` kapı değildir.
    assert!(eval("some($wfah, #.action == 'skor' or #.input.tutar > 1000)").is_err());

    // Kapı ÖNCE + `and` → çalışır.
    assert_eq!(
        eval("some($wfah, #.action == 'skor' and #.input.tutar > 1000)")
            .unwrap()
            .as_bool(),
        Some(true)
    );
    // Dış `and`'deki kapı iç gruba geçer.
    assert_eq!(
        eval("some($wfah, #.action == 'skor' and (#.input.tutar > 1000 or #.input.tutar > 5000))")
            .unwrap()
            .as_bool(),
        Some(true)
    );

    // `$prev`/`$first` kapsamı da BAĞIŞIK DEĞİL: tek girdiye bakar ama o girdinin
    // input'u null olabilir (ya da geçmiş boştur → kabuk null döner).
    assert!(eval("$prev.input.tutar > 1000").is_err());
    assert_eq!(
        eval("$prev.action == 'skor' and $prev.input.tutar > 1000")
            .unwrap()
            .as_bool(),
        Some(false)
    );

    // Eşitlik operatörleri kapı GEREKTİRMEZ — `null == x` zen'de sorun değil.
    assert_eq!(
        eval("some($wfah, #.input.tutar == 1500)").unwrap().as_bool(),
        Some(true)
    );
}

/// Sentinel'in yeni biçimi BOŞ geçmişte de patlamaz (eski biçim patlıyordu).
#[test]
fn new_sentinel_survives_empty_history() {
    let empty = json!({"$wfah":[], "$prev":{"seq":null,"action":null,"actor":null,"input":null,"at":null}});
    for expr in [r#"$prev.action != "baslat""#, r#"$prev.action == "baslat""#] {
        let v = zen_expression::evaluate_expression(expr, empty.clone().into())
            .unwrap_or_else(|e| panic!("EVAL FAIL {expr}: {e:?}"));
        assert!(v.as_bool().is_some(), "{expr} boolean üretmeli");
    }
    // Eski biçim aynı bağlamda HÂLÂ patlar — taşınma gerekçesi.
    assert!(zen_expression::evaluate_expression(
        r#"$wfah[len($wfah) - 1].action != "baslat""#, empty.into()).is_err());
}
