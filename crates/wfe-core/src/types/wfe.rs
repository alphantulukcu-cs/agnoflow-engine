use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WfeStatus {
    Active,
    Terminal,
    Error,
    /// SLA ihlali (deadline / dwell terminate) veya ileride manuel iptal.
    /// Hata değil, başarılı bitiş de değil — ama aktif de değil (2026-07-16 SLA sözleşmesi).
    Terminated,
}
