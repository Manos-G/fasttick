//! One pass of the tidy-tree algorithm.
//!
//! The pass follows A. van der Ploeg, "Drawing non-layered tidy trees in
//! linear time", Software: Practice and Experience 44(12), 2014. That
//! algorithm extends the Walker algorithm (as made linear by Buchheim,
//! Jünger and Leipert) to nodes of different sizes:
//!
//! - The first walk visits the nodes in post-order. It places each subtree
//!   to the right of its left siblings, as near as the contours allow.
//!   The contours are linked lists of nodes. *Threads* connect the end of a
//!   short contour to the next contour node of a taller sibling, so each
//!   contour step costs constant time.
//! - A list of the left siblings and their lowest points (`iyl`) tells
//!   which left sibling a conflict comes from. The extra space then goes in
//!   equal parts to the smaller siblings between them (Walker's rule).
//! - The second walk visits the nodes in pre-order and adds the modifiers.
//!
//! This module works in a "breadth and depth" frame: x is the breadth axis
//! (left to right) and the depth axis is only present through
//! [`Shape::bottom`]. The caller maps the frame to the layout direction.
//!
//! Differences from the paper:
//!
//! - The paper includes the gaps in the node sizes. Here the gap between
//!   two contour nodes depends on the pair (`sibling_gap` for two children
//!   of the same parent, else `subtree_gap`), and the caller puts the level
//!   gap into [`Shape::bottom`].
//! - A parent is centred between the centres of its first and last child
//!   (as in Walker's algorithm), not between their outer edges.
//! - The paper only moves a subtree to the right. If the root of a subtree
//!   is far to the right of its first leaf (a narrow parent over wide
//!   children), the subtree then keeps a gap that is too large. Here the
//!   first comparison (the two subtree roots) can also move the subtree to
//!   the left, as in Walker's algorithm, so each subtree is at its leftmost
//!   valid position.
//! - The walks use loops, not recursion, so deep trees cannot overflow the
//!   stack.
//! - The pass can visit the children in reverse order. The layout uses a
//!   normal and a reversed pass and takes the mean, which makes the drawing
//!   of a mirrored tree the exact mirror image.

/// The "no node" value of the index arrays.
pub(super) const NONE: u32 = u32::MAX;

/// The shape of a forest, prepared for the passes.
#[derive(Debug)]
pub(super) struct Shape<'a> {
    /// For each node: the index of its first child in `list`.
    pub first: &'a [u32],
    /// For each node: the number of children.
    pub count: &'a [u32],
    /// The children of all nodes, one block for each node.
    pub list: &'a [u32],
    /// For each node: the parent, or [`NONE`] for a root.
    pub parent: &'a [u32],
    /// For each node: the size along the breadth axis.
    pub width: &'a [f64],
    /// For each node: the position on the depth axis where its band ends
    /// and the band of its children starts (the node position, its size
    /// along the depth axis and the level gap).
    pub bottom: &'a [f64],
    /// The gap between two children of the same parent.
    pub sibling_gap: f64,
    /// The gap between two nodes that have different parents.
    pub subtree_gap: f64,
}

/// The state of one pass. See the module documentation.
#[derive(Debug)]
pub(super) struct Pass {
    reversed: bool,
    prelim: Vec<f64>,
    modifier: Vec<f64>,
    shift: Vec<f64>,
    change: Vec<f64>,
    thread_left: Vec<u32>,
    thread_right: Vec<u32>,
    extreme_left: Vec<u32>,
    extreme_right: Vec<u32>,
    modsum_extreme_left: Vec<f64>,
    modsum_extreme_right: Vec<f64>,
    /// The left siblings of the current subtree and their lowest points,
    /// as a stack: the top has the smallest lowest point.
    iyl: Vec<(f64, u32)>,
}

impl Pass {
    /// Runs one pass and returns the left edge of each node on the breadth
    /// axis. Each tree of the forest has its own frame, with the root at its
    /// preliminary position. `preorder` lists all nodes, each tree in
    /// pre-order. With `reversed`, the pass sees the children of each node
    /// in reverse order (the mirrored tree, without the mirror of the
    /// coordinates).
    pub(super) fn run(shape: &Shape<'_>, preorder: &[u32], reversed: bool) -> Vec<f64> {
        let n = shape.parent.len();
        let mut pass = Pass {
            reversed,
            prelim: vec![0.0; n],
            modifier: vec![0.0; n],
            shift: vec![0.0; n],
            change: vec![0.0; n],
            thread_left: vec![NONE; n],
            thread_right: vec![NONE; n],
            extreme_left: vec![NONE; n],
            extreme_right: vec![NONE; n],
            modsum_extreme_left: vec![0.0; n],
            modsum_extreme_right: vec![0.0; n],
            iyl: Vec::new(),
        };
        // The reverse of a pre-order visits all children of a node before
        // the node itself.
        for &t in preorder.iter().rev() {
            pass.first_walk(shape, t);
        }
        pass.second_walk(shape, preorder)
    }

