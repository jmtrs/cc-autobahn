//! Zulu timestamp → epoch-millis (no `chrono`).
//! Claude Code's fixed format: "2026-07-16T08:34:42.592Z"

/// Converts a Zulu timestamp to epoch-millis. `None` if the format doesn't match.
pub(crate) fn parse_zulu_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24 || b[23] != b'Z' {
        return None;
    }
    let n = |start: usize| -> Option<i64> {
        std::str::from_utf8(&b[start..start + 2])
            .ok()?
            .parse::<i64>()
            .ok()
    };
    let y: i64 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo = n(5)?;
    let d = n(8)?;
    let hh = n(11)?;
    let mi = n(14)?;
    let ss = n(17)?;
    let msec: i64 = std::str::from_utf8(&b[20..23]).ok()?.parse().ok()?;
    // Defensive range validation (Claude Code writes valid values, but an
    // out-of-range field would silently produce an incorrect epoch_ms → None).
    if !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&hh)
        || !(0..=59).contains(&mi)
        || !(0..=59).contains(&ss)
        || !(0..=999).contains(&msec)
    {
        return None;
    }
    let days = days_from_civil(y, mo as u64, d as u64);
    Some(days * 86_400_000 + hh * 3_600_000 + mi * 60_000 + ss * 1000 + msec)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm,
/// tested and branch-free). Proleptic Gregorian.
fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zulu_epoch_origin() {
        assert_eq!(parse_zulu_millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_zulu_millis("1970-01-01T00:00:01.000Z"), Some(1000));
        assert_eq!(
            parse_zulu_millis("1970-01-02T00:00:00.000Z"),
            Some(86_400_000)
        );
    }

    #[test]
    fn zulu_real_delta_matches_d8() {
        // The difference between the previous closure and the 3008-tok one (D8 case):
        // 1 min + 5 s + 278 ms = 65.278 s.
        let prev = parse_zulu_millis("2026-07-16T08:33:37.314Z").unwrap();
        let curr = parse_zulu_millis("2026-07-16T08:34:42.592Z").unwrap();
        assert_eq!(curr - prev, 65_278);
    }

    #[test]
    fn zulu_rejects_garbage() {
        assert_eq!(parse_zulu_millis("nope"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:34:42.592"), None); // missing Z
    }

    #[test]
    fn zulu_rejects_out_of_range() {
        // hour 24, minute 60, etc. → None (no silently incorrect epoch).
        assert_eq!(parse_zulu_millis("2026-07-16T24:00:00.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:60:00.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:00:60.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:00:00.9999Z"), None); // format
    }
}
