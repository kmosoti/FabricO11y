//! Executable custody publication and reclamation predicates, checked by Verus.
use vstd::prelude::*;
verus! {
pub fn advance(current:u64, received:u64)->(r:Option<u64>)
    ensures r.is_some() == (current < u64::MAX && received as int == current as int + 1),
        r.is_some() ==> r.unwrap() == received && r.unwrap() > current,
{ if current < u64::MAX && received == current + 1 {Some(received)} else {None} }
pub fn reclaimable(authority:bool, origin:bool, confirmed:u64, record:u64)->(r:bool)
    ensures r == (authority && origin && record > 0 && record <= confirmed),
{ authority && origin && record > 0 && record <= confirmed }
pub fn retire(expired:bool, admitted:u64, forwarded:u64)->(r:bool)
    ensures r == (expired && admitted <= forwarded),
{ expired && admitted <= forwarded }
pub fn within(snapshot:u64, record:u64)->(r:bool)
    ensures r == (record > 0 && record <= snapshot),
{ record > 0 && record <= snapshot }
}
#[cfg(test)]
mod tests {
    #[test]
    fn boundary_oracle() {
        for a in [0, 1, 2, u64::MAX - 1, u64::MAX] {
            for b in [0, 1, 2, u64::MAX - 1, u64::MAX] {
                assert_eq!(
                    super::advance(a, b),
                    if b as u128 == a as u128 + 1 {
                        Some(b)
                    } else {
                        None
                    }
                );
                for x in [false, true] {
                    for y in [false, true] {
                        assert_eq!(
                            super::reclaimable(x, y, a, b),
                            x && y && (1..=a).contains(&b)
                        );
                    }
                }
                assert_eq!(super::within(a, b), (1..=a).contains(&b));
                assert_eq!(super::retire(true, a, b), a as u128 <= b as u128);
                assert!(!super::retire(false, a, b));
            }
        }
    }
}
