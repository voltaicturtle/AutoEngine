//! Hex grid geometry for the battlefield.
//!
//! Hexes use axial coordinates `(q, r)` internally (cube coordinates with
//! `s = -q - r` implied), which makes distance, neighbors and rings simple.
//! Boards are rectangular in "odd-r" offset layout: pointy-top hexes where
//! every odd row is shifted half a hex to the right. Offset `(col, row)`
//! coordinates are what players and content files see.
//!
//! All iteration orders here are fixed, so pathfinding and target selection
//! that build on them stay deterministic.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// A hex position in axial coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hex {
    pub q: i32,
    pub r: i32,
}

/// The six axial directions, in a fixed clockwise order starting east.
pub const DIRECTIONS: [Hex; 6] = [
    Hex { q: 1, r: 0 },
    Hex { q: 1, r: -1 },
    Hex { q: 0, r: -1 },
    Hex { q: -1, r: 0 },
    Hex { q: -1, r: 1 },
    Hex { q: 0, r: 1 },
];

impl Hex {
    pub const fn new(q: i32, r: i32) -> Self {
        Hex { q, r }
    }

    /// The implied third cube coordinate.
    pub const fn s(self) -> i32 {
        -self.q - self.r
    }

    /// Converts from odd-r offset coordinates `(col, row)`.
    pub const fn from_offset(col: i32, row: i32) -> Self {
        let q = col - (row - (row & 1)) / 2;
        Hex { q, r: row }
    }

    /// Converts to odd-r offset coordinates `(col, row)`.
    pub const fn to_offset(self) -> (i32, i32) {
        let col = self.q + (self.r - (self.r & 1)) / 2;
        (col, self.r)
    }

    pub const fn add(self, other: Hex) -> Hex {
        Hex::new(self.q + other.q, self.r + other.r)
    }

    pub const fn sub(self, other: Hex) -> Hex {
        Hex::new(self.q - other.q, self.r - other.r)
    }

    pub const fn scale(self, k: i32) -> Hex {
        Hex::new(self.q * k, self.r * k)
    }

    /// Number of steps between two hexes.
    pub const fn distance(self, other: Hex) -> u32 {
        let d = self.sub(other);
        ((d.q.abs() + d.r.abs() + d.s().abs()) / 2) as u32
    }

    /// The six neighbors, in [`DIRECTIONS`] order.
    pub fn neighbors(self) -> [Hex; 6] {
        DIRECTIONS.map(|d| self.add(d))
    }

    /// All hexes exactly `radius` steps away, in a fixed order.
    pub fn ring(self, radius: u32) -> Vec<Hex> {
        if radius == 0 {
            return vec![self];
        }
        let mut out = Vec::with_capacity(6 * radius as usize);
        let mut h = self.add(DIRECTIONS[4].scale(radius as i32));
        for dir in DIRECTIONS {
            for _ in 0..radius {
                out.push(h);
                h = h.add(dir);
            }
        }
        out
    }

    /// All hexes within `radius` steps (inclusive), center first, then by ring.
    pub fn spiral(self, radius: u32) -> Vec<Hex> {
        (0..=radius).flat_map(|k| self.ring(k)).collect()
    }

    /// Hexes on the straight line from `self` to `other`, inclusive of both
    /// ends. Uses integer-only cube rounding so it is platform independent.
    pub fn line_to(self, other: Hex) -> Vec<Hex> {
        let n = self.distance(other) as i64;
        if n == 0 {
            return vec![self];
        }
        // Work in cube coordinates scaled by 2n, nudged by a tiny fixed
        // epsilon (1 unit in the scaled space) to break ties consistently.
        let (aq, ar, as_) = (self.q as i64, self.r as i64, self.s() as i64);
        let (bq, br, bs) = (other.q as i64, other.r as i64, other.s() as i64);
        let denom = 2 * n;
        (0..=n)
            .map(|i| {
                let q = (aq * (n - i) + bq * i) * 2 + 1;
                let r = (ar * (n - i) + br * i) * 2 + 1;
                let s = (as_ * (n - i) + bs * i) * 2 - 2;
                cube_round_scaled(q, r, s, denom)
            })
            .collect()
    }
}

/// Rounds cube coordinates given as `value / denom` to the nearest hex.
fn cube_round_scaled(q: i64, r: i64, s: i64, denom: i64) -> Hex {
    let round = |v: i64| -> i64 { (2 * v + denom).div_euclid(2 * denom) };
    let (mut rq, mut rr, rs) = (round(q), round(r), round(s));
    // Compare rounding errors scaled by denom.
    let dq = (rq * denom - q).abs();
    let dr = (rr * denom - r).abs();
    let ds = (rs * denom - s).abs();
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    Hex::new(rq as i32, rr as i32)
}

/// A rectangular board of `cols x rows` hexes in odd-r layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HexBoard {
    cols: i32,
    rows: i32,
}

impl HexBoard {
    pub fn new(cols: u32, rows: u32) -> Self {
        HexBoard { cols: cols as i32, rows: rows as i32 }
    }

    pub fn cols(&self) -> u32 {
        self.cols as u32
    }

    pub fn rows(&self) -> u32 {
        self.rows as u32
    }

    pub fn len(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, h: Hex) -> bool {
        let (c, r) = h.to_offset();
        (0..self.cols).contains(&c) && (0..self.rows).contains(&r)
    }

    /// Dense index for per-cell storage, or `None` if off the board.
    pub fn index(&self, h: Hex) -> Option<usize> {
        let (c, r) = h.to_offset();
        self.contains(h).then(|| (r * self.cols + c) as usize)
    }

