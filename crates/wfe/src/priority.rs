//! SLA öncelik formülü (1–10) — deadline'dan otomatik hesaplanır (SLA-3,
//! 2026-07-16 sözleşmesi). Kolon YOK: okuma anında hesaplanır (pool/liste
//! görünümleri). Priority start body'de verilmez — tamamen türetilir.

use chrono::{DateTime, Utc};

/// `deadline` NULL → 1. Aksi halde `elapsed / window` oranı [0,1]'e clamp'lenip
/// 1..=10 aralığına ölçeklenir (süresi geçen = 10).
pub fn compute_priority(
    created_at: DateTime<Utc>,
    deadline: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> i32 {
    let Some(deadline) = deadline else {
        return 1;
    };
    let window = (deadline - created_at).num_milliseconds();
    if window <= 0 {
        // Vade created_at'ta veya öncesinde — tükenmiş kabul edilir.
        return 10;
    }
    let elapsed = (now - created_at).num_milliseconds().max(0);
    let frac = (elapsed as f64 / window as f64).clamp(0.0, 1.0);
    (1 + (frac * 10.0).floor() as i32).clamp(1, 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn no_deadline_is_priority_one() {
        let now = Utc::now();
        assert_eq!(compute_priority(now, None, now), 1);
    }

    #[test]
    fn just_started_is_priority_one() {
        let created = Utc::now();
        let deadline = created + Duration::days(10);
        assert_eq!(compute_priority(created, Some(deadline), created), 1);
    }

    #[test]
    fn halfway_through_window_is_mid_priority() {
        let created = Utc::now();
        let deadline = created + Duration::days(10);
        let now = created + Duration::days(5); // frac = 0.5 → 1 + floor(5) = 6
        assert_eq!(compute_priority(created, Some(deadline), now), 6);
    }

    #[test]
    fn past_deadline_caps_at_ten() {
        let created = Utc::now();
        let deadline = created + Duration::days(10);
        let now = deadline + Duration::days(1);
        assert_eq!(compute_priority(created, Some(deadline), now), 10);
    }

    #[test]
    fn exactly_at_deadline_is_ten() {
        let created = Utc::now();
        let deadline = created + Duration::days(10);
        assert_eq!(compute_priority(created, Some(deadline), deadline), 10);
    }

    #[test]
    fn zero_or_negative_window_is_priority_ten() {
        let created = Utc::now();
        assert_eq!(compute_priority(created, Some(created), created), 10);
        assert_eq!(
            compute_priority(created, Some(created - Duration::days(1)), created),
            10
        );
    }

    #[test]
    fn priority_is_monotonic_in_elapsed_fraction() {
        let created = Utc::now();
        let deadline = created + Duration::days(10);
        let mut last = 0;
        for days in 0..=10 {
            let p = compute_priority(created, Some(deadline), created + Duration::days(days));
            assert!(p >= last, "priority azalmamalı: gün {days} → {p} < {last}");
            assert!((1..=10).contains(&p));
            last = p;
        }
    }
}
