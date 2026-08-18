//! `wf.wfe.end_terminal` GERİYE DÖNÜK çıkarımı (2026-08-17) — saf, I/O'suz.
//!
//! ## Neden var
//!
//! `end_terminal` kolonu 2026-08-17'de eklendi; ondan önce sonlanmış WFE satırlarında
//! "hangi bitişe varıldı" HİÇBİR YERDE yazmıyordu. Terminal `listable[]` (bkz.
//! `visibility::can_view` kriteri (g)) tam da bu bilgiye dayandığı için o satırlar
//! yeni grant'ten yararlanamaz.
//!
//! İlk değerlendirmede bu "geri kazanılamaz" sayılmıştı ve gerekçe şuydu: `wft.conditions`
//! O ANKİ ctx ile çözülüyordu, bugün yeniden koşturmak aynı cevabı vermeyebilir. Gerekçe
//! DOĞRU ama SONUÇ YANLIŞTI — çünkü kararı yeniden ÜRETMEK gerekmiyor, kararın İZİNİ
//! okumak yetiyor. Bitmiş satırda üç bağımsız kanıt duruyor:
//!
//! 1. **`wf.wfe.end_response`** — varılan terminal'in `wfe_end_response`'unun ÇÖZÜLMÜŞ
//!    hâli. Anahtar kümesi belgede SABİTTİR (`resolve_value` anahtar eklemez/silmez), ve
//!    sabit değerli alanlar (`{"status": "rejected"}`) bitişleri birbirinden ayırır.
//! 2. **WFAH'ın son gerçek aksiyonu + `from_node`** — o (node, action) ikilisinden
//!    çıkan `wft` hangi terminal'lere gidebiliyorsa aday kümesi odur.
//! 3. **Belgenin kendisi** — `(wfd_id, version)` değişmezdir, yani WFE koşarken hangi
//!    belgeyi gördüyse bugün de aynısını görüyoruz.
//!
//! ## Sözleşme: ASLA TAHMİN ETMEZ
//!
//! Her kanıt yalnız aday kümesini DARALTIR. Tek aday kalırsa `Certain`, birden çok
//! kalırsa `Ambiguous`, hiç kalmazsa `NoMatch` döner — son ikisinde çağıran kolonu
//! YAZMAZ ve satır eski davranışında (yalnız kök `listable`/`wf_admin`) kalır. Yanlış
//! bir `end_terminal` yazmak, görünmemesi gereken bir kişiye bitmiş işi göstermek
//! demektir; "bilmiyorum" demek her zaman daha ucuzdur.
//!
//! Modelin eksik kaldığı yerlerde (tanımadığımız bir `wft` biçimi, `from_node` taşımayan
//! eski WFAH satırı) filtre DARALTMAZ — yanlış cevap üretmek yerine belirsizlik bırakır.

use crate::types::wfd_v22::{Wfd, Wft, WftTarget};
use serde_json::Value;
use std::collections::BTreeSet;

/// Çıkarımın sonucu. `Certain` DIŞINDA hiçbir şey yazılmaz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndTerminalGuess {
    /// Tek aday kaldı — kolona bu yazılabilir.
    Certain(String),
    /// Birden çok aday hayatta; hangisi olduğu kanıtlanamıyor. Liste operatöre
    /// raporlanır (elle karar vermek isteyen için).
    Ambiguous(Vec<String>),
    /// Hiçbir aday kalmadı: belge ile satır birbirini tutmuyor (ör. `end_response`
    /// hiçbir terminal'in şekline uymuyor) ya da iki kanıt ÇELİŞİYOR. Sessizce
    /// geçilmemesi gereken durum — rapor bunu ayrı sayar.
    NoMatch,
}

