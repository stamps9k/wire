//! [`IpLimit`]: a limit on how many connections one client address may have
//! open at once.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::Mutex;

/// Counts the open connections from each client address, and refuses new
/// ones from an address that already has the most allowed.
///
/// Shared between the accept loop and the connection tasks through an
/// [`Arc`]. A connection is counted from the moment
/// [`try_acquire`](IpLimit::try_acquire) gives it an [`IpPermit`] until that
/// permit is dropped. An address stays in the map only while it has a
/// connection open, so the map never holds more addresses than there are
/// open connections.
///
/// Addresses are compared after [`IpAddr::to_canonical`], so an IPv4-mapped
/// IPv6 address shares its count with the plain IPv4 address.
pub struct IpLimit {
  /// The most connections allowed from one address at once.
  max: u32,
  /// The number of open connections from each address.
  counts: Mutex<HashMap<IpAddr, u32>>,
}

impl IpLimit {
  /// Creates a limit of `max` connections at once from each address.
  ///
  /// `max` should be at least 1. With 0, every connection is refused, and
  /// each refused address is left in the map with a count of 0.
  pub fn new(max: u32) -> Self {
    IpLimit {
      max,
      counts: Mutex::new(HashMap::new()),
    }
  }

  /// Counts a new connection from `ip`, if that address is under the limit.
  ///
  /// Returns an [`IpPermit`] that keeps the connection counted until it is
  /// dropped, so it must be held for as long as the connection is open.
  /// Returns `None`, and counts nothing, if the address already has the most
  /// connections allowed. Also returns `None` if the lock is poisoned, which
  /// takes a panic while it is held; nothing that holds it can panic.
  pub fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<IpPermit> {
    let Ok(mut map) = self.counts.lock() else {
      return None;
    };

    let count = map.entry(ip.to_canonical()).or_default();

    if *count < self.max {
      *count = count.checked_add(1)?;

      return Some(IpPermit {
        limit: self.clone(),
        ip: ip.to_canonical(),
      });
    }
    None
  }
}

/// One connection counted against its address, given out by
/// [`IpLimit::try_acquire`].
///
/// Dropping it takes the connection off the count. Holding it in the
/// connection's task means that happens when the connection ends, however
/// it ends.
pub struct IpPermit {
  /// The limit the connection is counted in.
  limit: Arc<IpLimit>,
  /// The address counted, in canonical form.
  ip: IpAddr,
}

impl Drop for IpPermit {
  /// Takes this connection off its address's count, and removes the address
  /// from the map when no connections from it are left open.
  fn drop(&mut self) {
    let Ok(mut map) = self.limit.counts.lock() else {
      return;
    };
    let Some(count) = map.get_mut(&self.ip.to_canonical()) else {
      return;
    };

    *count = count.saturating_sub(1);

    if *count == 0 {
      map.remove(&self.ip.to_canonical());
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::net::Ipv4Addr;
  use std::thread;

  const CLIENT: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 9);
  const OTHER: Ipv4Addr = Ipv4Addr::new(198, 51, 100, 7);

  fn limit(max: u32) -> Arc<IpLimit> {
    Arc::new(IpLimit::new(max))
  }

  /// The address as a plain IPv4 `IpAddr`.
  fn v4(ip: Ipv4Addr) -> IpAddr {
    IpAddr::V4(ip)
  }

  /// The same address in IPv4-mapped IPv6 form, `::ffff:a.b.c.d`.
  fn mapped(ip: Ipv4Addr) -> IpAddr {
    IpAddr::V6(ip.to_ipv6_mapped())
  }

  /// The count stored for `ip`, or `None` if the address is not in the map.
  fn count(limit: &IpLimit, ip: IpAddr) -> Option<u32> {
    limit.counts.lock().ok()?.get(&ip).copied()
  }

  /// The number of addresses in the map.
  fn addresses(limit: &IpLimit) -> Option<usize> {
    limit.counts.lock().ok().map(|map| map.len())
  }

  #[test]
  fn grants_up_to_the_limit() {
    let limit = limit(3);
    let permits: Vec<IpPermit> = (0..3)
      .filter_map(|_| limit.try_acquire(v4(CLIENT)))
      .collect();
    assert_eq!(permits.len(), 3);
    assert_eq!(count(&limit, v4(CLIENT)), Some(3));
  }

  #[test]
  fn refuses_once_the_limit_is_reached() {
    let limit = limit(2);
    let _first = limit.try_acquire(v4(CLIENT));
    let _second = limit.try_acquire(v4(CLIENT));
    assert!(limit.try_acquire(v4(CLIENT)).is_none());
  }

  #[test]
  fn a_refusal_does_not_change_the_count() {
    let limit = limit(2);
    let _first = limit.try_acquire(v4(CLIENT));
    let _second = limit.try_acquire(v4(CLIENT));
    let _refused = limit.try_acquire(v4(CLIENT));
    assert_eq!(count(&limit, v4(CLIENT)), Some(2));
  }

  #[test]
  fn dropping_a_permit_frees_its_slot() {
    let limit = limit(1);
    let first = limit.try_acquire(v4(CLIENT));
    assert!(first.is_some());
    assert!(limit.try_acquire(v4(CLIENT)).is_none());
    drop(first);
    assert!(limit.try_acquire(v4(CLIENT)).is_some());
  }

  #[test]
  fn an_address_is_removed_when_its_last_permit_is_dropped() {
    let limit = limit(2);
    let first = limit.try_acquire(v4(CLIENT));
    let second = limit.try_acquire(v4(CLIENT));
    drop(first);
    assert_eq!(count(&limit, v4(CLIENT)), Some(1));
    drop(second);
    assert_eq!(count(&limit, v4(CLIENT)), None);
    assert_eq!(addresses(&limit), Some(0));
  }

  #[test]
  fn addresses_are_counted_separately() {
    let limit = limit(1);
    let _client = limit.try_acquire(v4(CLIENT));
    assert!(limit.try_acquire(v4(OTHER)).is_some());
    assert!(limit.try_acquire(v4(CLIENT)).is_none());
  }

  #[test]
  fn a_mapped_address_shares_the_count_of_its_ipv4_address() {
    let limit = limit(1);
    let _plain = limit.try_acquire(v4(CLIENT));
    assert!(limit.try_acquire(mapped(CLIENT)).is_none());
  }

  #[test]
  fn a_mapped_address_is_counted_and_released_as_ipv4() {
    let limit = limit(1);
    let permit = limit.try_acquire(mapped(CLIENT));
    assert_eq!(count(&limit, v4(CLIENT)), Some(1));
    assert_eq!(count(&limit, mapped(CLIENT)), None);
    drop(permit);
    assert_eq!(addresses(&limit), Some(0));
  }

  #[test]
  fn the_map_is_empty_after_many_threads_acquire_and_release() {
    let limit = limit(4);
    thread::scope(|scope| {
      for _ in 0..8 {
        scope.spawn(|| {
          for _ in 0..1_000 {
            let permit = limit.try_acquire(v4(CLIENT));
            drop(permit);
          }
        });
      }
    });
    assert_eq!(addresses(&limit), Some(0));
  }
}
