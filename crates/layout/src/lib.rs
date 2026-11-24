//! Pure grid-layout logic for the terminal grid.
//!
//! Terminals fill the grid horizontally first (up to `max_cols` per row) and
//! only then grow vertically (up to `max_rows`). The last row holds the
//! remainder and its cells stretch to fill the full width.

/// Grid capacity limits. Values below 1 are treated as 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSpec {
    pub max_cols: usize,
    pub max_rows: usize,
}

impl GridSpec {
    pub fn new(max_cols: usize, max_rows: usize) -> Self {
        Self { max_cols: max_cols.max(1), max_rows: max_rows.max(1) }
    }

    pub fn capacity(&self) -> usize {
        self.max_cols.max(1) * self.max_rows.max(1)
    }

    pub fn is_full(&self, n: usize) -> bool {
        n >= self.capacity()
    }
}

impl Default for GridSpec {
    fn default() -> Self {
        Self::new(3, 2)
    }
}

/// Rows of cell indices (indices into the caller's ordered terminal list).
pub type Layout = Vec<Vec<usize>>;

/// Computes rows for `n` items. Items beyond capacity are ignored.
pub fn compute_layout(n: usize, spec: GridSpec) -> Layout {
    let cols = spec.max_cols.max(1);
    let n = n.min(spec.capacity());
    (0..n).collect::<Vec<_>>().chunks(cols).map(<[usize]>::to_vec).collect()
}

/// Rows of the cells the grid shows for `n` terminals: the terminals plus,
/// while there is room, one more slot (index `n`) for the "+" cell.
pub fn slots(n: usize, spec: GridSpec) -> Layout {
    compute_layout(if spec.is_full(n) { n } else { n + 1 }, spec)
}