    /// Inverse of [`HexBoard::index`].
    pub fn hex_at(&self, index: usize) -> Option<Hex> {
        (index < self.len()).then(|| {
            let i = index as i32;
            Hex::from_offset(i % self.cols, i / self.cols)
        })
    }

    /// All hexes, row by row.
    pub fn hexes(&self) -> impl Iterator<Item = Hex> + '_ {
        (0..self.rows).flat_map(move |r| (0..self.cols).map(move |c| Hex::from_offset(c, r)))
    }

    /// On-board neighbors in fixed order.
    pub fn neighbors(&self, h: Hex) -> impl Iterator<Item = Hex> + '_ {
        h.neighbors().into_iter().filter(move |n| self.contains(*n))
    }

    /// Mirrors a hex through the board center (180 degree rotation). Used to
    /// place the away team's units onto the shared combat board.
    pub fn mirror(&self, h: Hex) -> Hex {
        let (c, r) = h.to_offset();
        let (mc, mr) = (self.cols - 1 - c, self.rows - 1 - r);
        Hex::from_offset(mc, mr)
    }

    /// Breadth-first shortest path from `start` to any hex for which `is_goal`
    /// returns true, moving only through hexes where `passable` is true
    /// (the goal hex itself need not be passable). Returns the path excluding
    /// `start`, or `None` if unreachable. Ties are broken by the fixed
    /// neighbor order, so the result is deterministic.
    pub fn find_path(
        &self,
        start: Hex,
        mut is_goal: impl FnMut(Hex) -> bool,
        mut passable: impl FnMut(Hex) -> bool,
    ) -> Option<Vec<Hex>> {
        let start_idx = self.index(start)?;
        if is_goal(start) {
            return Some(Vec::new());
        }
        let mut came_from: Vec<Option<usize>> = vec![None; self.len()];
        let mut visited = vec![false; self.len()];
        visited[start_idx] = true;
        let mut queue = VecDeque::from([start]);
        while let Some(cur) = queue.pop_front() {
            let cur_idx = self.index(cur).unwrap();
            for n in self.neighbors(cur) {
                let ni = self.index(n).unwrap();
                if visited[ni] {
                    continue;
                }
                visited[ni] = true;
                came_from[ni] = Some(cur_idx);
                if is_goal(n) {
                    let mut path = vec![n];
                    let mut i = cur_idx;
                    while i != start_idx {
                        path.push(self.hex_at(i).unwrap());
                        i = came_from[i].unwrap();
                    }
                    path.reverse();
                    return Some(path);
                }
                if passable(n) {
                    queue.push_back(n);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_roundtrip() {
        for row in -5..5 {
            for col in -5..5 {
                assert_eq!(Hex::from_offset(col, row).to_offset(), (col, row));
            }
        }
    }

    #[test]
    fn neighbors_are_distance_one() {
        let h = Hex::new(2, -1);
        for n in h.neighbors() {
            assert_eq!(h.distance(n), 1);
        }
    }

    #[test]
    fn ring_and_spiral_sizes() {
        let c = Hex::new(0, 0);
        assert_eq!(c.ring(0).len(), 1);
        assert_eq!(c.ring(3).len(), 18);
        assert!(c.ring(3).iter().all(|h| c.distance(*h) == 3));
        assert_eq!(c.spiral(2).len(), 19);
    }

    #[test]
    fn line_has_distance_plus_one_contiguous_hexes() {
        let a = Hex::new(0, 0);
        let b = Hex::new(4, -1);
        let line = a.line_to(b);
        assert_eq!(line.len(), a.distance(b) as usize + 1);
        assert_eq!(line[0], a);
        assert_eq!(*line.last().unwrap(), b);
        for w in line.windows(2) {
            assert_eq!(w[0].distance(w[1]), 1);
        }
    }

    #[test]
    fn board_index_roundtrip_and_mirror() {
        let b = HexBoard::new(7, 8);
        assert_eq!(b.len(), 56);
        for (i, h) in b.hexes().enumerate() {
            assert_eq!(b.index(h), Some(i));
            assert_eq!(b.hex_at(i), Some(h));
            let m = b.mirror(h);
            assert!(b.contains(m));
            assert_eq!(b.mirror(m), h);
        }
        assert!(!b.contains(Hex::from_offset(7, 0)));
        assert!(!b.contains(Hex::from_offset(0, -1)));
    }

    #[test]
    fn path_avoids_blocked_hexes() {
        let b = HexBoard::new(5, 5);
        let start = Hex::from_offset(0, 2);
        let goal = Hex::from_offset(4, 2);
        // Wall down column 2 except the top row.
        let blocked = |h: Hex| {
            let (c, r) = h.to_offset();
            c == 2 && r > 0
        };
        let path = b.find_path(start, |h| h == goal, |h| !blocked(h)).unwrap();
        assert_eq!(*path.last().unwrap(), goal);
        assert!(path.iter().all(|h| !blocked(*h)));
        let mut prev = start;
        for h in &path {
            assert_eq!(prev.distance(*h), 1);
            prev = *h;
        }
        // Fully walled off: unreachable.
        assert!(b.find_path(start, |h| h == goal, |h| h.to_offset().0 != 2).is_none());
    }

    #[test]
    fn path_is_deterministic() {
        let b = HexBoard::new(7, 8);
        let s = Hex::from_offset(0, 0);
        let g = Hex::from_offset(6, 7);
        let p1 = b.find_path(s, |h| h == g, |_| true);
        let p2 = b.find_path(s, |h| h == g, |_| true);
        assert_eq!(p1, p2);
        assert_eq!(p1.unwrap().len(), s.distance(g) as usize);
    }
}
