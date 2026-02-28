/// Pure function: Get status string for hide_self setting
pub fn get_hide_self_status(hide_self: bool) -> &'static str {
    if hide_self {
        "ON"
    } else {
        "OFF"
    }
}

/// Pure function: Validate and clamp poll interval
pub fn clamp_poll_interval(interval: u64) -> u64 {
    interval.clamp(50, 10000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hide_self_status_on() {
        assert_eq!(get_hide_self_status(true), "ON");
    }

    #[test]
    fn hide_self_status_off() {
        assert_eq!(get_hide_self_status(false), "OFF");
    }

    #[test]
    fn clamp_below_min() {
        assert_eq!(clamp_poll_interval(10), 50);
    }

    #[test]
    fn clamp_above_max() {
        assert_eq!(clamp_poll_interval(20000), 10000);
    }

    #[test]
    fn clamp_within_range() {
        assert_eq!(clamp_poll_interval(500), 500);
    }

    #[test]
    fn clamp_at_min_boundary() {
        assert_eq!(clamp_poll_interval(50), 50);
    }

    #[test]
    fn clamp_at_max_boundary() {
        assert_eq!(clamp_poll_interval(10000), 10000);
    }

    #[test]
    fn clamp_zero() {
        assert_eq!(clamp_poll_interval(0), 50);
    }
}
