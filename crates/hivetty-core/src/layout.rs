//! Binary split tree for panes inside a surface (tab).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type PaneId = Uuid;

/// Orientation of a split: `Row` places children left|right, `Column` places them top/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Row,
    Column,
}

/// Arrow direction used for splitting, focus movement and resizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Column,
        }
    }
    /// True when the new/target pane goes after (right of / below) the reference.
    pub fn is_after(self) -> bool {
        matches!(self, Dir::Right | Dir::Down)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Leaf {
        pane: PaneId,
    },
    Split {
        axis: Axis,
        /// Fraction (0..1) of space given to `a`.
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const UNIT: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
    fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

const MIN_RATIO: f32 = 0.05;
const EPS: f32 = 1e-4;

impl Node {
    pub fn leaf(pane: PaneId) -> Self {
        Node::Leaf { pane }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf { pane } => out.push(*pane),
            Node::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf { pane } => *pane == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Split `target`, inserting `new` on the side given by `dir`. Returns false if target not found.
    pub fn split(&mut self, target: PaneId, dir: Dir, new: PaneId) -> bool {
        match self {
            Node::Leaf { pane } if *pane == target => {
                let old = Node::leaf(*pane);
                let fresh = Node::leaf(new);
                let (a, b) = if dir.is_after() { (old, fresh) } else { (fresh, old) };
                *self = Node::Split { axis: dir.axis(), ratio: 0.5, a: Box::new(a), b: Box::new(b) };
                true
            }
            Node::Leaf { .. } => false,
            Node::Split { a, b, .. } => a.split(target, dir, new) || b.split(target, dir, new),
        }
    }

    /// Remove `target`; its sibling takes the parent's place. Returns None if the tree becomes empty
    /// (target was the only leaf), otherwise Some(new_root). If target is absent, returns the tree unchanged.
    pub fn remove(self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf { pane } if pane == target => None,
            leaf @ Node::Leaf { .. } => Some(leaf),
            Node::Split { axis, ratio, a, b } => {
                if !a.contains(target) && !b.contains(target) {
                    return Some(Node::Split { axis, ratio, a, b });
                }
                match (a.remove(target), b.remove(target)) {
                    (None, Some(rest)) | (Some(rest), None) => Some(rest),
                    (Some(a), Some(b)) => Some(Node::Split { axis, ratio, a: Box::new(a), b: Box::new(b) }),
                    (None, None) => None,
                }
            }
        }
    }

    /// Compute normalized rectangles for every leaf.
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.rects_into(area, &mut out);
        out
    }

    fn rects_into(&self, r: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf { pane } => out.push((*pane, r)),
            Node::Split { axis, ratio, a, b } => {
                let (ra, rb) = match axis {
                    Axis::Row => {
                        let wa = r.w * ratio;
                        (Rect { w: wa, ..r }, Rect { x: r.x + wa, w: r.w - wa, ..r })
                    }
                    Axis::Column => {
                        let ha = r.h * ratio;
                        (Rect { h: ha, ..r }, Rect { y: r.y + ha, h: r.h - ha, ..r })
                    }
                };
                a.rects_into(ra, out);
                b.rects_into(rb, out);
            }
        }
    }

    /// Find the nearest pane adjacent to `from` in direction `dir`.
    pub fn neighbor(&self, from: PaneId, dir: Dir) -> Option<PaneId> {
        let rects = self.rects(Rect::UNIT);
        let src = rects.iter().find(|(p, _)| *p == from)?.1;
        rects
            .iter()
            .filter(|(p, _)| *p != from)
            .filter(|(_, r)| {
                let (touch, overlap) = match dir {
                    Dir::Left => ((r.x + r.w - src.x).abs() < EPS, ranges_overlap(src.y, src.h, r.y, r.h)),
                    Dir::Right => ((src.x + src.w - r.x).abs() < EPS, ranges_overlap(src.y, src.h, r.y, r.h)),
                    Dir::Up => ((r.y + r.h - src.y).abs() < EPS, ranges_overlap(src.x, src.w, r.x, r.w)),
                    Dir::Down => ((src.y + src.h - r.y).abs() < EPS, ranges_overlap(src.x, src.w, r.x, r.w)),
                };
                touch && overlap
            })
            .min_by(|(_, a), (_, b)| {
                let da = (a.cx() - src.cx()).powi(2) + (a.cy() - src.cy()).powi(2);
                let db = (b.cx() - src.cx()).powi(2) + (b.cy() - src.cy()).powi(2);
                da.total_cmp(&db)
            })
            .map(|(p, _)| *p)
    }

    /// Move the border of `target` in direction `dir` by `delta` (fraction of the parent split).
    /// Adjusts the nearest ancestor split along that axis. Returns true if something changed.
    pub fn resize(&mut self, target: PaneId, dir: Dir, delta: f32) -> bool {
        self.resize_inner(target, dir, delta).unwrap_or(false)
    }

    // Some(true)=handled, Some(false)=contains target but no matching split yet, None=not here
    fn resize_inner(&mut self, target: PaneId, dir: Dir, delta: f32) -> Option<bool> {
        match self {
            Node::Leaf { pane } => (*pane == target).then_some(false),
            Node::Split { axis, ratio, a, b } => {
                let in_a = a.contains(target);
                let child = if in_a { a.as_mut() } else if b.contains(target) { b.as_mut() } else { return None };
                if let Some(true) = child.resize_inner(target, dir, delta) {
                    return Some(true);
                }
                if *axis != dir.axis() {
                    return Some(false);
                }
                // Growing toward `dir`: if target is in `a` its right/bottom border moves; in `b`, its left/top.
                let sign = if dir.is_after() { 1.0 } else { -1.0 };
                let movable = if in_a { dir.is_after() } else { !dir.is_after() };
                if !movable {
                    return Some(false);
                }
                *ratio = (*ratio + sign * delta).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                Some(true)
            }
        }
    }

    /// Give `target` the fraction `f` (0..1) of its nearest ancestor split along `axis`
    /// (tmux `resize-pane -x 30%`). Returns true if a split was found.
    pub fn set_fraction(&mut self, target: PaneId, axis: Axis, f: f32) -> bool {
        match self {
            Node::Leaf { .. } => false,
            Node::Split { axis: ax, ratio, a, b } => {
                let in_a = a.contains(target);
                let child = if in_a { a.as_mut() } else if b.contains(target) { b.as_mut() } else { return false };
                if child.set_fraction(target, axis, f) {
                    return true;
                }
                if *ax != axis {
                    return false;
                }
                let f = f.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                *ratio = if in_a { f } else { 1.0 - f };
                true
            }
        }
    }

    /// Reset every split ratio to 0.5.
    pub fn equalize(&mut self) {
        if let Node::Split { ratio, a, b, .. } = self {
            *ratio = 0.5;
            a.equalize();
            b.equalize();
        }
    }

    /// Swap two leaves' positions.
    pub fn swap(&mut self, x: PaneId, y: PaneId) {
        match self {
            Node::Leaf { pane } => {
                if *pane == x {
                    *pane = y;
                } else if *pane == y {
                    *pane = x;
                }
            }
            Node::Split { a, b, .. } => {
                a.swap(x, y);
                b.swap(x, y);
            }
        }
    }
}

