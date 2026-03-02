/// Format elapsed seconds into a human-readable string.
///
/// Format rules:
/// - 0-59s: "Xs" (e.g., "45s")
/// - 1-59m: "Xm Ys" (e.g., "3m 27s")
/// - 1h+: "Xh Ym Zs" (e.g., "2h 15m 30s")
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let minutes = secs / 60;
        let remaining_secs = secs % 60;
        format!("{minutes}m {remaining_secs}s")
    } else {
        let hours = secs / 3600;
        let minutes = (secs % 3600) / 60;
        let remaining_secs = secs % 60;
        format!("{hours}h {minutes}m {remaining_secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn formats_seconds_only() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(1), "1s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn formats_minutes_and_seconds() {
        assert_eq!(format_duration(60), "1m 0s");
        assert_eq!(format_duration(61), "1m 1s");
        assert_eq!(format_duration(127), "2m 7s");
        assert_eq!(format_duration(3599), "59m 59s");
    }

    #[test]
    fn formats_hours_minutes_and_seconds() {
        assert_eq!(format_duration(3600), "1h 0m 0s");
        assert_eq!(format_duration(3601), "1h 0m 1s");
        assert_eq!(format_duration(7267), "2h 1m 7s");
        assert_eq!(format_duration(5430), "1h 30m 30s");
    }
}
