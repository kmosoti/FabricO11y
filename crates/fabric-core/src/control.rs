//! Control decisions ([ADR-0014]): what an administrative request means for a
//! Spindle enrollment, independent of where the control state is stored.
//!
//! [ADR-0014]: ../../../docs/decisions/ADR-0014-manage-nodes-through-server-control-state.md

/// The administrative status of one enrolled Spindle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Active,
    Paused,
    Revoked,
}

/// Why a control request is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRejection {
    /// Revocation is terminal: a revoked enrollment accepts no change.
    Revoked,
    /// The revision counter cannot advance past `u64::MAX`.
    RevisionExhausted,
    /// The enrollment name is outside `1-64` characters of `[A-Za-z0-9._-]`.
    InvalidName,
    /// The desired configuration is outside the server's shape limits.
    InvalidDesired,
}

/// Largest number of log paths a desired configuration may name.
pub const MAX_DESIRED_LOGS: usize = 16;
/// Largest accepted metric interval, in seconds.
pub const MAX_METRIC_INTERVAL_S: u64 = 3600;
/// Largest length of one desired log path, in bytes.
pub const MAX_DESIRED_PATH_BYTES: usize = 240;

/// The revision that follows `revision` after any accepted change.
pub const fn next_revision(revision: u64) -> Result<u64, ControlRejection> {
    match revision.checked_add(1) {
        Some(next) => Ok(next),
        None => Err(ControlRejection::RevisionExhausted),
    }
}

/// Pause, resume or revoke. Revocation is terminal.
pub const fn set_status(current: Status, target: Status) -> Result<Status, ControlRejection> {
    match current {
        Status::Revoked => Err(ControlRejection::Revoked),
        _ => Ok(target),
    }
}

/// Whether a new desired configuration may be stored for this enrollment.
pub const fn may_set_config(current: Status) -> Result<(), ControlRejection> {
    match current {
        Status::Revoked => Err(ControlRejection::Revoked),
        _ => Ok(()),
    }
}

/// Whether the enrollment's credential authenticates at all.
pub const fn authorizes(status: Status) -> bool {
    !matches!(status, Status::Revoked)
}

/// Whether the Spindle is told to stop collecting.
pub const fn is_paused(status: Status) -> bool {
    matches!(status, Status::Paused)
}

/// Enrollment names: 1 to 64 characters of `[A-Za-z0-9._-]`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Server-side shape checks on a desired configuration. The Spindle validates
/// again against its own local profile before activating anything.
pub fn check_desired<S: AsRef<str>>(
    metric_interval_s: u64,
    logs: &[S],
) -> Result<(), ControlRejection> {
    let bad_path = |p: &str| {
        !p.starts_with('/')
            || p.len() > MAX_DESIRED_PATH_BYTES
            || p.contains('\n')
            || p.contains('\0')
    };
    if metric_interval_s == 0
        || metric_interval_s > MAX_METRIC_INTERVAL_S
        || logs.len() > MAX_DESIRED_LOGS
        || logs.iter().any(|p| bad_path(p.as_ref()))
    {
        return Err(ControlRejection::InvalidDesired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    #[test]
    fn revocation_is_terminal_for_every_request() {
        for target in [Active, Paused, Revoked] {
            assert_eq!(set_status(Revoked, target), Err(ControlRejection::Revoked));
        }
        assert_eq!(may_set_config(Revoked), Err(ControlRejection::Revoked));
        assert!(!authorizes(Revoked));
    }

    #[test]
    fn active_and_paused_accept_every_status_change() {
        for current in [Active, Paused] {
            for target in [Active, Paused, Revoked] {
                assert_eq!(set_status(current, target), Ok(target));
            }
            assert_eq!(may_set_config(current), Ok(()));
            assert!(authorizes(current));
        }
        assert!(is_paused(Paused) && !is_paused(Active) && !is_paused(Revoked));
    }

    /// Counterexample kept from the extraction: at the base the revision was
    /// `before + 1`, which panics in a debug build at `u64::MAX`.
    #[test]
    fn the_last_revision_has_no_successor() {
        assert_eq!(next_revision(1), Ok(2));
        assert_eq!(
            next_revision(u64::MAX),
            Err(ControlRejection::RevisionExhausted)
        );
    }

    #[test]
    fn names_and_desired_configurations_follow_the_shape_limits() {
        assert!(valid_name("node-a.1_b"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(valid_name(&"x".repeat(64)) && !valid_name(&"x".repeat(65)));
        let none: [&str; 0] = [];
        assert_eq!(check_desired(15, &none), Ok(()));
        assert_eq!(
            check_desired(0, &none),
            Err(ControlRejection::InvalidDesired)
        );
        assert_eq!(check_desired(3600, &["/var/log/a"]), Ok(()));
        assert_eq!(
            check_desired(3601, &none),
            Err(ControlRejection::InvalidDesired)
        );
        assert_eq!(
            check_desired(15, &["relative"]),
            Err(ControlRejection::InvalidDesired)
        );
        assert_eq!(
            check_desired(15, &["/a\nb"]),
            Err(ControlRejection::InvalidDesired)
        );
        let seventeen = ["/l"; 17];
        assert_eq!(
            check_desired(15, &seventeen),
            Err(ControlRejection::InvalidDesired)
        );
    }
}