fn ranges_overlap(a0: f32, al: f32, b0: f32, bl: f32) -> bool {
    a0 < b0 + bl - EPS && b0 < a0 + al - EPS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<PaneId> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[test]
    fn set_fraction_nearest_axis() {
        let p = ids(3);
        let mut t = Node::leaf(p[0]);
        t.split(p[0], Dir::Right, p[1]);
        t.split(p[1], Dir::Down, p[2]);
        assert!(t.set_fraction(p[2], Axis::Row, 0.3));
        let Node::Split { ratio, .. } = &t else { panic!() };
        assert!((ratio - 0.7).abs() < 1e-6);
        assert!(t.set_fraction(p[2], Axis::Column, 0.25));
        let Node::Split { b, .. } = &t else { panic!() };
        let Node::Split { ratio, .. } = b.as_ref() else { panic!() };
        assert!((ratio - 0.75).abs() < 1e-6);
        assert!(!Node::leaf(p[0]).set_fraction(p[0], Axis::Row, 0.5));
    }

    #[test]
    fn split_and_remove() {
        let p = ids(3);
        let mut t = Node::leaf(p[0]);
        assert!(t.split(p[0], Dir::Right, p[1]));
        assert!(t.split(p[1], Dir::Down, p[2]));
        assert_eq!(t.panes(), vec![p[0], p[1], p[2]]);
        let t = t.remove(p[1]).unwrap();
        assert_eq!(t.panes(), vec![p[0], p[2]]);
        let t = t.remove(p[0]).unwrap();
        assert_eq!(t, Node::leaf(p[2]));
        assert!(t.remove(p[2]).is_none());
    }

    #[test]
    fn split_before_places_new_first() {
        let p = ids(2);
        let mut t = Node::leaf(p[0]);
        t.split(p[0], Dir::Left, p[1]);
        assert_eq!(t.panes(), vec![p[1], p[0]]);
    }

    #[test]
    fn neighbors() {
        // [0 | [1 / 2]]
        let p = ids(3);
        let mut t = Node::leaf(p[0]);
        t.split(p[0], Dir::Right, p[1]);
        t.split(p[1], Dir::Down, p[2]);
        assert_eq!(t.neighbor(p[0], Dir::Right), Some(p[1]));
        assert_eq!(t.neighbor(p[1], Dir::Down), Some(p[2]));
        assert_eq!(t.neighbor(p[2], Dir::Up), Some(p[1]));
        assert_eq!(t.neighbor(p[2], Dir::Left), Some(p[0]));
        assert_eq!(t.neighbor(p[0], Dir::Left), None);
    }

    #[test]
    fn resize_moves_correct_border() {
        let p = ids(2);
        let mut t = Node::leaf(p[0]);
        t.split(p[0], Dir::Right, p[1]);
        assert!(t.resize(p[0], Dir::Right, 0.1));
        assert!(t.resize(p[1], Dir::Left, 0.1)); // b grows left -> ratio shrinks
        assert!(!t.resize(p[0], Dir::Left, 0.1)); // a's left edge is the window edge
        assert!(!t.resize(p[0], Dir::Down, 0.1));
        if let Node::Split { ratio, .. } = t {
            assert!((ratio - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn serde_roundtrip() {
        let p = ids(2);
        let mut t = Node::leaf(p[0]);
        t.split(p[0], Dir::Down, p[1]);
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Node>(&s).unwrap(), t);
    }
}
