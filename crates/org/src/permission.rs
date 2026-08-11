//! Tenant permission havuzunun SAF çekirdeği: etkin küme hesabı.
//!
//! Bu modülde I/O YOKTUR. `repo::permission` ham satırları çeker, karar burada
//! verilir — kural SQL'in `WHERE`'ine kaçarsa test edilemez hale gelir (bu repoda
//! DB'li test koşulmuyor).
//!
//! Kanonik semantik (`docs/superpowers/specs/2026-08-11-tenant-permission-rol-modeli-design.md` §4):
//!
//! ```text
//! etkin_rol(u) = { r : ∃ birim b → check_user_role(u, b, r) }
//! etkin_p(u)   = ⋃ rp(etkin_rol(u)) − up_excluded(u)
//! ```

use crate::models::Permission;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use uuid::Uuid;

/// `org.ur` satırı — kullanıcı→rol ataması.
#[derive(Debug, Clone)]
pub struct UrRow {
    /// `check_user_role` birim EŞİTLİĞİ ister; `None` hiçbir kapı açmaz.
    pub orgu_id: Option<Uuid>,
    pub r_id: Uuid,
    pub role_name: String,
    pub role_is_active: bool,
    pub ur_type: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

/// `org.orgu_r` satırı — birime rol grant'ı (kullanıcının ÜYE olduğu birimler).
#[derive(Debug, Clone)]
pub struct OrguRRow {
    pub orgu_id: Uuid,
    pub r_id: Uuid,
    pub role_name: String,
    pub role_is_active: bool,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

/// `org.rp` satırı — rol = permission grubu.
#[derive(Debug, Clone)]
pub struct RpRow {
    pub r_id: Uuid,
    pub p_id: Uuid,
}

/// `org.up` satırı — kişisel ıskarta (T‑A2).
#[derive(Debug, Clone)]
pub struct UpRow {
    pub p_id: Uuid,
    pub up_type: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

/// Etkin küme hesabının TÜM girdisi. Süzülmemiş gelir: `is_active` ve timeslice
/// kararlarını `effective_permissions` verir.
#[derive(Debug, Clone, Default)]
pub struct PermissionRows {
    pub ur: Vec<UrRow>,
    pub orgu_r: Vec<OrguRRow>,
    pub rp: Vec<RpRow>,
    pub up: Vec<UpRow>,
    pub perms: Vec<Permission>,
}

/// Kullanıcının sahip olduğu bir yetki + hangi rollerden geldiği.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectivePermission {
    pub p_id: Uuid,
    pub code: String,
    pub display_name: String,
    /// Provenance: "Ahmet neden 1043'e sahip?" — ıskarta koymadan önce gerekir.
    pub via_roles: Vec<String>,
}

/// Kullanıcının etkin permission kümesi.
///
/// Sıra `code` üzerinedir (deterministik çıktı; istemci sıralamak zorunda kalmasın).
pub fn effective_permissions(
    rows: &PermissionRows,
    now: DateTime<Utc>,
) -> Vec<EffectivePermission> {
    // 1. (birim, rol) ıskartaları. `check_user_role`'ün son NOT EXISTS'i gibi
    //    timeslice UYGULAMAZ — motorla aynı cevabı vermek için bilinçli.
    let role_excluded: HashSet<(Uuid, Uuid)> = rows
        .ur
        .iter()
        .filter(|r| r.ur_type == UR_EXCLUDED)
        .filter_map(|r| r.orgu_id.map(|b| (b, r.r_id)))
        .collect();

    // 2. Etkin roller: kullanıcının EN AZ BİR biriminde etkin olan her rol.
    let mut active_roles: HashMap<Uuid, &str> = HashMap::new();
    for r in &rows.ur {
        // `orgu_id IS NULL` satırı yetki üretmez: `check_user_role` birim eşitliği ister.
        let Some(unit) = r.orgu_id else { continue };
        if r.ur_type == UR_EXCLUDED
            || !r.role_is_active
            || role_excluded.contains(&(unit, r.r_id))
            || !in_window(now, r.valid_from, r.valid_until)
        {
            continue;
        }
        active_roles.insert(r.r_id, &r.role_name);
    }
    for r in &rows.orgu_r {
        if !r.role_is_active
            || role_excluded.contains(&(r.orgu_id, r.r_id))
            || !in_window(now, r.valid_from, r.valid_until)
        {
            continue;
        }
        active_roles.insert(r.r_id, &r.role_name);
    }

    // 3. Kişisel ıskartalar — BURADA timeslice geçerlidir (`org.up`, T‑A2).
    let perm_excluded: HashSet<Uuid> = rows
        .up
        .iter()
        .filter(|u| u.up_type == UR_EXCLUDED && in_window(now, u.valid_from, u.valid_until))
        .map(|u| u.p_id)
        .collect();

    let catalog: HashMap<Uuid, &Permission> = rows.perms.iter().map(|p| (p.p_id, p)).collect();

    // 4. Etkin rollerin permission'ları; aynı yetki birden çok rolden gelirse TEK
    //    satır, `via_roles` hepsini listeler (BTreeSet → sıralı, tekrarsız).
    let mut acc: HashMap<Uuid, (&Permission, BTreeSet<&str>)> = HashMap::new();
    for rp in &rows.rp {
        let Some(role_name) = active_roles.get(&rp.r_id) else { continue };
        if perm_excluded.contains(&rp.p_id) {
            continue;
        }
        // Kataloğu olmayan satır sessizce atlanır (yarış / eski veri) — patlamaz.
        let Some(p) = catalog.get(&rp.p_id).filter(|p| p.is_active) else { continue };
        acc.entry(rp.p_id)
            .or_insert_with(|| (p, BTreeSet::new()))
            .1
            .insert(role_name);
    }

    let mut out: Vec<EffectivePermission> = acc
        .into_values()
        .map(|(p, via)| EffectivePermission {
            p_id: p.p_id,
            code: p.code.clone(),
            display_name: p.display_name.clone(),
            via_roles: via.into_iter().map(String::from).collect(),
        })
        .collect();
    out.sort_by(|a, b| a.code.cmp(&b.code));
    out
}

/// `POST /ext/permissions/check` yanıtı.
///
/// `unknown` TEŞHİStir, yetki cevabı değil: havuzda hiç olmayan bir kod hem
/// `denied`da hem `unknown`da görünür. Bilinmeyen kodu hata saymıyoruz (tenant
/// henüz tanımlamamış olabilir) ama sessiz de bırakmıyoruz — aksi halde dış
/// uygulamadaki yazım hatası "yetki yok" gibi okunurdu.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct CheckResult {
    pub granted: Vec<String>,
    pub denied: Vec<String>,
    pub unknown: Vec<String>,
}

/// İstenen kodları etkin kümeye karşı sınar.
///
/// Karşılaştırma harf duyarsızdır (`p_code_unique` indeksi de `lower(code)`
/// üzerinde). Yanıt ÇAĞIRANIN yazımını yankılar: istemci gönderdiği diziyle
/// eşleştirme yapabilsin.
pub fn check_codes(
    effective: &[EffectivePermission],
    catalog: &[Permission],
    requested: &[String],
) -> CheckResult {
    let granted_keys: HashSet<String> = effective.iter().map(|p| fold_code(&p.code)).collect();
    let catalog_keys: HashSet<String> = catalog.iter().map(|p| fold_code(&p.code)).collect();

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = CheckResult::default();
    for code in requested {
        let key = fold_code(code);
        if !seen.insert(key.clone()) {
            continue;
        }
        if granted_keys.contains(&key) {
            out.granted.push(code.clone());
        } else {
            out.denied.push(code.clone());
            if !catalog_keys.contains(&key) {
                out.unknown.push(code.clone());
            }
        }
    }
    out
}

/// Kod karşılaştırma anahtarı. `code` ASCII ile sınırlı olduğu için (`p_code_format`)
/// bu katlama PostgreSQL'in `lower()`'ı ile AYNI sonucu verir — Türkçe `İ`/`ı`
/// serbest bırakılsaydı ikisi ayrışırdı (`libc towlower` noktayı düşürür,
/// `str::to_lowercase` birleştirici nokta bırakır) ve havuzda benzersiz sayılan iki
/// kod burada farklı görünürdü.
fn fold_code(code: &str) -> String {
    code.to_ascii_lowercase()
}

/// `org.ur` / `org.up`'nin ıskarta işareti (aynı sözcük, iki tabloda).
const UR_EXCLUDED: &str = "excluded";

fn in_window(
    now: DateTime<Utc>,
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> bool {
    from.is_none_or(|f| f <= now) && until.is_none_or(|u| u > now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap()
    }

    fn past() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    fn future() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()
    }

    fn perm(p_id: Uuid, code: &str) -> Permission {
        Permission {
            p_id,
            orgtnt_id: Uuid::nil(),
            code: code.into(),
            display_name: format!("{code} adı"),
            description: None,
            is_active: true,
            created_at: past(),
            updated_at: past(),
        }
    }

    fn ur(orgu_id: Option<Uuid>, r_id: Uuid, role: &str) -> UrRow {
        UrRow {
            orgu_id,
            r_id,
            role_name: role.into(),
            role_is_active: true,
            ur_type: "granted".into(),
            valid_from: None,
            valid_until: None,
        }
    }

    fn excluded_ur(orgu_id: Uuid, r_id: Uuid, role: &str) -> UrRow {
        UrRow {
            ur_type: "excluded".into(),
            ..ur(Some(orgu_id), r_id, role)
        }
    }

    fn orgu_r(orgu_id: Uuid, r_id: Uuid, role: &str) -> OrguRRow {
        OrguRRow {
            orgu_id,
            r_id,
            role_name: role.into(),
            role_is_active: true,
            valid_from: None,
            valid_until: None,
        }
    }

    fn codes(out: &[EffectivePermission]) -> Vec<&str> {
        out.iter().map(|p| p.code.as_str()).collect()
    }

    /// 1. Rol tek birimde etkin → permission'ları etkin kümede.
    #[test]
    fn granted_role_yields_its_permissions() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r, "memur")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert_eq!(codes(&effective_permissions(&rows, now())), vec!["1043"]);
    }

    /// 2. Birim A'da `excluded`, birim B'de grant → permission VAR (kapsam birimdir).
    #[test]
    fn exclusion_in_one_unit_does_not_kill_grant_in_another() {
        let (a, b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![
                excluded_ur(a, r, "memur"),
                ur(Some(b), r, "memur"),
            ],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert_eq!(codes(&effective_permissions(&rows, now())), vec!["1043"]);
    }

    /// 3. Aynı birimde grant + `excluded` → permission YOK.
    #[test]
    fn exclusion_overrides_grant_in_same_unit() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r, "memur"), excluded_ur(b, r, "memur")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 4. `orgu_r` birim devralması → permission etkin.
    #[test]
    fn unit_role_grant_is_inherited() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            orgu_r: vec![orgu_r(b, r, "sube_yetkilisi")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert_eq!(codes(&effective_permissions(&rows, now())), vec!["1043"]);
    }

    /// Aynı birimdeki `excluded` satırı BİRİM devralmasını da ezer
    /// (`check_user_role`'ün son NOT EXISTS'i her iki kanalı da kapatır).
    #[test]
    fn exclusion_overrides_inherited_unit_role() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![excluded_ur(b, r, "sube_yetkilisi")],
            orgu_r: vec![orgu_r(b, r, "sube_yetkilisi")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 5. `org.ur.orgu_id IS NULL` satırı yetki ÜRETMEZ (`check_user_role` birim
    /// eşitliği ister; burada grant saymak kimsenin niyet etmediği yetki dağıtırdı).
    #[test]
    fn ur_row_without_unit_grants_nothing() {
        let (r, p) = (Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(None, r, "memur")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 7. Süresi geçmiş `ur` sayılmaz.
    #[test]
    fn expired_ur_row_grants_nothing() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![UrRow {
                valid_until: Some(past()),
                ..ur(Some(b), r, "memur")
            }],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 7b. Henüz başlamamış `ur` sayılmaz.
    #[test]
    fn not_yet_valid_ur_row_grants_nothing() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![UrRow {
                valid_from: Some(future()),
                ..ur(Some(b), r, "memur")
            }],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 7c. Süresi geçmiş `orgu_r` sayılmaz.
    #[test]
    fn expired_unit_role_grant_yields_nothing() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            orgu_r: vec![OrguRRow {
                valid_until: Some(past()),
                ..orgu_r(b, r, "sube_yetkilisi")
            }],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// MOTOR PARİTESİ: `check_user_role`'ün son NOT EXISTS'i `excluded` satırına
    /// timeslice UYGULAMAZ — süresi geçmiş bir ıskarta rolü yine kapatır. Motorla
    /// aynı cevabı vermek, "portal yetki veriyor ama node açılmıyor" çelişkisinden
    /// daha önemli. (`org.up` ıskartası bundan AYRI: orada timeslice geçerlidir.)
    #[test]
    fn expired_role_exclusion_still_blocks_matching_engine() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![
                ur(Some(b), r, "memur"),
                UrRow {
                    valid_until: Some(past()),
                    ..excluded_ur(b, r, "memur")
                },
            ],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 8. `p.is_active=false` → etkin kümede yok.
    #[test]
    fn inactive_permission_is_excluded() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r, "memur")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![Permission {
                is_active: false,
                ..perm(p, "1043")
            }],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 9. `r.is_active=false` → o rolün permission'ları yok.
    #[test]
    fn inactive_role_grants_nothing() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![UrRow {
                role_is_active: false,
                ..ur(Some(b), r, "memur")
            }],
            rp: vec![RpRow { r_id: r, p_id: p }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 10. Kişisel ıskarta İKİ rolden gelen aynı permission'ı TAMAMEN kaldırır.
    #[test]
    fn personal_exception_removes_permission_from_all_roles() {
        let (b, r1, r2, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r1, "memur"), ur(Some(b), r2, "sef")],
            rp: vec![
                RpRow { r_id: r1, p_id: p },
                RpRow { r_id: r2, p_id: p },
            ],
            up: vec![UpRow {
                p_id: p,
                up_type: "excluded".into(),
                valid_from: None,
                valid_until: None,
            }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }

    /// 11. Süresi geçmiş ıskarta → permission geri gelir.
    #[test]
    fn expired_personal_exception_restores_permission() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r, "memur")],
            rp: vec![RpRow { r_id: r, p_id: p }],
            up: vec![UpRow {
                p_id: p,
                up_type: "excluded".into(),
                valid_from: None,
                valid_until: Some(past()),
            }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert_eq!(codes(&effective_permissions(&rows, now())), vec!["1043"]);
    }

    /// 12. İki rol aynı permission'ı verir → tek satır, `via_roles` İKİSİNİ listeler.
    #[test]
    fn duplicate_permission_reports_all_source_roles_once() {
        let (b, r1, r2, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r1, "sef"), ur(Some(b), r2, "memur")],
            rp: vec![
                RpRow { r_id: r1, p_id: p },
                RpRow { r_id: r2, p_id: p },
            ],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        let out = effective_permissions(&rows, now());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].via_roles, vec!["memur".to_string(), "sef".to_string()]);
    }

    /// Çıktı `code`'a göre sıralıdır — istemci tarafında sıralama gerekmesin ve
    /// testler HashMap yineleme sırasına bağlı olmasın.
    #[test]
    fn output_is_sorted_by_code() {
        let (b, r) = (Uuid::new_v4(), Uuid::new_v4());
        let (p1, p2, p3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r, "memur")],
            rp: vec![
                RpRow { r_id: r, p_id: p1 },
                RpRow { r_id: r, p_id: p2 },
                RpRow { r_id: r, p_id: p3 },
            ],
            perms: vec![perm(p1, "9000"), perm(p2, "1043"), perm(p3, "KREDI_ONAY")],
            ..Default::default()
        };
        assert_eq!(
            codes(&effective_permissions(&rows, now())),
            vec!["1043", "9000", "KREDI_ONAY"]
        );
    }

    // ── check_codes ───────────────────────────────────────────────────────────

    fn eff(code: &str) -> EffectivePermission {
        EffectivePermission {
            p_id: Uuid::new_v4(),
            code: code.into(),
            display_name: code.into(),
            via_roles: vec!["memur".into()],
        }
    }

    fn req(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    /// Etkin kümedeki kod `granted`.
    #[test]
    fn check_grants_code_in_effective_set() {
        let out = check_codes(
            &[eff("1043")],
            &[perm(Uuid::new_v4(), "1043")],
            &req(&["1043"]),
        );
        assert_eq!(out.granted, req(&["1043"]));
        assert!(out.denied.is_empty());
        assert!(out.unknown.is_empty());
    }

    /// Havuzda VAR ama kullanıcıda yok → `denied`, `unknown` DEĞİL.
    #[test]
    fn check_denies_known_code_user_lacks() {
        let out = check_codes(&[], &[perm(Uuid::new_v4(), "1043")], &req(&["1043"]));
        assert_eq!(out.denied, req(&["1043"]));
        assert!(out.granted.is_empty());
        assert!(out.unknown.is_empty(), "havuzda tanımlı kod unknown değildir");
    }

    /// Havuzda HİÇ olmayan kod hem `denied` hem `unknown`.
    #[test]
    fn check_reports_code_missing_from_catalog_as_unknown_and_denied() {
        let out = check_codes(&[], &[perm(Uuid::new_v4(), "1043")], &req(&["1O43"]));
        assert_eq!(out.denied, req(&["1O43"]));
        assert_eq!(out.unknown, req(&["1O43"]));
    }

    /// Karşılaştırma harf duyarsız (`p_code_unique` indeksi de lower(code) üzerinde).
    #[test]
    fn check_matches_codes_case_insensitively() {
        let out = check_codes(
            &[eff("KREDI_ONAY")],
            &[perm(Uuid::new_v4(), "KREDI_ONAY")],
            &req(&["kredi_onay"]),
        );
        assert_eq!(
            out.granted,
            req(&["kredi_onay"]),
            "yanıt ÇAĞIRANIN yazımını yankılar"
        );
    }

    /// Tekrarlı istek tek satıra iner; ilk yazım korunur.
    #[test]
    fn check_deduplicates_repeated_codes() {
        let out = check_codes(
            &[eff("1043")],
            &[perm(Uuid::new_v4(), "1043")],
            &req(&["1043", "1043"]),
        );
        assert_eq!(out.granted, req(&["1043"]));
    }

    /// Boş istek boş yanıt (hata değil).
    #[test]
    fn check_with_no_codes_returns_empty_result() {
        assert_eq!(check_codes(&[eff("1043")], &[], &[]), CheckResult::default());
    }

    /// İstek sırası korunur — istemci kendi dizisiyle hizalayabilsin.
    #[test]
    fn check_preserves_request_order() {
        let catalog = vec![
            perm(Uuid::new_v4(), "a"),
            perm(Uuid::new_v4(), "b"),
            perm(Uuid::new_v4(), "c"),
        ];
        let out = check_codes(&[eff("c"), eff("a")], &catalog, &req(&["c", "b", "a"]));
        assert_eq!(out.granted, req(&["c", "a"]));
        assert_eq!(out.denied, req(&["b"]));
    }

    /// Kataloğu olmayan `rp` satırı sessizce atlanır (yarış/eski veri) — patlamaz.
    #[test]
    fn rp_row_without_catalog_entry_is_skipped() {
        let (b, r, p) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let rows = PermissionRows {
            ur: vec![ur(Some(b), r, "memur")],
            rp: vec![RpRow {
                r_id: r,
                p_id: Uuid::new_v4(),
            }],
            perms: vec![perm(p, "1043")],
            ..Default::default()
        };
        assert!(effective_permissions(&rows, now()).is_empty());
    }
}