/// WFAH'ın son GERÇEK aksiyonu (marker değil) ve hangi node'dan alındığı.
///
/// `from_node` `Option`: kolon 2026-08-10'da eklendi, ondan eski WFAH satırlarında
/// NULL'dır. Yokluğu bir hata değildir — yalnız (2) numaralı kanıt zayıflar ve filtre
/// aksiyon adıyla eşleşen TÜM geçişleri aday sayar.
#[derive(Debug, Clone, Copy)]
pub struct LastAction<'a> {
    pub from_node: Option<&'a str>,
    pub action: &'a str,
}

/// Bitmiş bir WFE'nin hangi terminal'de sonlandığını KANITLARDAN çıkarır.
///
/// `end_response`: `wf.wfe.end_response` kolonu (NULL olabilir).
/// `last`: WFAH'ın son gerçek aksiyonu (bulunamazsa `None`).
///
/// Yalnız BAŞARILI `Terminal` satırlarında çağrılmalıdır: `error`/`terminated`
/// satırlarında varılmış bir terminal YOKTUR ve buradan dönen her cevap yanlış olurdu.
pub fn infer_end_terminal(
    wfd: &Wfd,
    end_response: Option<&Value>,
    last: Option<LastAction<'_>>,
) -> EndTerminalGuess {
    let all: BTreeSet<String> = wfd.terminals.iter().map(|t| t.id.clone()).collect();
    if all.is_empty() {
        return EndTerminalGuess::NoMatch;
    }
    // Tek terminal'li belgede soru zaten yok: WFE `terminal` durumundaysa oraya varmıştır.
    if all.len() == 1 {
        return EndTerminalGuess::Certain(all.into_iter().next().expect("len==1"));
    }

    // (1) Yanıt gövdesi filtresi.
    let by_response: BTreeSet<String> = match end_response {
        Some(resp) => wfd
            .terminals
            .iter()
            .filter(|t| response_matches(t.wfe_end_response.iter(), resp))
            .map(|t| t.id.clone())
            .collect(),
        None => all.clone(),
    };

    // (2) Son aksiyondan erişilebilirlik filtresi. BOŞ dönerse (tanımadığımız bir yol,
    // `from_node` yok, marker'lar) daraltma YAPILMAZ — modelin eksiği belirsizliğe
    // dönüşür, yanlış cevaba değil.
    let by_reach = last
        .map(|l| reachable_terminals(wfd, l))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| all.clone());

    let survivors: Vec<String> = by_response.intersection(&by_reach).cloned().collect();

    match survivors.len() {
        1 => EndTerminalGuess::Certain(survivors.into_iter().next().expect("len==1")),
        0 => EndTerminalGuess::NoMatch,
        _ => EndTerminalGuess::Ambiguous(survivors),
    }
}

/// Bir terminal'in `wfe_end_response` şablonu, kayıtlı yanıtla BAĞDAŞIYOR mu?
///
/// İki kapı:
/// * **Anahtar kümesi TAM eşleşmeli.** Şablonun anahtarları belgede sabittir;
///   `$`-çözümü değer üretir, anahtar eklemez/silmez. En keskin ayraç budur.
/// * **Sabit değerler birebir eşleşmeli.** İçinde `$`-referansı GEÇEN değer atlanır:
///   çalışma anındaki ctx'e bağlıydı, bugünkü kayıtla karşılaştırmak anlamsız. Atlamak
///   yalnız ayırt etme gücünü düşürür — yanlış eşleşme ÜRETMEZ.
fn response_matches<'a, I>(template: I, stored: &Value) -> bool
where
    I: Iterator<Item = (&'a String, &'a Value)>,
{
    let Some(obj) = stored.as_object() else {
        return false;
    };
    let mut declared = 0usize;
    for (key, decl) in template {
        declared += 1;
        let Some(actual) = obj.get(key) else {
            return false;
        };
        if !contains_dollar_ref(decl) && decl != actual {
            return false;
        }
    }
    declared == obj.len()
}

