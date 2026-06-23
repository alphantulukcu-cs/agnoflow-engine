use async_trait::async_trait;
use uuid::Uuid;
use wfe_core::{EngineError, WfdPort, types::wfd::WFD};

pub struct InlineWfdPort {
    wfd: WFD,
}

impl InlineWfdPort {
    pub fn new(wfd: WFD) -> Self {
        Self { wfd }
    }
}

#[async_trait]
impl WfdPort for InlineWfdPort {
    async fn fetch(&self, _wfd_id: Uuid, _version: u32) -> Result<WFD, EngineError> {
        Ok(self.wfd.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn minimal_wfd() -> WFD {
        WFD {
            id:            "test".into(),
            name:          "test".into(),
            version:       "1".into(),
            description:   None,
            context:       serde_json::json!({"type": "object", "properties": {}}),
            start:         vec![],
            actions:       HashMap::new(),
            terminals:     vec![],
            transitions:   vec![],
            listable:      vec![],
            terminal_when: "false".into(),
            extra:         HashMap::new(),
        }
    }

    #[tokio::test]
    async fn fetch_ignores_id_and_version() {
        let port    = InlineWfdPort::new(minimal_wfd());
        let fetched = port.fetch(Uuid::new_v4(), 999).await.unwrap();
        assert_eq!(fetched.id, "test");
    }
}
