//! Small, shared human formatting helpers.

use std::time::Duration;

/// `850ms`→`<1s`, `4s`, `1m 3s`, `2h 5m` — the compact style agent harnesses use.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs == 0 {
        return "<1s".to_owned();
    }
    let (hours, minutes, seconds) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds}s"),
        _ => format!("{hours}h {minutes}m"),
    }
}

/// `950`→`950`, `12_300`→`12.3k`, `1_048_576`→`1.0M`.
pub fn format_tokens(tokens: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let value = tokens as f64;
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}k", value / 1_000.0)
    } else {
        format!("{:.1}M", value / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(format_duration(Duration::from_millis(850)), "<1s");
        assert_eq!(format_duration(Duration::from_secs(4)), "4s");
        assert_eq!(format_duration(Duration::from_secs(63)), "1m 3s");
        assert_eq!(format_duration(Duration::from_secs(7_500)), "2h 5m");
    }

    #[test]
    fn tokens() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_300), "12.3k");
        assert_eq!(format_tokens(1_048_576), "1.0M");
    }
}
