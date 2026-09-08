use super::actor::Actor;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfahEntry {
    pub seq: u32,
    pub action: String,
    pub actor: Actor,
    pub input: Option<Value>,
    pub applied_at: DateTime<Utc>,
    /// Ç2 (v2.3): satırın geçişten ÖNCEki node'u. HAREKET üreten satır (asıl aksiyon)
    /// commit'in from/to'sunu taşır; MARKER satırları `None` taşır.
    ///
    /// Ayrım çalışma zamanında marker ADINDAN türetilmez: satırı üreten kod ne
    /// yazacağını zaten bilir (Ç2, reddedilen Seçenek B = adapter'da ad listesi).
    /// Alan eklendiği için yeni bir marker üreticisi bu ikiliyi yazmadan DERLENEMEZ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_node: Option<String>,
    /// Ç2: geçişin hedef node'u. `None` = marker satırı ya da hedefi olmayan geçiş
    /// (terminal/failed/terminated; çok hedefli `ForkTo`'da hedefler
    /// `wf.wfe_branch`'te satır satır durur).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_node: Option<String>,
    /// Ç4: satırı yazan KOLUN kimliği = `BranchState::entry_node`. Kol nereye giderse
    /// gitsin (kol içi hareket, fork öncesine geri gönderme) etiket DEĞİŞMEZ.
    ///
    /// `None` TEK anlam taşır: *"bu satır bir kolda değil"* — fork öncesi satırlar,
    /// join sonrası satırlar, `_fork`/`_collapse`/`_join` marker'ları ve paralel
    /// olmayan WFE'lerin tüm satırları. Sentinel (`_main` vb.) KULLANILMAZ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_entry: Option<String>,
    /// E14: satırın yazıldığı TUR — aynı fork'a ikinci kez girilirse (geri gönderme
    /// sonrası join'e dönüş) birinci turun satırları ikinci turunkilerle aynı
    /// `branch_entry`'yi taşır; ayrım bu alandadır.
    ///
    /// **1'den başlar ve FORK BAŞINA sayar** — global bir sayaç DEĞİL. Değeri
    /// defterden türetilir: kolun giriş node'unu `input.branches`inde taşıyan `_fork`
    /// satırlarının sayısı (`pipeline::branch_round_of`). Kol tablosunda tur kolonu
    /// YOKTUR; tek girdi defterdir (`$valid` saflık sözleşmesi, §3.1).
    ///
    /// `branch_entry` ile BİRLİKTE null ya da birlikte doludur — bu bir DEĞİŞMEZDİR:
    /// `branch_entry` NULL ⇔ `branch_round` NULL (*"bu satır bir kolda değil"*).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_round: Option<u32>,
}

/// Append-only action history. push() returns a new Wfah — never mutates.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Wfah(pub Vec<WfahEntry>);

impl Wfah {
    pub fn empty() -> Self {
        Self(vec![])
    }

    /// Returns a new Wfah with the entry appended. seq = last_seq + 1.
    ///
    /// Ç2/Ç4/E14 alanları (`from_node`/`to_node`/`branch_entry`/`branch_round`) `None`
    /// kalır: bu kısayol
    /// TEST/geçmiş kurma yoludur, hareket üreten satırı KURAN yol değildir. Motorun
    /// gerçek üreticileri `WfahEntry` literali kurar ve alanları açıkça doldurur.
    pub fn push(&self, action: String, actor: Actor, input: Option<Value>) -> Self {
        let seq = self.0.last().map(|e| e.seq + 1).unwrap_or(1);
        let mut entries = self.0.clone();
        entries.push(WfahEntry {
            seq,
            action,
            actor,
            input,
            applied_at: Utc::now(),
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        });
        Self(entries)
    }

    /// Bu geçişte üretilmiş kayıtlar eklenmiş yeni bir Wfah. §7 pipeline'ı atomik
    /// olduğu için satırlar commit'e kadar yalnız bellekte durur; "aksiyon işlendi"
    /// anındaki defter böyle kurulur (bkz. `pipeline::Engine::apply`).
    pub fn extended(&self, entries: &[WfahEntry]) -> Self {
        let mut all = self.0.clone();
        all.extend_from_slice(entries);
        Self(all)
    }

    pub fn entries(&self) -> &[WfahEntry] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        }
    }

    #[test]
    fn push_increments_seq() {
        let wfah = Wfah::empty();
        let w1 = wfah.push("start".into(), actor(), None);
        let w2 = w1.push("approve".into(), actor(), None);
        assert_eq!(w1.entries()[0].seq, 1);
        assert_eq!(w2.entries()[1].seq, 2);
    }

    #[test]
    fn push_does_not_mutate_original() {
        let wfah = Wfah::empty();
        let _w1 = wfah.push("start".into(), actor(), None);
        assert_eq!(wfah.entries().len(), 0);
    }
}
