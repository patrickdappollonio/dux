//! How a duration is printed in text a user reads: whole seconds, never a
//! float with seven decimals and never a truncation that turns a short wait
//! into "0s".

use std::time::Duration;

/// `d` rounded to the nearest whole second, and never below one: a wait the
/// message is about lasted some time, so it does not read as "0s".
pub fn whole_seconds(d: Duration) -> u64 {
    let rounded = (d.as_millis() + 500) / 1000;
    u64::try_from(rounded).unwrap_or(u64::MAX).max(1)
}

/// [`whole_seconds`] with its unit spelled out: "1 second", "15 seconds".
pub fn seconds_phrase(d: Duration) -> String {
    match whole_seconds(d) {
        1 => "1 second".to_string(),
        n => format!("{n} seconds"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_to_whole_seconds_and_never_to_zero() {
        assert_eq!(whole_seconds(Duration::from_secs(15)), 15);
        assert_eq!(whole_seconds(Duration::from_millis(742)), 1);
        assert_eq!(whole_seconds(Duration::from_millis(150)), 1);
        assert_eq!(whole_seconds(Duration::ZERO), 1);
        assert_eq!(whole_seconds(Duration::from_millis(2499)), 2);
        assert_eq!(whole_seconds(Duration::from_millis(2500)), 3);
        assert_eq!(seconds_phrase(Duration::from_millis(150)), "1 second");
        assert_eq!(seconds_phrase(Duration::from_secs(10)), "10 seconds");
    }
}
