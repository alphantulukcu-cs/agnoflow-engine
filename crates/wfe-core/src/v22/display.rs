//! Aksiyon/node/terminal **GÖSTERİM adları** — `Ref.label`'ın TEK kaynağı.
//!
//! Motor kimlikleri makine anahtarlarıdır: node key'i TASARIMCI verir (2026-08-12;
//! `c_a`'dan TÜRETİLMEZ, biçim kısıtı şemada: `^[A-Za-z_][A-Za-z0-9_-]*$`), terminal
//! id'si `^[a-zA-Z0-9_]+$` bir sabittir. Eski belgelerde anahtarlar tarihsel olarak
//! `slug(c_a)` biçiminde olabilir (`self__mudur`) — bu bir SÖZLEŞME DEĞİL, yalnız veri.
//! Bunlar **wire sözleşmesidir, kullanıcı metni DEĞİLDİR** — istemci
//! onları GERİ GÖNDERİR, ASLA AYRIŞTIRMAZ ve ASLA EKRANA BASMAZ.
//!
//! Kimlik ile gösterimi ayırmanın gerekçesi: anahtar isteğe (`POST /wfe/:id/actions`)
//! gider, etiket yalnız ekrana. İkisi API'de tek bir çift olarak (`{id, label}`)
//! dolaşır ve `label` ASLA boş dönmez — belgede yoksa anahtarın okunur hâli üretilir,
//! böylece istemcinin fallback yazmasına gerek kalmaz.

use crate::types::wfd_v22::Wfd;

/// Makine anahtarını okunur metne çevirir: `_`/`-` boşluk olur, tekrarlar tekleşir.
/// Anahtarı DEĞİŞTİRMEZ — yalnız gösterim üretir. `_` ile BAŞLAYAN anahtarlar
/// (motorun kendi işaretleri: `_branch_cancelled`) olduğu gibi bırakılır: istemciler
/// onları metinden tanıyor, sözleşme sayılırlar.
pub fn humanize_key(key: &str) -> String {
    if key.starts_with('_') {
        return key.to_string();
    }
    let spaced: String = key
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c })
        .collect();
    let out = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() {
        key.to_string()
    } else {
        out
    }
}

fn non_empty(s: Option<&String>) -> Option<&str> {
    s.map(|v| v.trim()).filter(|v| !v.is_empty())
}

/// Bir node'un gösterim adı: `nodes.<key>.label`, yoksa anahtarın okunur hâli.
pub fn node_label(wfd: &Wfd, node_key: &str) -> String {
    non_empty(wfd.nodes.get(node_key).and_then(|n| n.label.as_ref()))
        .map(str::to_string)
        .unwrap_or_else(|| humanize_key(node_key))
}

/// Bir terminal'in gösterim adı: `terminals[].label`, yoksa id'nin okunur hâli.
/// Terminal id'si artık makine kimliğidir (`^[a-zA-Z0-9_]+$`), bu yüzden ekrana
/// basılabilir tek metin buradan çıkar.
pub fn terminal_label(wfd: &Wfd, terminal_id: &str) -> String {
    non_empty(
        wfd.terminals
            .iter()
            .find(|t| t.id == terminal_id)
            .and_then(|t| t.label.as_ref()),
    )
    .map(str::to_string)
    .unwrap_or_else(|| humanize_key(terminal_id))
}

/// Bir aksiyonun gösterim adı: `actions.<key>.label`, yoksa anahtarın okunur hâli.
///
/// Geri gönderme burada ÖZEL HAL DEĞİLDİR (2026-08-21): ne rezerve bir anahtar ne de
/// sabit bir metin vardır — editör adı `Geri Gönder`, `Geri Gönder 2`… diye üretir ve
/// hepsine AYNI `label`ı ("Geri Gönder") yazar, yani gösterimin tekliği belgeden gelir,
/// motordan değil. Hedef aksiyon anahtarına kodlanmadığı için (`Wft::SendBack`)
/// bölünecek bir anahtar da yok; hedefin etiketi ayrı bir `Ref` olarak
/// `send_back_target_label`'dan gelir.
pub fn action_label(wfd: &Wfd, action: &str) -> String {
    non_empty(wfd.actions.get(action).and_then(|a| a.label.as_ref()))
        .map(str::to_string)
        .unwrap_or_else(|| humanize_key(action))
}