/// Değerin herhangi bir yerinde `$` ile başlayan bir metin var mı?
///
/// Kaba ama BİLİNÇLİ olarak kaba: `"$100 ödendi"` gibi gerçek bir sabit de `$`-referansı
/// sayılır ve karşılaştırma dışında kalır. Yanlış yönde hata yapmaz — yalnız o alanı
/// ayraç olarak kullanmaktan vazgeçeriz.
fn contains_dollar_ref(v: &Value) -> bool {
    match v {
        Value::String(s) => s.starts_with('$'),
        Value::Array(items) => items.iter().any(contains_dollar_ref),
        Value::Object(map) => map.values().any(contains_dollar_ref),
        _ => false,
    }
}

/// Verilen (node, action) ikilisinden ÇIKAN yolların ulaşabildiği terminal'ler.
///
/// Üç kaynak taranır — bir WFE'yi terminal'e götürebilen bütün yollar:
/// * `transitions[]` — normal ACT geçişi,
/// * `start[]` — tek adımda biten akış (start kuralının `wft`'si doğrudan terminal),
/// * `nodes.<key>.call.wft` — WFC-RETURN (SLA'nın terminal yasağı burada GEÇERSİZ).
///
/// `escalation[]`/`claim_timeout` taranmaz: ikisinin de hedefi NODE olmak zorundadır
/// (validator `sla_terminal_target`), yani terminal üretemezler.
fn reachable_terminals(wfd: &Wfd, last: LastAction<'_>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();

    for tr in &wfd.transitions {
        if tr.action != last.action {
            continue;
        }
        // `from_node` bilinmiyorsa (eski WFAH satırı) aksiyon adı tek ölçüttür.
        if let Some(node) = last.from_node {
            if !tr.from.contains(node) {
                continue;
            }
        }
        collect_wft_terminals(&tr.wft, &mut out);
    }

    for rule in &wfd.start {
        if rule.action != last.action {
            continue;
        }
        if let Some(node) = last.from_node {
            if rule.from != node {
                continue;
            }
        }
        collect_wft_terminals(&rule.wft, &mut out);
    }

    // WFC dönüşü: çağrı node'unda insan ACT'i alınmaz, o yüzden yalnız `from_node`
    // biliniyorsa bakılır — aksi halde belgedeki HER çağrı node'unu aday sayardık.
    if let Some(node_key) = last.from_node {
        if let Some(wft) = wfd
            .nodes
            .get(node_key)
            .and_then(|n| n.call.as_ref())
            .and_then(|c| c.wft.as_ref())
        {
            collect_wft_terminals(wft, &mut out);
        }
    }

    // PARALEL DELİĞİ (bilinçli genişletme). Join hedefi terminal olan bir fork'ta WFE,
    // KOL aksiyonuyla değil JOIN dolduğunda sonlanır: son gerçek aksiyon kolun kendi
    // geçişidir ve o geçişin `wft`'si NODE'a bakar — join terminal'i oradan HİÇ
    // görünmez. Filtre bu hâliyle bırakılırsa "boş değil ama YANLIŞ" bir küme üretir ve
    // yanlış bir `Certain` yazılabilirdi (modülün tek gerçek tehlikesi budur: false
    // terminal her iki filtreden de geçerken doğru olanın erişilebilirlikten düşmesi).
    //
    // Çözüm hassas değil, GÜVENLİ yönde: belgede paralel varsa TÜM join terminal'leri
    // adaylara eklenir. Genişletmek yalnız kesinlik kaybettirir (daha çok `Ambiguous`),
    // asla yanlış cevap üretmez — hangi fork'un içinde olunduğunu WFAH'tan güvenilir
    // biçimde çıkarmaya kalkmak ise tam da kaçınmak istediğimiz yeniden-üretim olurdu.
    collect_join_terminals(wfd, &mut out);

    out
}

