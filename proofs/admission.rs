use vstd::prelude::*;

verus! {

/// Independent executable candidate; not yet used by the Fabric queue.
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

fn main() {}
