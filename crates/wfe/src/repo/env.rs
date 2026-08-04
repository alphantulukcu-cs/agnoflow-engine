//! Ortam konfigürasyonu deposu (`$env`) — tasarım:
//! `docs/superpowers/specs/2026-08-04-env-config-design.md`.
//!
//! İki tablo: `wf.environment` (tenant düzeyinde ortam kaydı) ve `wf.wfd_env_var`
//! (mantıksal WFD başına değerler). Çözüm sırası **tam eşleşme > joker (`env_id IS NULL`)
//! > tanımsız**; tanımsız anahtar burada sessizce atlanır ve `wfe-core` tarafında ifade
//! çözülürken hataya dönüşür (bkz. `wfe_core::v22::env`).

use crate::db::crypto;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;
use wfe_core::v22::env::{EnvSet, EnvValue, RunEnv};

#[derive(Debug)]
pub struct EnvError(pub String);
impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for EnvError {}

impl From<sqlx::Error> for EnvError {
    fn from(e: sqlx::Error) -> Self {
        EnvError(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    pub label: Option<String>,
    pub is_default: bool,
}

/// Tenant'ın ortamları (ad sırasıyla).
pub async fn list_environments(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Environment>, EnvError> {
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, bool)>(
        "SELECT id, name, label, is_default FROM wf.environment \
         WHERE orgtnt_id = $1 ORDER BY name",
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Environment {
            id: r.0,
            name: r.1,
            label: r.2,
            is_default: r.3,
        })
        .collect())
}

/// Ortamı ADIYLA çözer; ad verilmezse tenant'ın varsayılanı.
///
/// Tenant'ın hiç ortamı yoksa `default` adında bir tane AÇILIR. Migration mevcut tenant'lar
/// için bunu seed eder, ama migration'dan SONRA oluşturulan bir tenant'ın ilk WFE'si aksi
/// hâlde `environment_id NOT NULL` yüzünden patlardı.
pub async fn resolve_environment(
    pool: &PgPool,
    orgtnt_id: Uuid,
    name: Option<&str>,
) -> Result<Environment, EnvError> {
    if let Some(name) = name {
        return sqlx::query_as::<_, (Uuid, String, Option<String>, bool)>(
            "SELECT id, name, label, is_default FROM wf.environment \
             WHERE orgtnt_id = $1 AND name = $2",
        )
        .bind(orgtnt_id)
        .bind(name)
        .fetch_optional(pool)
        .await?
        .map(|r| Environment {
            id: r.0,
            name: r.1,
            label: r.2,
            is_default: r.3,
        })
        .ok_or_else(|| EnvError(format!("bilinmeyen ortam: '{name}'")));
    }

    if let Some(r) = sqlx::query_as::<_, (Uuid, String, Option<String>, bool)>(
        "SELECT id, name, label, is_default FROM wf.environment \
         WHERE orgtnt_id = $1 AND is_default",
    )
    .bind(orgtnt_id)
    .fetch_optional(pool)
    .await?
    {
        return Ok(Environment {
            id: r.0,
            name: r.1,
            label: r.2,
            is_default: r.3,
        });
    }

    let r = sqlx::query_as::<_, (Uuid, String, Option<String>, bool)>(
        "INSERT INTO wf.environment (orgtnt_id, name, label, is_default) \
         VALUES ($1, 'default', 'Varsayılan', true) \
         ON CONFLICT (orgtnt_id, name) DO UPDATE SET label = EXCLUDED.label \
         RETURNING id, name, label, is_default",
    )
    .bind(orgtnt_id)
    .fetch_one(pool)
    .await?;
    Ok(Environment {
        id: r.0,
        name: r.1,
        label: r.2,
        is_default: r.3,
    })
}

/// Bir koşumun ortam değişkenlerini yükler ve çözer.
///
/// `include_secrets = false` → secret satırlar HİÇ yüklenmez (draft/`simulate` koşumu).
/// GitLab'ın "protected variable" kuralının karşılığı: taslak denemesi prod kimlik
/// bilgisiyle dış sisteme istek atamaz. Eksik secret, kullanan autoexec'i `$env.X tanımlı
/// değil` ile düşürür — boş string'le devam edip kimliksiz istek atmaktan iyidir.
pub async fn load_run_env(
    pool: &PgPool,
    project_id: Uuid,
    wfd_name: &str,
    env_id: Uuid,
    include_secrets: bool,
) -> Result<RunEnv, EnvError> {
    // Tam eşleşme joker'i EZER: sıralamada joker (env_id IS NULL) ÖNCE gelir, sonra gelen
    // tam eşleşme aynı anahtarı BTreeMap'te üzerine yazar.
    let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<Vec<u8>>, bool)>(
        "SELECT key, value_type, value, value_enc, is_secret \
           FROM wf.wfd_env_var \
          WHERE project_id = $1 AND wfd_name = $2 \
            AND (env_id IS NULL OR env_id = $3) \
            AND ($4 OR NOT is_secret) \
          ORDER BY (env_id IS NOT NULL)",
    )
    .bind(project_id)
    .bind(wfd_name)
    .bind(env_id)
    .bind(include_secrets)
    .fetch_all(pool)
    .await?;

    let mut vars: BTreeMap<String, EnvValue> = BTreeMap::new();
    for (key, value_type, value, value_enc, is_secret) in rows {
        let raw = if is_secret {
            match value_enc {
                Some(b) => crypto::decrypt(&b)
                    .map_err(|e| EnvError(format!("'{key}' secret'ı çözülemedi: {e}")))?,
                None => continue,
            }
        } else {
            value.unwrap_or_default()
        };
        let typed = typed_value(&value_type, &raw)
            .ok_or_else(|| EnvError(format!("'{key}' değeri {value_type} değil: '{raw}'")))?;
        vars.insert(
            key,
            if is_secret {
                EnvValue::secret(typed)
            } else {
                EnvValue::public(typed)
            },
        );
    }
    Ok(RunEnv::new(EnvSet::new(vars)))
}

/// `value_type` kolonuna göre metni tipli JSON'a çevirir. Tip kolonu, bir sayının ZEN'de
/// string olarak gelip `> 1000` karşılaştırmasını "Compare: Unsupported type" ile
/// patlatmasını önlemek için var.
fn typed_value(value_type: &str, raw: &str) -> Option<Value> {
    match value_type {
        "number" => raw.parse::<f64>().ok().and_then(|n| {
            if n.fract() == 0.0 && n.abs() < i64::MAX as f64 {
                Some(Value::from(n as i64))
            } else {
                serde_json::Number::from_f64(n).map(Value::Number)
            }
        }),
        "boolean" => match raw {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => Some(Value::String(raw.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_value_parses_by_declared_type() {
        assert_eq!(typed_value("string", "5000"), Some(Value::from("5000")));
        assert_eq!(typed_value("number", "5000"), Some(Value::from(5000)));
        assert_eq!(typed_value("number", "1.5"), Some(Value::from(1.5)));
        assert_eq!(typed_value("boolean", "true"), Some(Value::Bool(true)));
        // Bozuk değer sessizce string'e DÜŞMEZ — yükleme hatası olur.
        assert_eq!(typed_value("number", "abc"), None);
        assert_eq!(typed_value("boolean", "Evet"), None);
    }
}
