use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct Orgtnt {
    pub orgtnt_id:  Uuid,
    pub name:       String,
    pub code:       String,
    pub is_active:  bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Orgt {
    pub orgt_id:     Uuid,
    pub orgtnt_id:   Uuid,
    pub name:        String,
    pub description: Option<String>,
    pub is_active:   bool,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Orgu {
    pub orgu_id:        Uuid,
    pub orgt_id:        Uuid,
    pub orgtnt_id:      Uuid,
    pub parent_orgu_id: Option<Uuid>,
    pub path:           String,
    pub orgu_type:      serde_json::Value,
    pub name:           String,
    pub metadata:       Option<serde_json::Value>,
    pub is_active:      bool,
    pub created_at:     DateTime<Utc>,
    pub updated_at:     DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct User {
    pub u_id:       Uuid,
    pub orgtnt_id:  Uuid,
    pub username:   String,
    pub full_name:  String,
    pub email:      Option<String>,
    pub is_active:  bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Role {
    pub r_id:         Uuid,
    pub orgtnt_id:    Uuid,
    pub name:         String,
    pub display_name: String,
    pub is_active:    bool,
    pub created_at:   DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserOrgu {
    pub u_orgu_id:  Uuid,
    pub orgtnt_id:  Uuid,
    pub u_id:       Uuid,
    pub orgu_id:    Uuid,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserRole {
    pub ur_id:       Uuid,
    pub orgtnt_id:   Uuid,
    pub u_id:        Uuid,
    pub r_id:        Uuid,
    pub role_name:   String,
    pub orgu_id:     Option<Uuid>,
    pub orgu_scope:  Option<String>,
    pub ur_type:     String,
    pub valid_from:  Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at:  DateTime<Utc>,
}

/// Minimal org unit view used by wfe-core via OrgPort.
#[derive(Debug, Clone, Serialize)]
pub struct OrgUnit {
    pub orgu_id:   Uuid,
    pub orgu_type: serde_json::Value,
    pub path:      String,
}

impl From<Orgu> for OrgUnit {
    fn from(o: Orgu) -> Self {
        Self { orgu_id: o.orgu_id, orgu_type: o.orgu_type, path: o.path }
    }
}

/// Madde 6: vekalet/delegasyon kaydı. `grantee` bir CandidateActor JSONB'dir
/// (kişi {c_u:[...]} veya havuz {c_orgu, c_r:[...]}); alıcı eşleşmesi wfe-core
/// matcher'ında yapılır. Koltuk = (seat_orgu_id, seat_role).
#[derive(Debug, Serialize, FromRow)]
pub struct Delegation {
    pub delegation_id:     Uuid,
    pub orgtnt_id:         Uuid,
    pub delegator_user_id: Uuid,
    pub seat_orgu_id:      Uuid,
    pub seat_role:         String,
    pub grantee:           serde_json::Value,
    pub valid_from:        DateTime<Utc>,
    pub valid_to:          DateTime<Utc>,
    pub active:            bool,
    pub created_by:        Uuid,
    pub created_at:        DateTime<Utc>,
}
