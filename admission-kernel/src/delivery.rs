//! Executable SDK frontier arithmetic, verified by Verus.
use vstd::prelude::*;
verus! {
/// Capacity remains charged until a contiguous receipt is confirmed.
pub fn next_sequence(accepted: u64, confirmed: u64, slots: u64) -> (r: Option<u64>)
    ensures
        r.is_some() == (confirmed <= accepted && accepted < u64::MAX
            && accepted - confirmed < slots),
        r.is_some() ==> r.unwrap() as int == accepted as int + 1
            && confirmed <= r.unwrap() && r.unwrap() - confirmed <= slots,
{
    if confirmed <= accepted && accepted < u64::MAX && accepted - confirmed < slots {
        Some(accepted + 1)
    } else { None }
}
/// Refuse replay, skips, unadmitted receipts and malformed states.
pub fn confirm_sequence(accepted: u64, confirmed: u64, receipt: u64) -> (r: Option<u64>)
    ensures
        r.is_some() == (confirmed < accepted && receipt as int == confirmed as int + 1),
        r.is_some() ==> r.unwrap() == receipt && confirmed < r.unwrap()
            && r.unwrap() <= accepted
            && accepted - r.unwrap() < accepted - confirmed,
{
    if confirmed < accepted && receipt == confirmed + 1 {
        Some(receipt)
    } else { None }
}
/// Completion is about the captured target, independent of subsequent admissions.
pub fn covered(confirmed: u64, target: u64) -> (r: bool)
    ensures r == (target <= confirmed),
{ target <= confirmed }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontier_matches_wider_arithmetic_at_boundaries() {
        let values = [0, 1, 2, 7, u64::MAX - 1, u64::MAX];
        for accepted in values {
            for confirmed in values {
                for slots in values {
                    let next = accepted as u128 + 1;
                    let expected = if confirmed <= accepted
                        && next <= u64::MAX as u128
                        && next - confirmed as u128 <= slots as u128
                    {
                        Some(next as u64)
                    } else {
                        None
                    };
                    assert_eq!(next_sequence(accepted, confirmed, slots), expected);
                }
                for receipt in values {
                    let expected =
                        if confirmed < accepted && receipt as u128 == confirmed as u128 + 1 {
                            Some(receipt)
                        } else {
                            None
                        };
                    assert_eq!(confirm_sequence(accepted, confirmed, receipt), expected);
                    assert_eq!(
                        covered(confirmed, receipt),
                        confirmed as u128 >= receipt as u128
                    );
                }
            }
        }
    }
}
