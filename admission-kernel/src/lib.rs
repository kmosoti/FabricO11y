#![forbid(unsafe_code)]
//! Checked arithmetic shared by the executable adapter and proof checkers.
use vstd::prelude::*;

verus! {
pub fn reserve(capacity: usize, used: usize, charge: usize) -> (result: Option<usize>)
    ensures
        match result {
            Some(next) => used <= capacity && charge <= capacity - used
                && next as int == used as int + charge as int && next <= capacity,
            None => used > capacity || charge > capacity - used,
        }
{
    if used <= capacity && charge <= capacity - used {
        Some(used + charge)
    } else {
        None
    }
}
}

#[cfg(test)]
mod tests {
    #[test]
    fn reservation_matches_wider_independent_arithmetic() {
        let values = [0, 1, 2, 7, usize::MAX / 2, usize::MAX - 1, usize::MAX];
        for capacity in values {
            for used in values {
                for charge in values {
                    let sum = used as u128 + charge as u128;
                    let expected = if used <= capacity && sum <= capacity as u128 {
                        Some(sum as usize)
                    } else {
                        None
                    };
                    assert_eq!(super::reserve(capacity, used, charge), expected);
                }
            }
        }
    }
}
