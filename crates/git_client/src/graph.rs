//! Lane layout for drawing the history as a git graph.
//!
//! Pure function over commits in display order (children before parents):
//! each commit gets a lane (column) and, for its row, the line segments
//! entering the node from above and leaving it downwards. Lanes are reused
//! once free but never compacted, so lines only move sideways at forks and
//! merges.

use std::collections::HashMap;

use crate::CommitInfo;

/// A line inside one half of a row, from lane `from` (top edge, or the node
/// for the bottom half) to lane `to` (the node for the top half, or the
/// bottom edge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of the commit's node.
    pub lane: usize,
    /// Colour index of the node (index into a palette, cycled).
    pub color: usize,
    /// Upper half: top edge → middle (lanes ending at the node converge on
    /// it, the others pass straight through).
    pub top: Vec<Seg>,
    /// Lower half: middle → bottom edge (node to its parents' lanes, plus the
    /// lanes passing through).
    pub bottom: Vec<Seg>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Graph {
    pub rows: Vec<GraphRow>,
    /// Most lanes any row uses.
    pub max_lanes: usize,
}

/// Lays out `commits` (as returned by `GitRepo::log`).
pub fn layout(commits: &[CommitInfo]) -> Graph {
    // Commits are compared by integer id: their index, or an id past the
    // end for parents outside the list (cut off by the log limit).
    let mut ids: HashMap<&str, usize> = commits.iter().enumerate().map(|(i, c)| (c.sha.as_str(), i)).collect();
    let parents: Vec<Vec<usize>> = commits
        .iter()
        .map(|c| {
            c.parents
                .iter()
                .map(|p| {
                    let next = ids.len();
                    *ids.entry(p.as_str()).or_insert(next)
                })
                .collect()
        })
        .collect();
    // Each lane waits for a commit id and carries a colour.
    let mut lanes: Vec<Option<(usize, usize)>> = Vec::new();
    let mut next_color = 0;
    let mut rows = Vec::with_capacity(commits.len());
    let mut max_lanes = 0;
    let free_slot = |lanes: &mut Vec<Option<(usize, usize)>>| match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    };
    let mut new_color = || {
        let c = next_color;
        next_color += 1;
        c
    };
    let mut waiting = Vec::new();
    let mut from_node = Vec::new();
    for (id, parent_ids) in parents.iter().enumerate() {
        waiting.clear();
        waiting.extend(
            lanes
                .iter()
                .enumerate()
                .filter(|(_, l)| l.is_some_and(|(w, _)| w == id))
                .map(|(i, _)| i),
        );
        let (lane, color) = match waiting.first().and_then(|&i| lanes[i].map(|(_, c)| (i, c))) {
            Some(found) => found,
            None => {
                let i = free_slot(&mut lanes);
                (i, new_color())
            }
        };
        let mut row = GraphRow {
            lane,
            color,
            ..Default::default()
        };
        for (i, l) in lanes.iter().enumerate() {
            if let Some((_, c)) = l {
                let to = if waiting.contains(&i) { lane } else { i };
                row.top.push(Seg { from: i, to, color: *c });
            }
        }
        for &i in &waiting {
            lanes[i] = None;
        }
        // Parents: the first keeps the node's lane and colour unless another
        // lane already waits for it.
        from_node.clear();
        for (p, &parent) in parent_ids.iter().enumerate() {
            let waiting_lane = lanes
                .iter()
                .enumerate()
                .find_map(|(i, l)| l.filter(|&(w, _)| w == parent).map(|(_, c)| (i, c)));
            match waiting_lane {
                Some((i, color)) => from_node.push(Seg {
                    from: lane,
                    to: i,
                    color,
                }),
                None => {
                    let (i, c) = if p == 0 && lanes[lane].is_none() {
                        (lane, color)
                    } else {
                        let i = free_slot(&mut lanes);
                        (i, new_color())
                    };
                    lanes[i] = Some((parent, c));
                    from_node.push(Seg {
                        from: lane,
                        to: i,
                        color: c,
                    });
                }
            }
        }
        for (i, l) in lanes.iter().enumerate() {
            if let Some((_, c)) = l {
                if !from_node.iter().any(|s| s.to == i) {
                    row.bottom.push(Seg {
                        from: i,
                        to: i,
                        color: *c,
                    });
                }
            }
        }
        row.bottom.extend_from_slice(&from_node);
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let used = row
            .top
            .iter()
            .chain(&row.bottom)
            .flat_map(|s| [s.from, s.to])
            .chain([lane])
            .max()
            .unwrap_or(0)
            + 1;
        max_lanes = max_lanes.max(used);
        rows.push(row);
    }
    Graph { rows, max_lanes }
}

