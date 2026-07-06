//! ISO 8601 duration parser — escalation `after` ve root `timeout` için (M5/M6).
//! Desteklenen: PnW | PnDTnHnMnS alt kümeleri ("P30D", "P3D", "PT2H30M", "P1DT12H", "PT45S").

use crate::error::EngineError;
use chrono::Duration;

pub fn parse_iso8601_duration(s: &str) -> Result<Duration, EngineError> {
    let err = || EngineError::InvalidWfd(format!("geçersiz ISO 8601 duration: '{s}'"));
    let rest = s.strip_prefix('P').ok_or_else(err)?;
    if rest.is_empty() {
        return Err(err());
    }

    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    if let Some(t) = time_part {
        if t.is_empty() {
            return Err(err());
        }
    }

    let mut total = Duration::zero();
    let mut parse_segments = |part: &str, units: &[(char, i64)]| -> Result<(), EngineError> {
        let mut num = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                let unit_secs = units
                    .iter()
                    .find(|(u, _)| *u == c)
                    .map(|(_, secs)| *secs)
                    .ok_or_else(err)?;
                let n: i64 = num.parse().map_err(|_| err())?;
                total += Duration::seconds(n * unit_secs);
                num.clear();
            }
        }
        if !num.is_empty() {
            return Err(err());
        }
        Ok(())
    };

    parse_segments(date_part, &[('W', 7 * 86400), ('D', 86400)])?;
    if let Some(t) = time_part {
        parse_segments(t, &[('H', 3600), ('M', 60), ('S', 1)])?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_durations() {
        assert_eq!(parse_iso8601_duration("P3D").unwrap(), Duration::days(3));
        assert_eq!(parse_iso8601_duration("P30D").unwrap(), Duration::days(30));
        assert_eq!(parse_iso8601_duration("P1W").unwrap(), Duration::days(7));
        assert_eq!(parse_iso8601_duration("PT2H").unwrap(), Duration::hours(2));
        assert_eq!(
            parse_iso8601_duration("P1DT12H").unwrap(),
            Duration::days(1) + Duration::hours(12)
        );
        assert_eq!(
            parse_iso8601_duration("PT2H30M").unwrap(),
            Duration::hours(2) + Duration::minutes(30)
        );
        assert_eq!(parse_iso8601_duration("PT45S").unwrap(), Duration::seconds(45));
    }

    #[test]
    fn rejects_invalid() {
        assert!(parse_iso8601_duration("3D").is_err());
        assert!(parse_iso8601_duration("P").is_err());
        assert!(parse_iso8601_duration("PT").is_err());
        assert!(parse_iso8601_duration("P3X").is_err());
        assert!(parse_iso8601_duration("P3").is_err());
        assert!(parse_iso8601_duration("PT2H30").is_err());
    }
}
