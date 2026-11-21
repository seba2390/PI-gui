//! Text formatting helpers for the views.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// `~/code/app` style display of a path.
pub fn short_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// "now", "5m", "3h", "2d", "4w" — compact like sidebar timestamps.
pub fn relative_time(then: SystemTime, now: SystemTime) -> String {
    let elapsed = now.duration_since(then).unwrap_or(Duration::ZERO).as_secs();
    match elapsed {
        0..60 => "now".to_owned(),
        60..3_600 => format!("{}m", elapsed / 60),
        3_600..86_400 => format!("{}h", elapsed / 3_600),
        86_400..604_800 => format!("{}d", elapsed / 86_400),
        _ => format!("{}w", elapsed / 604_800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths() {
        let home = Path::new("/home/u");
        assert_eq!(
            short_path(Path::new("/home/u/code/app"), Some(home)),
            "~/code/app"
        );
        assert_eq!(short_path(Path::new("/home/u"), Some(home)), "~");
        assert_eq!(short_path(Path::new("/srv/x"), Some(home)), "/srv/x");
    }

    #[test]
    fn relative_times() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let ago = |secs| now - Duration::from_secs(secs);
        assert_eq!(relative_time(ago(5), now), "now");
        assert_eq!(relative_time(ago(300), now), "5m");
        assert_eq!(relative_time(ago(7_200), now), "2h");
        assert_eq!(relative_time(ago(200_000), now), "2d");
        assert_eq!(relative_time(ago(2_000_000), now), "3w");
    }
}