/// Lane of each commit by sha (helper for callers and tests).
pub fn lanes_by_sha(commits: &[CommitInfo], graph: &Graph) -> HashMap<String, usize> {
    commits
        .iter()
        .zip(&graph.rows)
        .map(|(c, r)| (c.sha.clone(), r.lane))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commits from (sha, parents) in display order.
    fn h(spec: &[(&str, &[&str])]) -> Vec<CommitInfo> {
        spec.iter()
            .map(|(sha, parents)| CommitInfo {
                sha: sha.to_string(),
                parents: parents.iter().map(std::string::ToString::to_string).collect(),
                ..Default::default()
            })
            .collect()
    }

    fn crosses(segs: &[Seg]) -> usize {
        segs.iter().filter(|s| s.from != s.to).count()
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let c = h(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        let g = layout(&c);
        assert_eq!(g.max_lanes, 1);
        assert!(g
            .rows
            .iter()
            .all(|r| r.lane == 0 && crosses(&r.top) == 0 && crosses(&r.bottom) == 0));
        assert!(
            g.rows.iter().all(|r| r.color == g.rows[0].color),
            "colour kept along the branch"
        );
        assert!(g.rows[2].bottom.is_empty(), "root has no line below");
        assert!(g.rows[0].top.is_empty(), "tip has no line above");
    }

    #[test]
    fn branch_and_merge() {
        // m merges f2 into main; main: m - b - a ; feature: f2 - f1 - a
        let c = h(&[
            ("m", &["b", "f2"]),
            ("f2", &["f1"]),
            ("b", &["a"]),
            ("f1", &["a"]),
            ("a", &[]),
        ]);
        let g = layout(&c);
        let lanes = lanes_by_sha(&c, &g);
        assert_eq!(g.max_lanes, 2);
        assert_eq!(g.rows[0].bottom.len(), 2, "merge has two lines to its parents");
        assert_eq!(crosses(&g.rows[0].bottom), 1);
        assert_ne!(lanes["f2"], lanes["b"]);
        assert_eq!(lanes["f1"], lanes["f2"]);
        assert_eq!(lanes["a"], lanes["b"], "main keeps its lane");
        // The feature lane joins main's right below f1 (git's `|/`), so
        // only one line reaches the root.
        assert!(
            g.rows[3].bottom.contains(&Seg {
                from: lanes["f1"],
                to: lanes["a"],
                color: g.rows[2].color
            }),
            "{:?}",
            g.rows[3]
        );
        assert_eq!(g.rows[4].top.len(), 1);
        assert_ne!(g.rows[1].color, g.rows[0].color);
    }

    #[test]
    fn parallel_branches_without_merge() {
        let c = h(&[("x2", &["x1"]), ("y1", &["r"]), ("x1", &["r"]), ("r", &[])]);
        let g = layout(&c);
        let lanes = lanes_by_sha(&c, &g);
        assert_eq!(g.max_lanes, 2);
        assert_ne!(lanes["x1"], lanes["y1"]);
        // x lane passes straight through y1's row.
        assert!(g.rows[1]
            .top
            .iter()
            .any(|s| s.from == lanes["x1"] && s.to == lanes["x1"]));
    }

    #[test]
    fn octopus_merge() {
        let c = h(&[
            ("m", &["a", "b", "c"]),
            ("c", &["r"]),
            ("b", &["r"]),
            ("a", &["r"]),
            ("r", &[]),
        ]);
        let g = layout(&c);
        assert_eq!(g.rows[0].bottom.len(), 3);
        assert_eq!(g.max_lanes, 3);
        // c opens the lane for r; b and a join it below themselves.
        assert_eq!(crosses(&g.rows[2].bottom), 1, "{:?}", g.rows[2]);
        assert_eq!(crosses(&g.rows[3].bottom), 1, "{:?}", g.rows[3]);
        assert_eq!(g.rows[4].top.len(), 1);
    }

    #[test]
    fn parent_beyond_the_limit_runs_to_the_bottom() {
        // `b`'s parent `a` was cut off by the log limit.
        let c = h(&[("c", &["b"]), ("b", &["a"])]);
        let g = layout(&c);
        assert_eq!(
            g.rows[1].bottom,
            vec![Seg {
                from: 0,
                to: 0,
                color: g.rows[1].color
            }]
        );
    }

    #[test]
    fn free_lanes_are_reused() {
        // A side branch merged early frees its lane; a later branch reuses it.
        let c = h(&[
            ("m2", &["d", "s2"]),
            ("s2", &["c"]),
            ("d", &["m1"]),
            ("m1", &["c", "s1"]),
            ("s1", &["b"]),
            ("c", &["b"]),
            ("b", &[]),
        ]);
        let g = layout(&c);
        assert_eq!(g.max_lanes, 2, "{:?}", g.rows);
    }
}
