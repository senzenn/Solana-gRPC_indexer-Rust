//! Stable CPI instruction paths.
//!
//! Outer instruction `i` is `"i"`. A CPI from that instruction is `"i.j"`,
//! nested CPI `"i.j.k"`. Indices are 0-based, matching Solana's compiled
//! instruction list and `InnerInstructions.index`.
//!
//! Stack height (Solana v1.14.6+) reconstructs the tree. Missing height is
//! treated as 2: a direct child of the outer instruction.

use std::fmt;

/// 0-based path from an outer instruction down the CPI tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IxPath(Vec<u32>);

impl IxPath {
    pub fn root(outer: u32) -> Self {
        Self(vec![outer])
    }

    pub fn child(&self, index: u32) -> Self {
        let mut next = self.0.clone();
        next.push(index);
        Self(next)
    }

    pub fn outer(&self) -> u32 {
        self.0.first().copied().unwrap_or(0)
    }

    pub fn indices(&self) -> &[u32] {
        &self.0
    }
}

impl fmt::Display for IxPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for part in &self.0 {
            if !first {
                f.write_str(".")?;
            }
            first = false;
            write!(f, "{part}")?;
        }
        Ok(())
    }
}

/// Solana allows far fewer CPI levels. The bound keeps paths short when a
/// height is bad: without it each such instruction nests one level deeper.
const MAX_STACK_HEIGHT: u32 = 16;

/// Assign paths to inner instructions of one outer instruction.
///
/// `stack_heights[i]` is the invocation height (outer = 1, first CPI = 2).
/// `None` is treated as 2 so pre-v1.14.6 transactions still get stable
/// sibling indices under the outer instruction.
pub fn assign_inner_paths(outer_index: u32, stack_heights: &[Option<u32>]) -> Vec<IxPath> {
    let mut out = Vec::with_capacity(stack_heights.len());
    let mut stack: Vec<(IxPath, u32)> = vec![(IxPath::root(outer_index), 0)];

    for height_opt in stack_heights {
        let height =
            usize::try_from(height_opt.unwrap_or(2).clamp(2, MAX_STACK_HEIGHT)).unwrap_or(2);
        while stack.len() >= height {
            stack.pop();
        }
        if stack.is_empty() {
            stack.push((IxPath::root(outer_index), 0));
        }
        let Some(parent) = stack.last_mut() else {
            continue;
        };
        let child = parent.1;
        parent.1 = parent.1.saturating_add(1);
        let path = parent.0.child(child);
        out.push(path.clone());
        stack.push((path, 0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(outer: u32, heights: &[Option<u32>]) -> Vec<String> {
        assign_inner_paths(outer, heights)
            .into_iter()
            .map(|p| p.to_string())
            .collect()
    }

    #[test]
    fn root_displays_as_outer_index() {
        assert_eq!(IxPath::root(0).to_string(), "0");
        assert_eq!(IxPath::root(3).to_string(), "3");
        assert_eq!(IxPath::root(3).outer(), 3);
    }

    #[test]
    fn missing_heights_are_direct_children() {
        assert_eq!(paths(0, &[None, None]), vec!["0.0", "0.1"]);
    }

    #[test]
    fn nested_cpi_uses_stack_height() {
        assert_eq!(
            paths(0, &[Some(2), Some(3), Some(2)]),
            vec!["0.0", "0.0.0", "0.1"]
        );
    }

    #[test]
    fn deeper_nest_then_sibling_at_parent() {
        assert_eq!(
            paths(1, &[Some(2), Some(3), Some(4), Some(3)]),
            vec!["1.0", "1.0.0", "1.0.0.0", "1.0.1"]
        );
    }

    #[test]
    fn bad_heights_keep_paths_bounded() {
        let got = paths(0, &[Some(u32::MAX); 20]);
        assert_eq!(got[1], "0.0.0");
        assert_eq!(got[14], "0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0");
        assert_eq!(got[15], "0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1");
        assert_eq!(got[19], "0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.5");
    }

    #[test]
    fn empty_inners() {
        assert!(assign_inner_paths(0, &[]).is_empty());
    }
}