/// Belgedeki TÜM `Wft::Parallel` join hedeflerinin terminal olanları.
///
/// Fork'lar `transitions[]`, `start[]` ve `nodes.<k>.call.wft` içinde durabilir —
/// üçü de taranır, çünkü eksik bırakılan yer yukarıdaki deliği açık bırakır.
fn collect_join_terminals(wfd: &Wfd, out: &mut BTreeSet<String>) {
    let mut add = |wft: &Wft| {
        if let Wft::Parallel { parallel } = wft {
            if let WftTarget::Terminal { terminal } = &parallel.join {
                out.insert(terminal.clone());
            }
        }
    };
    for tr in &wfd.transitions {
        add(&tr.wft);
    }
    for rule in &wfd.start {
        add(&rule.wft);
    }
    for node in wfd.nodes.values() {
        if let Some(wft) = node.call.as_ref().and_then(|c| c.wft.as_ref()) {
            add(wft);
        }
    }
}

/// Bir `wft`in gidebileceği terminal id'leri. Node hedefleri ilgilendirmez.
///
/// `Wft::Targets` (GLB) yalnız node taşır — şema `GlobalTarget { node }` ile bunu
/// zorluyor, dolayısıyla terminal üretemez.
fn collect_wft_terminals(wft: &Wft, out: &mut BTreeSet<String>) {
    match wft {
        Wft::Terminal { terminal } => {
            out.insert(terminal.clone());
        }
        Wft::Conditional { conditions, default } => {
            for c in conditions {
                if let Some(t) = &c.terminal {
                    out.insert(t.clone());
                }
            }
            if let Some(WftTarget::Terminal { terminal }) = default {
                out.insert(terminal.clone());
            }
        }
        Wft::Parallel { parallel } => {
            if let WftTarget::Terminal { terminal } = &parallel.join {
                out.insert(terminal.clone());
            }
        }
        Wft::Collapse { collapse } => {
            if let WftTarget::Terminal { terminal } = collapse {
                out.insert(terminal.clone());
            }
        }
        Wft::Node { .. } | Wft::Targets { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURE: &str = include_str!("../../../../docs/spec/examples/kredi-basvuru.golden.json");

    fn golden() -> Wfd {
        Wfd::from_json(FIXTURE).unwrap()
    }

    /// Golden'da iki terminal var ve `wfe_end_response` şablonları AYRIŞIYOR —
    /// gerçek bir belgede yanıt filtresinin tek başına ne kadar ayırt ettiğini ölçer.
    #[test]
    fn golden_terminals_are_separable_by_their_response_shape() {
        let wfd = golden();
        assert!(wfd.terminals.len() >= 2, "fixture en az iki terminal taşımalı");
        for t in &wfd.terminals {
            // Her terminal'in KENDİ şablonundan üretilmiş bir yanıt yalnız KENDİSİNE uymalı.
            let resp = synth_response(&wfd, &t.id);
            let hits: Vec<&str> = wfd
                .terminals
                .iter()
                .filter(|o| response_matches(o.wfe_end_response.iter(), &resp))
                .map(|o| o.id.as_str())
                .collect();
            assert_eq!(hits, vec![t.id.as_str()], "{} ayırt edilemedi", t.id);
        }
    }

    /// Şablondan sentetik bir "çözülmüş yanıt" üretir: sabitler aynen, `$`-referansları
    /// yerine bir yer tutucu (çalışma anında ne yazdığı bilinmiyor).
    fn synth_response(wfd: &Wfd, terminal_id: &str) -> Value {
        let t = wfd.terminals.iter().find(|t| t.id == terminal_id).unwrap();
        let mut map = serde_json::Map::new();
        for (k, v) in &t.wfe_end_response {
            map.insert(
                k.clone(),
                if contains_dollar_ref(v) { json!("<runtime>") } else { v.clone() },
            );
        }
        Value::Object(map)
    }

    #[test]
    fn single_terminal_document_is_certain_without_any_evidence() {
        let mut wfd = golden();
        wfd.terminals.truncate(1);
        let id = wfd.terminals[0].id.clone();
        assert_eq!(infer_end_terminal(&wfd, None, None), EndTerminalGuess::Certain(id));
    }

    #[test]
    fn response_alone_can_decide() {
        let wfd = golden();
        for t in &wfd.terminals {
            let resp = synth_response(&wfd, &t.id);
            assert_eq!(
                infer_end_terminal(&wfd, Some(&resp), None),
                EndTerminalGuess::Certain(t.id.clone()),
            );
        }
    }

    /// Anahtar kümesi TAM eşleşme ister: fazladan alan taşıyan yanıt hiçbir şablona uymaz.
    #[test]
    fn extra_key_in_stored_response_matches_nothing() {
        let wfd = golden();
        let mut resp = synth_response(&wfd, &wfd.terminals[0].id);
        resp.as_object_mut().unwrap().insert("sarkan".into(), json!(1));
        assert_eq!(infer_end_terminal(&wfd, Some(&resp), None), EndTerminalGuess::NoMatch);
    }

    /// Kanıt yoksa karar da yok — belgede iki terminal varken sessizce birini SEÇMEZ.
    #[test]
    fn no_evidence_stays_ambiguous() {
        let wfd = golden();
        match infer_end_terminal(&wfd, None, None) {
            EndTerminalGuess::Ambiguous(ids) => assert_eq!(ids.len(), wfd.terminals.len()),
            other => panic!("kanıtsız karar verilmemeli: {other:?}"),
        }
    }

    /// İki kanıt ÇELİŞİRSE (yanıt A der, erişilebilirlik YALNIZ B der) cevap
    /// `NoMatch`tir — birini seçmek, hangisinin yanıldığını bilmeden yazmak olurdu.
    ///
    /// Çelişki ELDE KURULUR: fixture'ın koşullu `wft`leri iki terminal'e de gidebildiği
    /// için gerçek bir çelişki barındırmıyor (ve barındırmaması İYİ). Bir geçişin
    /// `wft`'si B'ye SABİTLENİR, yanıt ise A'nın şablonundan üretilir.
    #[test]
    fn contradicting_evidence_is_no_match() {
        let mut wfd = golden();
        let a = wfd.terminals[0].id.clone();
        let b = wfd.terminals[1].id.clone();
        let resp = synth_response(&wfd, &a);

        let (node, action) = transition_to(&wfd, &b).expect("fixture'da terminal geçişi olmalı");
        let tr = wfd
            .transitions
            .iter_mut()
            .find(|t| t.action == action && t.from.contains(&node))
            .expect("geçiş");
        tr.wft = Wft::Terminal { terminal: b.clone() };

        // Kontrol: erişilebilirlik artık YALNIZ B.
        let reach = reachable_terminals(&wfd, LastAction { from_node: Some(&node), action: &action });
        assert_eq!(reach.iter().cloned().collect::<Vec<_>>(), vec![b]);

        let guess = infer_end_terminal(
            &wfd,
            Some(&resp),
            Some(LastAction { from_node: Some(&node), action: &action }),
        );
        assert_eq!(guess, EndTerminalGuess::NoMatch);
    }

    /// Erişilebilirlik TEK BAŞINA karar verebiliyorsa yanıt olmadan da yeter.
    #[test]
    fn reachability_alone_can_decide() {
        let wfd = golden();
        let target = &wfd.terminals[0].id;
        let (node, action) = transition_to(&wfd, target).expect("geçiş");
        let reach = reachable_terminals(&wfd, LastAction { from_node: Some(&node), action: &action });
        // Fixture'da bu geçiş tek terminal'e gidiyorsa `Certain` bekleriz; birden çok
        // hedefi varsa test yalnız "daraltıyor" iddiasını kontrol eder.
        if reach.len() == 1 {
            assert_eq!(
                infer_end_terminal(&wfd, None, Some(LastAction { from_node: Some(&node), action: &action })),
                EndTerminalGuess::Certain(target.clone()),
            );
        } else {
            assert!(reach.len() < wfd.terminals.len() || wfd.terminals.len() == reach.len());
        }
    }

    /// `from_node` YOKSA (2026-08-10 öncesi WFAH satırı) filtre çökmez, yalnız zayıflar.
    #[test]
    fn missing_from_node_still_narrows_by_action_name() {
        let wfd = golden();
        let target = &wfd.terminals[0].id;
        let (_, action) = transition_to(&wfd, target).expect("geçiş");
        let reach = reachable_terminals(&wfd, LastAction { from_node: None, action: &action });
        assert!(reach.contains(target), "aksiyon adıyla da hedefe ulaşılmalı");
    }

    /// Tanınmayan aksiyon → boş küme → daraltma YOK (yanlış cevap değil).
    #[test]
    fn unknown_action_does_not_narrow() {
        let wfd = golden();
        let reach = reachable_terminals(&wfd, LastAction { from_node: None, action: "boyle_aksiyon_yok" });
        assert!(reach.is_empty());
        // Yanıt kanıtı hâlâ çalışır — erişilebilirlik boşsa devre dışı kalır.
        let resp = synth_response(&wfd, &wfd.terminals[0].id);
        assert_eq!(
            infer_end_terminal(&wfd, Some(&resp), Some(LastAction { from_node: None, action: "boyle_aksiyon_yok" })),
            EndTerminalGuess::Certain(wfd.terminals[0].id.clone()),
        );
    }

    /// PARALEL DELİĞİ regresyonu: join hedefi terminal olan bir fork'ta, KOL aksiyonundan
    /// çıkan erişilebilirlik kümesi join terminal'ini de İÇERMELİ. İçermezse filtre
    /// "boş değil ama yanlış" olur ve yanlış bir `Certain` yazılabilir.
    #[test]
    fn join_terminal_is_reachable_from_a_branch_action() {
        let paralel: Wfd = Wfd::from_json(include_str!(
            "../../../../docs/spec/examples/paralel-onay.json"
        ))
        .unwrap();

        // Belgede join hedefi terminal olan bir fork var mı? Yoksa deliği elde kuralım.
        let mut wfd = paralel;
        let join_terminal = wfd.terminals[0].id.clone();
        let fork = wfd
            .transitions
            .iter_mut()
            .find(|t| matches!(t.wft, Wft::Parallel { .. }))
            .expect("paralel fixture bir fork taşımalı");
        if let Wft::Parallel { parallel } = &mut fork.wft {
            parallel.join = WftTarget::Terminal { terminal: join_terminal.clone() };
        }

        // Kol içindeki HERHANGİ bir aksiyon: node hedefli, join'i hiç görmüyor.
        let (node, action) = wfd
            .transitions
            .iter()
            .find(|t| matches!(t.wft, Wft::Node { .. }))
            .map(|t| (t.from.iter()[0].to_string(), t.action.clone()))
            .expect("node hedefli bir geçiş olmalı");

        let reach = reachable_terminals(&wfd, LastAction { from_node: Some(&node), action: &action });
        assert!(
            reach.contains(&join_terminal),
            "join terminal'i aday kümesinde OLMALI, yoksa yanlış Certain yazılabilir: {reach:?}",
        );
    }

    /// Belgedeki bir terminal'e giden (node, action) ikilisi.
    fn transition_to(wfd: &Wfd, terminal_id: &str) -> Option<(String, String)> {
        for tr in &wfd.transitions {
            let mut hits = BTreeSet::new();
            collect_wft_terminals(&tr.wft, &mut hits);
            if hits.contains(terminal_id) {
                return Some((tr.from.iter()[0].to_string(), tr.action.clone()));
            }
        }
        None
    }
}
