//! The update-state machine: `pts`, `qts`, `date`, `seq` and per-channel `pts`, and the rules that
//! decide whether an update is applied, skipped as a duplicate, or reveals a gap.
//!
//! Pure functions over plain values, so the rules are tested without a socket. The session asks
//! [`check_pts`] and [`check_seq`] before it applies an update, and fetches the difference on a
//! [`Check::Gap`].

use serde::{Deserialize, Serialize};

/// The verdict on one update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The update is the next one: apply it and advance the counter.
    Apply,
    /// Already seen: drop it.
    Duplicate,
    /// Something in between was missed: fetch the difference.
    Gap,
}

/// The common update state of the account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub pts: i32,
    pub qts: i32,
    pub date: i32,
    pub seq: i32,
}

/// Check a common or channel `pts`. `count` is the number of events the update carries
/// (`pts_count`). With `local == 0` there is no state for this counter and any update is taken.
pub fn check_pts(local: i32, pts: i32, count: i32) -> Check {
    if local == 0 {
        return Check::Apply;
    }
    let expected = local.saturating_add(count);
    if pts == expected {
        Check::Apply
    } else if pts < expected {
        Check::Duplicate
    } else {
        Check::Gap
    }
}

/// Check the container `seq` of an `updates` or `updatesCombined` object. `seq_start` is the first
/// sequence number the container covers (equal to `seq` for `updates`). A `seq` of zero means
/// the container is not sequenced and is always applied.
pub fn check_seq(local: i32, seq_start: i32, seq: i32) -> Check {
    if seq == 0 {
        return Check::Apply;
    }
    if seq_start == local.saturating_add(1) {
        Check::Apply
    } else if seq <= local {
        Check::Duplicate
    } else {
        Check::Gap
    }
}

/// Advance the common `pts` after an applied update.
pub fn advance_pts(position: &mut Position, pts: i32) {
    position.pts = position.pts.max(pts);
}

/// Advance `seq` after an applied container, and take its date.
pub fn advance_seq(position: &mut Position, seq: i32, date: i32) {
    if seq != 0 {
        position.seq = seq;
    }
    if date != 0 {
        position.date = date;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_pts_is_applied() {
        assert_eq!(check_pts(10, 11, 1), Check::Apply);
        assert_eq!(check_pts(10, 13, 3), Check::Apply);
    }

    #[test]
    fn old_pts_is_a_duplicate() {
        assert_eq!(check_pts(10, 10, 1), Check::Duplicate);
        assert_eq!(check_pts(10, 9, 1), Check::Duplicate);
        assert_eq!(check_pts(10, 12, 3), Check::Duplicate);
    }

    #[test]
    fn skipped_pts_is_a_gap() {
        assert_eq!(check_pts(10, 12, 1), Check::Gap);
        assert_eq!(check_pts(10, 20, 2), Check::Gap);
    }

    #[test]
    fn unknown_pts_takes_the_update() {
        assert_eq!(check_pts(0, 500, 1), Check::Apply);
    }

    #[test]
    fn seq_rules() {
        assert_eq!(check_seq(5, 6, 6), Check::Apply);
        assert_eq!(check_seq(5, 4, 5), Check::Duplicate);
        assert_eq!(check_seq(5, 8, 9), Check::Gap);
        assert_eq!(check_seq(5, 0, 0), Check::Apply);
    }

    #[test]
    fn advancing_never_moves_pts_back() {
        let mut position = Position {
            pts: 10,
            ..Position::default()
        };
        advance_pts(&mut position, 7);
        assert_eq!(position.pts, 10);
        advance_pts(&mut position, 12);
        assert_eq!(position.pts, 12);
        advance_seq(&mut position, 4, 1_700_000_000);
        assert_eq!((position.seq, position.date), (4, 1_700_000_000));
        advance_seq(&mut position, 0, 0);
        assert_eq!(position.seq, 4);
    }
}
