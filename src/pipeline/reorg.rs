//! Fork / reorg helpers.
//!
//! A Solana slot number is not reused. An orphan is a slot we indexed that is
//! not an ancestor of the newly finalized tip. If the parent chain cannot be
//! walked back to the previous finalized slot, return `None` and do not delete.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotCommitment {
    Processed,
    Confirmed,
    Finalized,
    Dead,
}

impl SlotCommitment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Processed => "processed",
            Self::Confirmed => "confirmed",
            Self::Finalized => "finalized",
            Self::Dead => "dead",
        }
    }
}

/// Walk parents from `finalized` until `old_finalized` (inclusive).
///
/// `parents` maps slot -> parent. Returns `None` when the chain is incomplete
/// (missing parent before reaching `old_finalized`).
pub fn canonical_chain(
    finalized: u64,
    old_finalized: u64,
    parents: &HashMap<u64, u64>,
) -> Option<HashSet<u64>> {
    let mut chain = HashSet::new();
    let mut cur = finalized;
    loop {
        if !chain.insert(cur) {
            return None;
        }
        if cur <= old_finalized {
            return Some(chain);
        }
        cur = *parents.get(&cur)?;
        if chain.len() > 10_000 {
            return None;
        }
    }
}

/// Slots in `known` that are not on the canonical chain.
///
/// `known` should be slots in `(old_finalized, finalized]`.
pub fn orphans_if_chain_complete(
    finalized: u64,
    old_finalized: u64,
    parents: &HashMap<u64, u64>,
    known: &[u64],
) -> Option<Vec<u64>> {
    if finalized <= old_finalized {
        return Some(Vec::new());
    }
    let chain = canonical_chain(finalized, old_finalized, parents)?;
    let mut orphans: Vec<u64> = known
        .iter()
        .copied()
        .filter(|slot| *slot > old_finalized && *slot <= finalized && !chain.contains(slot))
        .collect();
    orphans.sort_unstable();
    orphans.dedup();
    Some(orphans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parents(pairs: &[(u64, u64)]) -> HashMap<u64, u64> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn no_orphans_on_straight_chain() {
        let parents = parents(&[(103, 102), (102, 101), (101, 100)]);
        let known = [101, 102, 103];
        let orphans = orphans_if_chain_complete(103, 100, &parents, &known).unwrap();
        assert!(orphans.is_empty());
    }

    #[test]
    fn skipped_sibling_slots_are_orphans() {
        // 101 and 102 confirmed on a fork; finalized 103 has parent 100.
        let parents = parents(&[(101, 100), (102, 101), (103, 100)]);
        let known = [101, 102, 103];
        let orphans = orphans_if_chain_complete(103, 100, &parents, &known).unwrap();
        assert_eq!(orphans, vec![101, 102]);
    }

    #[test]
    fn incomplete_parent_map_does_not_delete() {
        let parents = parents(&[(103, 102)]);
        let known = [101, 102, 103];
        assert!(orphans_if_chain_complete(103, 100, &parents, &known).is_none());
    }

    #[test]
    fn equal_or_older_finalized_is_noop() {
        let parents = HashMap::new();
        assert_eq!(
            orphans_if_chain_complete(100, 100, &parents, &[100]).unwrap(),
            Vec::<u64>::new()
        );
        assert_eq!(
            orphans_if_chain_complete(90, 100, &parents, &[90]).unwrap(),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn cycle_is_incomplete() {
        let parents = parents(&[(102, 101), (101, 102)]);
        assert!(canonical_chain(102, 0, &parents).is_none());
    }
}