/// Row/column position of `index` within a layout.
pub fn position_of(layout: &Layout, index: usize) -> Option<(usize, usize)> {
    layout.iter().enumerate().find_map(|(r, row)| row.iter().position(|&i| i == index).map(|c| (r, c)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Neighbour of `index` in `dir`. Vertical moves pick the cell whose horizontal
/// centre is closest, since rows can have different cell counts.
pub fn neighbor(layout: &Layout, index: usize, dir: Direction) -> Option<usize> {
    let (r, c) = position_of(layout, index)?;
    let row = &layout[r];
    match dir {
        Direction::Left => c.checked_sub(1).map(|c| row[c]),
        Direction::Right => row.get(c + 1).copied(),
        Direction::Up | Direction::Down => {
            let target_r = if dir == Direction::Up { r.checked_sub(1)? } else { r + 1 };
            let target = layout.get(target_r)?;
            let centre = (c as f32 + 0.5) / row.len() as f32;
            let tc = ((centre * target.len() as f32) as usize).min(target.len() - 1);
            Some(target[tc])
        }
    }
}

/// Index to focus after removing `removed` from a list of `n_before` items.
pub fn focus_after_close(n_before: usize, removed: usize) -> Option<usize> {
    if n_before <= 1 {
        None
    } else if removed + 1 < n_before {
        Some(removed) // the next item shifts into this slot
    } else {
        Some(removed - 1)
    }
}

/// Relative sizes of rows and of the cells inside each row (resizable grid).
#[derive(Clone, Debug, PartialEq)]
pub struct GridWeights {
    pub rows: Vec<f32>,
    pub cols: Vec<Vec<f32>>,
}

impl GridWeights {
    /// Equal sizes for every row and cell of `layout`.
    pub fn equal(layout: &Layout) -> Self {
        Self { rows: vec![1.0; layout.len()], cols: layout.iter().map(|r| vec![1.0; r.len()]).collect() }
    }

    /// Whether these weights describe `layout` (same rows and cells per row).
    pub fn matches(&self, layout: &Layout) -> bool {
        self.rows.len() == layout.len() && self.cols.iter().map(Vec::len).eq(layout.iter().map(Vec::len))
    }
}

/// Moves the boundary between `weights[a]` and `weights[a + 1]` by `delta_px`
/// along an axis that is `total_px` long. Neither cell gets smaller than
/// `min_px`; the other cells are untouched.
pub fn drag_boundary(weights: &mut [f32], a: usize, delta_px: f32, total_px: f32, min_px: f32) {
    if a + 1 >= weights.len() || total_px <= 0.0 {
        return;
    }
    let sum: f32 = weights.iter().sum();
    let per_px = sum / total_px;
    let pair = weights[a] + weights[a + 1];
    let min_w = (min_px * per_px).min(pair / 2.0);
    let new_a = (weights[a] + delta_px * per_px).clamp(min_w, pair - min_w);
    weights[a] = new_a;
    weights[a + 1] = pair - new_a;
}

/// Back to equal sizes.
pub fn reset_weights(weights: &mut [f32]) {
    weights.iter_mut().for_each(|w| *w = 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_add_the_plus_cell_until_full() {
        let spec = GridSpec::new(3, 2);
        assert_eq!(slots(0, spec), vec![vec![0]]);
        assert_eq!(slots(1, spec), vec![vec![0, 1]]);
        assert_eq!(slots(3, spec), vec![vec![0, 1, 2], vec![3]]);
        assert_eq!(slots(5, spec), vec![vec![0, 1, 2], vec![3, 4, 5]]);
        assert_eq!(slots(6, spec), compute_layout(6, spec));
        assert_eq!(slots(1, GridSpec::new(1, 1)), vec![vec![0]]);
    }

    fn rows(n: usize, c: usize, r: usize) -> Vec<usize> {
        compute_layout(n, GridSpec::new(c, r)).iter().map(Vec::len).collect()
    }

    #[test]
    fn expands_horizontally_then_vertically() {
        assert_eq!(rows(0, 3, 2), Vec::<usize>::new());
        assert_eq!(rows(1, 3, 2), vec![1]);
        assert_eq!(rows(2, 3, 2), vec![2]);
        assert_eq!(rows(3, 3, 2), vec![3]);
        assert_eq!(rows(4, 3, 2), vec![3, 1]);
        assert_eq!(rows(5, 3, 2), vec![3, 2]);
        assert_eq!(rows(6, 3, 2), vec![3, 3]);
    }

    #[test]
    fn caps_at_capacity() {
        assert_eq!(rows(9, 3, 2), vec![3, 3]);
        assert!(GridSpec::new(3, 2).is_full(6));
        assert!(!GridSpec::new(3, 2).is_full(5));
    }

    #[test]
    fn exhaustive_invariants() {
        for c in 1..=6 {
            for r in 1..=6 {
                let spec = GridSpec::new(c, r);
                for n in 0..=spec.capacity() + 2 {
                    let l = compute_layout(n, spec);
                    let flat: Vec<usize> = l.iter().flatten().copied().collect();
                    assert_eq!(flat, (0..n.min(spec.capacity())).collect::<Vec<_>>());
                    assert!(l.len() <= r);
                    assert!(l.iter().all(|row| !row.is_empty() && row.len() <= c));
                    // all rows except the last are full
                    if l.len() > 1 {
                        assert!(l[..l.len() - 1].iter().all(|row| row.len() == c));
                    }
                }
            }
        }
    }

    #[test]
    fn zero_spec_is_clamped() {
        assert_eq!(rows(2, 0, 0), vec![1]);
    }

    #[test]
    fn neighbors() {
        let l = compute_layout(4, GridSpec::new(3, 2)); // [0 1 2] / [3]
        assert_eq!(neighbor(&l, 1, Direction::Right), Some(2));
        assert_eq!(neighbor(&l, 0, Direction::Left), None);
        assert_eq!(neighbor(&l, 2, Direction::Down), Some(3));
        assert_eq!(neighbor(&l, 3, Direction::Up), Some(1)); // centre of a full-width cell
        let l = compute_layout(5, GridSpec::new(3, 2)); // [0 1 2] / [3 4]
        assert_eq!(neighbor(&l, 4, Direction::Up), Some(2));
        assert_eq!(neighbor(&l, 0, Direction::Down), Some(3));
        assert_eq!(neighbor(&l, 3, Direction::Down), None);
    }

    #[test]
    fn weights_shape_and_reset() {
        let l = compute_layout(5, GridSpec::new(3, 2));
        let mut w = GridWeights::equal(&l);
        assert!(w.matches(&l));
        assert!(!w.matches(&compute_layout(4, GridSpec::new(3, 2))));
        drag_boundary(&mut w.cols[0], 0, 100.0, 900.0, 50.0);
        assert!(w.cols[0][0] > 1.0);
        reset_weights(&mut w.cols[0]);
        assert_eq!(w.cols[0], vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn drag_moves_only_the_pair_and_respects_min() {
        let mut w = vec![1.0, 1.0, 1.0];
        // 900 px for 3 units: 300 px each. Drag boundary 0|1 right by 100 px.
        drag_boundary(&mut w, 0, 100.0, 900.0, 50.0);
        assert!((w[0] * 300.0 - 400.0).abs() < 0.01 && (w[1] * 300.0 - 200.0).abs() < 0.01);
        assert_eq!(w[2], 1.0);
        // Dragging far clamps at the minimum size of the neighbour.
        drag_boundary(&mut w, 0, 10_000.0, 900.0, 50.0);
        assert!((w[1] * 300.0 - 50.0).abs() < 0.01, "{w:?}");
        drag_boundary(&mut w, 0, -10_000.0, 900.0, 50.0);
        assert!((w[0] * 300.0 - 50.0).abs() < 0.01, "{w:?}");
        let sum: f32 = w.iter().sum();
        assert!((sum - 3.0).abs() < 1e-4);
    }

    #[test]
    fn focus_after_closing() {
        assert_eq!(focus_after_close(1, 0), None);
        assert_eq!(focus_after_close(3, 0), Some(0));
        assert_eq!(focus_after_close(3, 2), Some(1));
    }
}
