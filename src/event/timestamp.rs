//! [`timestamp_ms`]: how events and summaries write a point in time, as
//! whole milliseconds since the Unix epoch.

use std::time::SystemTime;

/// Converts `time` to milliseconds since the Unix epoch, dropping any
/// fraction of a millisecond.
///
/// A time before the epoch gives 0, and a time too large for a `u64` gives
/// `u64::MAX`, so this never fails.
pub fn timestamp_ms(time: SystemTime) -> u64 {
  u64::try_from(
    time
      .duration_since(SystemTime::UNIX_EPOCH)
      .unwrap_or_default()
      .as_millis(),
  )
  .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;

  type TestResult = Result<(), Box<dyn std::error::Error>>;

  fn after_epoch(duration: Duration) -> Result<SystemTime, &'static str> {
    SystemTime::UNIX_EPOCH
      .checked_add(duration)
      .ok_or("time not representable")
  }

  #[test]
  fn the_epoch_is_zero() {
    assert_eq!(timestamp_ms(SystemTime::UNIX_EPOCH), 0);
  }

  #[test]
  fn a_time_before_the_epoch_is_zero() -> TestResult {
    let before = SystemTime::UNIX_EPOCH
      .checked_sub(Duration::from_secs(1))
      .ok_or("time not representable")?;
    assert_eq!(timestamp_ms(before), 0);
    Ok(())
  }

  #[test]
  fn fractions_of_a_millisecond_are_dropped() -> TestResult {
    let time = after_epoch(Duration::new(1_700_000_000, 123_999_999))?;
    assert_eq!(timestamp_ms(time), 1_700_000_000_123);
    Ok(())
  }

  #[test]
  fn the_largest_u64_millisecond_is_kept() -> TestResult {
    let time = after_epoch(Duration::from_millis(u64::MAX))?;
    assert_eq!(timestamp_ms(time), u64::MAX);
    Ok(())
  }

  #[test]
  fn a_time_past_u64_milliseconds_is_u64_max() -> TestResult {
    let time = after_epoch(Duration::from_millis(u64::MAX))?
      .checked_add(Duration::from_millis(1))
      .ok_or("time not representable")?;
    assert_eq!(timestamp_ms(time), u64::MAX);
    Ok(())
  }
}