    /// The child at position `i` in the order of this pass.
    fn child(&self, s: &Shape<'_>, t: u32, i: usize) -> u32 {
        let t = t as usize;
        let first = s.first[t] as usize;
        if self.reversed {
            s.list[first + s.count[t] as usize - 1 - i]
        } else {
            s.list[first + i]
        }
    }

    fn first_walk(&mut self, s: &Shape<'_>, t: u32) {
        let cs = s.count[t as usize] as usize;
        if cs == 0 {
            self.set_extremes(s, t);
            return;
        }
        self.iyl.clear();
        let c0 = self.child(s, t, 0);
        let low = s.bottom[self.extreme_left[c0 as usize] as usize];
        self.update_iyl(low, 0);
        for i in 1..cs {
            let ci = self.child(s, t, i);
            // The lowest point of the subtree, read before `separate` can
            // move the extreme node into a left sibling.
            let low = s.bottom[self.extreme_right[ci as usize] as usize];
            self.separate(s, t, i);
            self.update_iyl(low, i as u32);
        }
        self.position_root(s, t);
        self.set_extremes(s, t);
    }

    fn set_extremes(&mut self, s: &Shape<'_>, t: u32) {
        let cs = s.count[t as usize] as usize;
        let tu = t as usize;
        if cs == 0 {
            self.extreme_left[tu] = t;
            self.extreme_right[tu] = t;
            self.modsum_extreme_left[tu] = 0.0;
            self.modsum_extreme_right[tu] = 0.0;
        } else {
            let first = self.child(s, t, 0) as usize;
            let last = self.child(s, t, cs - 1) as usize;
            self.extreme_left[tu] = self.extreme_left[first];
            self.modsum_extreme_left[tu] = self.modsum_extreme_left[first];
            self.extreme_right[tu] = self.extreme_right[last];
            self.modsum_extreme_right[tu] = self.modsum_extreme_right[last];
        }
    }

    /// Moves subtree `i` of `t` to the leftmost position where it does not
    /// overlap the subtrees `0..i`.
    fn separate(&mut self, s: &Shape<'_>, t: u32, i: usize) {
        // The right contour of the left siblings, and its sum of modifiers.
        let mut sr = self.child(s, t, i - 1);
        let mut mssr = self.modifier[sr as usize];
        // The left contour of the current subtree, and its sum of
        // modifiers.
        let mut cl = self.child(s, t, i);
        let mut mscl = self.modifier[cl as usize];
        let mut cursor = self.iyl.len() - 1;
        // The first pair is the two subtree roots. They always overlap on
        // the depth axis, so their distance is a lower bound for the
        // position of the subtree. The subtree starts there, also when this
        // moves it to the left (as in Walker's algorithm); the other pairs
        // can only move it to the right. Without this step, a subtree whose
        // children extend to the left of its root would keep a large gap.
        let mut first_pair = true;
        while sr != NONE && cl != NONE {
            let sru = sr as usize;
            let clu = cl as usize;
            let sy = s.bottom[sru];
            while cursor > 0 && sy > self.iyl[cursor].0 {
                cursor -= 1;
            }
            let gap = if s.parent[sru] == s.parent[clu] {
                s.sibling_gap
            } else {
                s.subtree_gap
            };
            let dist = (mssr + self.prelim[sru] + s.width[sru] + gap) - (mscl + self.prelim[clu]);
            if dist > 0.0 || (first_pair && dist < 0.0) {
                mscl += dist;
                let from = self.iyl[cursor].1 as usize;
                self.move_subtree(s, t, i, from, dist);
            }
            first_pair = false;
            let cy = s.bottom[clu];
            if sy <= cy {
                sr = self.next_right_contour(s, sr);
                if sr != NONE {
                    mssr += self.modifier[sr as usize];
                }
            }
            if sy >= cy {
                cl = self.next_left_contour(s, cl);
                if cl != NONE {
                    mscl += self.modifier[cl as usize];
                }
            }
        }
        if sr == NONE && cl != NONE {
            // The current subtree is taller than its left siblings.
            self.set_left_thread(s, t, i, cl, mscl);
        } else if sr != NONE && cl == NONE {
            // The left siblings are taller than the current subtree.
            self.set_right_thread(s, t, i, sr, mssr);
        }
    }

