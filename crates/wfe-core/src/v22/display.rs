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
/// GLB (global aksiyon) artık burada ÖZEL HAL DEĞİLDİR: hedef aksiyon anahtarına
/// kodlanmadığı için (`Wft::Targets`) bölünecek bir anahtar yok. Hedefin etiketi
/// ayrı bir `Ref` olarak `node_label`'dan gelir.
pub fn action_label(wfd: &Wfd, action: &str) -> String {
    non_empty(wfd.actions.get(action).and_then(|a| a.label.as_ref()))
        .map(str::to_string)
        .unwrap_or_else(|| humanize_key(action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::wfd_v22::{ActionDef, InputDef, NodeDef};
    use serde_json::json;

    fn wfd() -> Wfd {
        let mut w: Wfd = serde_json::from_value(json!({
            "wfd_version": "2.2",
            "id": "x", "name": "X", "version": "1.0.0",
            "context": {"type": "object", "properties": {}},
            "nodes": {}, "start": [], "actions": {},
            "transitions": [],
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
        }
    }

    #[test]
    fn plain_action_uses_label_then_humanized_key() {
        let mut w = wfd();
        w.actions.insert("Onayla".into(), act(Some("Onayla ve gönder")));
        w.actions.insert("Geri_Cevir".into(), act(None));
        assert_eq!(action_label(&w, "Onayla"), "Onayla ve gönder");
        assert_eq!(action_label(&w, "Geri_Cevir"), "Geri Cevir");
        // Tanımsız aksiyon da okunur döner (katalogda olmayan anahtar).
        assert_eq!(action_label(&w, "Bir_Sey"), "Bir Sey");
    }

    /// GLB artık etikette özel hal değil: hedef anahtara kodlanmadığı için
    /// aksiyon etiketi taban aksiyonun kendi etiketidir, hedef AYRI bir Ref'tir.
    #[test]
    fn global_action_label_is_just_the_action_label() {
        let mut w = wfd();
        w.actions
            .insert("Geri_Gonder".into(), act(Some("Geri Gönder")));
        assert_eq!(action_label(&w, "Geri_Gonder"), "Geri Gönder");
        assert_eq!(node_label(&w, "self__mudur"), "Müdür");
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