/// Bir geri gönderme HEDEFİNİN gösterim adı: hedefin kendi `label`'ı, yoksa hedef
/// node'un `label`'ı, o da yoksa anahtarın okunur hâli.
///
/// Portal bu metni doğrudan butona basar ("Başa Gönder", "Şube Müdürüne Gönder").
/// Hedef etiketi node etiketinden AYRI tutulur çünkü ikisi farklı soruyu yanıtlar:
/// node label'ı "bu adım kimin havuzu" (her yerde aynı), hedef label'ı "buraya geri
/// göndermek NE DEMEK" (gönderen node'a göre değişir — aynı node'a başka bir adımdan
/// geri gönderirken metin de başka olabilir).
pub fn send_back_target_label(wfd: &Wfd, node_key: &str, target_label: Option<&str>) -> String {
    match target_label.map(str::trim).filter(|v| !v.is_empty()) {
        Some(text) => text.to_string(),
        None => node_label(wfd, node_key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::wfd_v22::Wft;
    use crate::types::wfd_v22::{ActionDef, InputDef, NodeDef};
    use serde_json::json;

    fn wfd() -> Wfd {
        let mut w: Wfd = serde_json::from_value(json!({
            "wfd_version": "2.3",
            "id": "x", "name": "X", "version": "1.0.0",
            "context": {"type": "object", "properties": {}},
            "nodes": {}, "start": [], "actions": {},
            "terminals": [
                {"id": "onaylandi", "label": "Onaylandı", "wfe_end_response": {}},
                {"id": "reddedildi", "wfe_end_response": {}}
            ]
        }))
        .expect("fixture");
        w.nodes.insert(
            "self__mudur".into(),
            serde_json::from_value::<NodeDef>(json!({
                "label": "Müdür",
                "c_a": {"c_orgu": "self", "c_r": ["mudur"]}
            }))
            .expect("node"),
        );
        w.nodes.insert(
            "self__gm".into(),
            serde_json::from_value::<NodeDef>(json!({
                "c_a": {"c_orgu": "self", "c_r": ["gm"]}
            }))
            .expect("node"),
        );
        w
    }

    fn act(label: Option<&str>) -> ActionDef {
        ActionDef {
            label: label.map(str::to_string),
            description: None,
            input: InputDef {
                required: vec![],
                optional: vec![],
            },
            // v2.3 (`Ç5`): yönlendirme alanları aksiyon kaydına indi. Bu test yalnız
            // GÖSTERİM adını sınıyor, yönlendirmeyi değil — alanlar en yalın geçerli
            // değerlerle doldurulur.
            from: "self__gm".into(),
            when: None,
            extra_c_a: None,
            wfes_effects: None,
            trigger: vec![],
            wft: Wft::Terminal {
                terminal: "t_done".into(),
            },
        }
    }

    #[test]
    fn plain_action_uses_label_then_humanized_key() {
        let mut w = wfd();
        w.actions
            .insert("Onayla".into(), act(Some("Onayla ve gönder")));
        w.actions.insert("Geri_Cevir".into(), act(None));
        assert_eq!(action_label(&w, "Onayla"), "Onayla ve gönder");
        assert_eq!(action_label(&w, "Geri_Cevir"), "Geri Cevir");
        // Tanımsız aksiyon da okunur döner (katalogda olmayan anahtar).
        assert_eq!(action_label(&w, "Bir_Sey"), "Bir Sey");
    }

    /// Geri gönderme aksiyonunun etiketi de belgeden gelir: motorda ÖZEL HAL YOK.
    /// Editör ikinci geri göndermeye `Geri_Gonder_2` anahtarı verir ama `label`ı
    /// AYNI yazar — kullanıcı iki ayrı kimliği aynı isimle görür.
    #[test]
    fn send_back_actions_share_a_label_but_not_an_identity() {
        let mut w = wfd();
        w.actions
            .insert("Geri_Gonder".into(), act(Some("Geri Gönder")));
        w.actions
            .insert("Geri_Gonder_2".into(), act(Some("Geri Gönder")));
        assert_eq!(action_label(&w, "Geri_Gonder"), "Geri Gönder");
        assert_eq!(action_label(&w, "Geri_Gonder_2"), "Geri Gönder");
        // Label yazılmamışsa anahtarın okunur hâline düşer (özel hal yok).
        w.actions.insert("Geri_Gonder_3".into(), act(None));
        assert_eq!(action_label(&w, "Geri_Gonder_3"), "Geri Gonder 3");
    }

    /// Hedef etiketi: kendi label'ı > node label'ı > anahtarın okunur hâli.
    #[test]
    fn send_back_target_label_prefers_its_own_text() {
        let w = wfd();
        assert_eq!(
            send_back_target_label(&w, "self__mudur", Some("Başa Gönder")),
            "Başa Gönder"
        );
        // Boş/whitespace label yok sayılır — node label'ına düşer.
        assert_eq!(
            send_back_target_label(&w, "self__mudur", Some("  ")),
            "Müdür"
        );
        assert_eq!(send_back_target_label(&w, "self__mudur", None), "Müdür");
        // Node'un da label'ı yoksa anahtar okunur hâle gelir.
        assert_eq!(send_back_target_label(&w, "self__gm", None), "self gm");
    }

    #[test]
    fn engine_markers_are_left_verbatim() {
        let w = wfd();
        assert_eq!(action_label(&w, "_branch_cancelled"), "_branch_cancelled");
    }

    #[test]
    fn node_label_falls_back_to_a_readable_key() {
        let w = wfd();
        assert_eq!(node_label(&w, "self__mudur"), "Müdür");
        assert_eq!(node_label(&w, "self__gm"), "self gm");
        assert_eq!(node_label(&w, "yok__olan"), "yok olan");
    }

    /// Terminal de kimlik/gösterim ayrımına uyar: `label` varsa o, yoksa id'nin
    /// okunur hâli — istemci ham id'yi hiç görmez.
    #[test]
    fn terminal_label_falls_back_to_a_readable_id() {
        let w = wfd();
        assert_eq!(terminal_label(&w, "onaylandi"), "Onaylandı");
        assert_eq!(terminal_label(&w, "reddedildi"), "reddedildi");
        assert_eq!(terminal_label(&w, "bilinmeyen_id"), "bilinmeyen id");
    }
}