    fn move_subtree(&mut self, s: &Shape<'_>, t: u32, i: usize, from: usize, dist: f64) {
        let ci = self.child(s, t, i) as usize;
        self.modifier[ci] += dist;
        self.modsum_extreme_left[ci] += dist;
        self.modsum_extreme_right[ci] += dist;
        // Give the space to the siblings between `from` and `i` in equal
        // parts. `add_child_spacing` applies it in the second walk.
        if from + 1 != i {
            let parts = (i - from) as f64;
            let next = self.child(s, t, from + 1) as usize;
            self.shift[next] += dist / parts;
            self.shift[ci] -= dist / parts;
            self.change[ci] -= dist - dist / parts;
        }
    }

    fn next_left_contour(&self, s: &Shape<'_>, v: u32) -> u32 {
        if s.count[v as usize] == 0 {
            self.thread_left[v as usize]
        } else {
            self.child(s, v, 0)
        }
    }

    fn next_right_contour(&self, s: &Shape<'_>, v: u32) -> u32 {
        let cs = s.count[v as usize] as usize;
        if cs == 0 {
            self.thread_right[v as usize]
        } else {
            self.child(s, v, cs - 1)
        }
    }

    fn set_left_thread(&mut self, s: &Shape<'_>, t: u32, i: usize, cl: u32, modsum_cl: f64) {
        let first = self.child(s, t, 0) as usize;
        let current = self.child(s, t, i) as usize;
        let li = self.extreme_left[first] as usize;
        self.thread_left[li] = cl;
        // Change the modifier so that the sum of modifiers along the thread
        // is correct, and the preliminary position so that `li` does not
        // move.
        let diff = (modsum_cl - self.modifier[cl as usize]) - self.modsum_extreme_left[first];
        self.modifier[li] += diff;
        self.prelim[li] -= diff;
        self.extreme_left[first] = self.extreme_left[current];
        self.modsum_extreme_left[first] = self.modsum_extreme_left[current];
    }

    fn set_right_thread(&mut self, s: &Shape<'_>, t: u32, i: usize, sr: u32, modsum_sr: f64) {
        let current = self.child(s, t, i) as usize;
        let previous = self.child(s, t, i - 1) as usize;
        let ri = self.extreme_right[current] as usize;
        self.thread_right[ri] = sr;
        let diff = (modsum_sr - self.modifier[sr as usize]) - self.modsum_extreme_right[current];
        self.modifier[ri] += diff;
        self.prelim[ri] -= diff;
        self.extreme_right[current] = self.extreme_right[previous];
        self.modsum_extreme_right[current] = self.modsum_extreme_right[previous];
    }

    /// Centres `t` between the centres of its first and last child.
    fn position_root(&mut self, s: &Shape<'_>, t: u32) {
        let cs = s.count[t as usize] as usize;
        let first = self.child(s, t, 0) as usize;
        let last = self.child(s, t, cs - 1) as usize;
        let first_centre = self.prelim[first] + self.modifier[first] + s.width[first] * 0.5;
        let last_centre = self.prelim[last] + self.modifier[last] + s.width[last] * 0.5;
        self.prelim[t as usize] = (first_centre + last_centre) * 0.5 - s.width[t as usize] * 0.5;
    }

    /// Updates the list of left siblings with subtree `index`, whose lowest
    /// point is `low`. Siblings that the new subtree hides are removed.
    fn update_iyl(&mut self, low: f64, index: u32) {
        while let Some(&(top, _)) = self.iyl.last() {
            if low >= top {
                self.iyl.pop();
            } else {
                break;
            }
        }
        self.iyl.push((low, index));
    }

    /// Adds the modifiers from the roots down and returns the positions.
    fn second_walk(&mut self, s: &Shape<'_>, preorder: &[u32]) -> Vec<f64> {
        let n = s.parent.len();
        let mut modsum = vec![0.0; n];
        let mut x = vec![0.0; n];
        for &t in preorder {
            let tu = t as usize;
            let parent = s.parent[tu];
            let base = if parent == NONE {
                0.0
            } else {
                modsum[parent as usize]
            };
            let sum = base + self.modifier[tu];
            modsum[tu] = sum;
            x[tu] = self.prelim[tu] + sum;
            self.add_child_spacing(s, t);
        }
        x
    }

    /// Applies the `shift` and `change` values of the children of `t` to
    /// their modifiers.
    fn add_child_spacing(&mut self, s: &Shape<'_>, t: u32) {
        let mut d = 0.0;
        let mut delta = 0.0;
        for i in 0..s.count[t as usize] as usize {
            let c = self.child(s, t, i) as usize;
            d += self.shift[c];
            delta += d + self.change[c];
            self.modifier[c] += delta;
        }
    }
}
