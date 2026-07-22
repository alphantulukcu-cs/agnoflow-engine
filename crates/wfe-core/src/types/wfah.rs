use super::actor::Actor;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfahEntry {
    pub seq: u32,
    pub action: String,
    pub actor: Actor,
    pub input: Option<Value>,
    pub applied_at: DateTime<Utc>,
}

/// Append-only action history. push() returns a new Wfah — never mutates.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Wfah(pub Vec<WfahEntry>);

impl Wfah {
    pub fn empty() -> Self {
        Self(vec![])
    }

    /// Returns a new Wfah with the entry appended. seq = last_seq + 1.
    pub fn push(&self, action: String, actor: Actor, input: Option<Value>) -> Self {
        let seq = self.0.last().map(|e| e.seq + 1).unwrap_or(1);
        let mut entries = self.0.clone();
        entries.push(WfahEntry {
            seq,
            action,
            actor,
            input,
            applied_at: Utc::now(),
        });
        Self(entries)
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
