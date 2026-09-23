//! Implicit (index-addressed) treap over pieces.
//!
//! The piece list has to support four things at once, on documents with up to a
//! few million pieces:
//!
//! * `split` at an arbitrary byte offset and `merge`, so an edit is a local
//!   splice rather than a rewrite;
//! * `O(log n)` prefix queries for *bytes* and for *newlines*, which is what
//!   makes "byte offset -> line number" and "line number -> byte offset" cheap
//!   without a second index structure;
//! * removal/insertion of whole runs of pieces, so undo can move a deleted run
//!   in and out of the tree without copying its bytes;
//! * node recycling, so a long editing session does not leak one allocation per
//!   keystroke.
//!
//! Nodes live in a flat `Vec` addressed by `u32`; children use [`NIL`] instead of
//! `Option<u32>`, which keeps `Node` at 32 bytes and keeps the hot paths free of
//! branch-heavy option handling. The arena is only ever borrowed one field at a
//! time, so no `unsafe` is needed.

use super::{count_newlines, Piece, PieceSource, NIL};

/// A piece plus its subtree aggregates.
///
/// Public only so [`super::PieceStream`] can walk the arena; treat as opaque.
#[derive(Clone, Copy)]
pub struct Node {
    pub piece: Piece,
    /// Max-heap priority; drives the expected `O(log n)` depth.
    pub prio: u32,
    pub left: u32,
    pub right: u32,
    /// Total bytes in this subtree.
    pub sub_len: u32,
    /// Total newlines in this subtree.
    pub sub_nl: u32,
}

/// One row of [`Treap::dump`]: `(index, prio, left, right, src, start, len, sub_len, sub_nl)`.
pub type NodeRow = (u32, u32, u32, u32, u32, u32, u32, u32, u32);

pub struct Treap {
    nodes: Vec<Node>,
    free: Vec<u32>,
    root: u32,
    rng: u64,
}

impl Default for Treap {
    fn default() -> Self {
        Self::new()
    }
}

impl Treap {
    pub fn new() -> Self {
        Treap {
            nodes: Vec::new(),
            free: Vec::new(),
            root: NIL,
            // Fixed seed: tree shape is deterministic across runs, which keeps
            // benchmarks and bug reports reproducible.
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    #[inline]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    #[inline]
    pub fn root(&self) -> u32 {
        self.root
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.root == NIL
    }

    /// Total bytes.
    #[inline]
    pub fn len(&self) -> u32 {
        self.sub_len(self.root)
    }

    /// Total newlines.
    #[inline]
    pub fn newlines(&self) -> u32 {
        self.sub_nl(self.root)
    }

    /// Live piece count.
    pub fn piece_count(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    #[inline]
    fn sub_len(&self, n: u32) -> u32 {
        if n == NIL {
            0
        } else {
            self.nodes[n as usize].sub_len
        }
    }

    #[inline]
    fn sub_nl(&self, n: u32) -> u32 {
        if n == NIL {
            0
        } else {
            self.nodes[n as usize].sub_nl
        }
    }

    #[inline]
    fn next_prio(&mut self) -> u32 {
        // xorshift64*: cheap, no dependency, good enough to keep the treap flat.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn alloc(&mut self, piece: Piece) -> u32 {
        let prio = self.next_prio();
        self.alloc_with_prio(piece, prio)
    }

    /// Allocate a node with an explicit priority.
    ///
    /// Splitting a piece produces two nodes that take over the original node's
    /// position in the tree. They must inherit its priority: drawing fresh ones
    /// would let a half outrank its own parent, which breaks heap order and
    /// eventually corrupts the tree. Equal priorities between the two halves are
    /// harmless, because they never end up in the same tree at split time.
    fn alloc_with_prio(&mut self, piece: Piece, prio: u32) -> u32 {
        debug_assert!(piece.len > 0, "zero-length pieces must never enter the tree");
        let node = Node {
            piece,
            prio,
            left: NIL,
            right: NIL,
            sub_len: piece.len,
            sub_nl: piece.nl,
        };
        match self.free.pop() {
            Some(i) => {
                self.nodes[i as usize] = node;
                i
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as u32
            }
        }
    }

    #[inline]
    fn pull(&mut self, n: u32) {
        if n == NIL {
            return;
        }
        let (l, r, pl, pn) = {
            let x = &self.nodes[n as usize];
            (x.left, x.right, x.piece.len, x.piece.nl)
        };
        let (sl, snl) = if l == NIL {
            (0, 0)
        } else {
            let x = &self.nodes[l as usize];
            (x.sub_len, x.sub_nl)
        };
        let (sr, snr) = if r == NIL {
            (0, 0)
        } else {
            let x = &self.nodes[r as usize];
            (x.sub_len, x.sub_nl)
        };
        let x = &mut self.nodes[n as usize];
        x.sub_len = sl + pl + sr;
        x.sub_nl = snl + pn + snr;
    }

    pub fn merge(&mut self, a: u32, b: u32) -> u32 {
        if a == NIL {
            return b;
        }
        if b == NIL {
            return a;
        }
        if self.nodes[a as usize].prio > self.nodes[b as usize].prio {
            let r = self.nodes[a as usize].right;
            let m = self.merge(r, b);
            self.nodes[a as usize].right = m;
            self.pull(a);
            a
        } else {
            let l = self.nodes[b as usize].left;
            let m = self.merge(a, l);
            self.nodes[b as usize].left = m;
            self.pull(b);
            b
        }
    }

    /// Split into (first `k` bytes, remainder).
    ///
    /// `k` is clamped to `[0, len]`. If the cut lands inside a piece, that piece
    /// is replaced by two pieces whose newline counts are recovered by scanning
    /// the two halves — bounded by one chunk, see [`super::chunk::CHUNK_CAP`].
    pub fn split(&mut self, root: u32, k: u32, src: &PieceSource) -> (u32, u32) {
        if root == NIL {
            return (NIL, NIL);
        }
        let (l, r, piece) = {
            let n = &self.nodes[root as usize];
            (n.left, n.right, n.piece)
        };
        let llen = self.sub_len(l);

        if k < llen {
            let (a, b) = self.split(l, k, src);
            self.nodes[root as usize].left = b;
            self.pull(root);
            (a, root)
        } else if k > llen + piece.len {
            let (a, b) = self.split(r, k - llen - piece.len, src);
            self.nodes[root as usize].right = a;
            self.pull(root);
            (root, b)
        } else {
            let off = k - llen;
            if off == 0 {
                // The cut falls on this node's left edge, so the left result is
                // exactly its left subtree and the right result is this node with
                // its right subtree intact. Both are already valid treaps: no
                // merge is needed, and merging here would re-attach a child that
                // is still pointed at (producing a cycle and duplicated nodes).
                self.nodes[root as usize].left = NIL;
                self.pull(root);
                (l, root)
            } else if off == piece.len {
                // Symmetric: detach the right subtree and keep this node with its
                // left subtree intact.
                self.nodes[root as usize].right = NIL;
                self.pull(root);
                (root, r)
            } else {
                let (p1, p2) = src.split_piece(piece, off);
                let prio = self.nodes[root as usize].prio;
                self.recycle(root);
                // Both halves inherit the original node's priority so the two
                // resulting trees keep their heap order.
                let n1 = self.alloc_with_prio(p1, prio);
                let n2 = self.alloc_with_prio(p2, prio);
                (self.merge(l, n1), self.merge(n2, r))
            }
        }
    }

    /// Build a balanced treap from pieces already in document order, in `O(n)`.
    ///
    /// Uses the classic stack construction of the Cartesian tree induced by the
    /// random priorities, which is both faster and flatter than folding
    /// [`Self::merge`] over the input.
    pub fn build_sorted(&mut self, pieces: &[Piece]) -> u32 {
        if pieces.is_empty() {
            return NIL;
        }
        let mut idxs: Vec<u32> = Vec::with_capacity(pieces.len());
        for &p in pieces {
            idxs.push(self.alloc(p));
        }

        let mut stack: Vec<u32> = Vec::with_capacity(64);
        for &cur in &idxs {
            let mut last = NIL;
            while let Some(&top) = stack.last() {
                // `<=` rather than `<` so ties resolve the same way `merge` resolves
                // them (the later node becomes the parent). Both orders are valid
                // heaps, but agreeing keeps the two construction paths producing the
                // same shape.
                if self.nodes[top as usize].prio <= self.nodes[cur as usize].prio {
                    last = stack.pop().expect("loop condition guarantees a node");
                } else {
                    break;
                }
            }
            self.nodes[cur as usize].left = last;
            if let Some(&top) = stack.last() {
                self.nodes[top as usize].right = cur;
            }
            stack.push(cur);
        }

        let root = stack[0];
        self.recompute_all(root);
        root
    }

    fn recompute_all(&mut self, root: u32) {
        if root == NIL {
            return;
        }
        let mut stack: Vec<(u32, bool)> = vec![(root, false)];
        while let Some((n, visited)) = stack.pop() {
            if visited {
                self.pull(n);
            } else {
                stack.push((n, true));
                let (l, r) = {
                    let x = &self.nodes[n as usize];
                    (x.left, x.right)
                };
                if l != NIL {
                    stack.push((l, false));
                }
                if r != NIL {
                    stack.push((r, false));
                }
            }
        }
    }

    /// Append a run of pieces to the end of the tree.
    pub fn push_run(&mut self, pieces: &[Piece]) {
        let mid = self.build_sorted(pieces);
        self.root = self.merge(self.root, mid);
    }

    /// Replace the whole tree with `pieces`.
    pub fn reset(&mut self, pieces: &[Piece]) {
        self.nodes.clear();
        self.free.clear();
        self.root = NIL;
        self.root = self.build_sorted(pieces);
    }

    /// Cut `[from, to)` out of the tree and append its pieces, in order, to `out`.
    pub fn take_run(&mut self, from: u32, to: u32, src: &PieceSource, out: &mut Vec<Piece>) {
        if from >= to {
            return;
        }
        let (l, rest) = self.split(self.root, from, src);
        let (mid, r) = self.split(rest, to - from, src);
        self.collect(mid, out);
        self.recycle_subtree(mid);
        self.root = self.merge(l, r);
    }

    /// Insert a run of pieces at byte offset `pos`.
    pub fn insert_run(&mut self, pos: u32, pieces: &[Piece], src: &PieceSource) {
        if pieces.is_empty() {
            return;
        }
        let (l, r) = self.split(self.root, pos, src);
        let mid = self.build_sorted(pieces);
        // Sequential rather than nested: `merge` takes `&mut self`, so the inner
        // call must complete before the outer one starts.
        let left = self.merge(l, mid);
        self.root = self.merge(left, r);
    }

    /// Copy out the pieces of a subtree, in document order.
    pub fn collect(&self, root: u32, out: &mut Vec<Piece>) {
        self.collect_in_order(root, out);
    }

    fn collect_in_order(&self, root: u32, out: &mut Vec<Piece>) {
        if root == NIL {
            return;
        }
        let mut stack = vec![(root, false)];
        while let Some((n, visited)) = stack.pop() {
            let x = &self.nodes[n as usize];
            if visited {
                out.push(x.piece);
            } else {
                // Push so the *pop* order is left, this node, right. The stack is
                // LIFO, so the right subtree must be pushed first.
                if x.right != NIL {
                    stack.push((x.right, false));
                }
                stack.push((n, true));
                if x.left != NIL {
                    stack.push((x.left, false));
                }
            }
        }
    }

    /// Flatten the whole tree into a piece list in document order.
    pub fn flatten(&self) -> Vec<Piece> {
        let mut out = Vec::with_capacity(self.piece_count());
        self.collect_in_order(self.root, &mut out);
        out
    }

    fn recycle(&mut self, n: u32) {
        if n != NIL {
            self.free.push(n);
        }
    }

    fn recycle_subtree(&mut self, root: u32) {
        if root == NIL {
            return;
        }
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            let (l, r) = {
                let x = &self.nodes[n as usize];
                (x.left, x.right)
            };
            if l != NIL {
                stack.push(l);
            }
            if r != NIL {
                stack.push(r);
            }
            self.free.push(n);
        }
    }

    /// Dump the arena for diagnosis. See [`NodeRow`] for the field order.
    pub fn dump(&self) -> Vec<NodeRow> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    i as u32,
                    n.prio,
                    n.left,
                    n.right,
                    n.piece.src,
                    n.piece.start,
                    n.piece.len,
                    n.sub_len,
                    n.sub_nl,
                )
            })
            .collect()
    }

    /// Verify the structural invariants of the tree.
    ///
    /// Walks the tree once and asserts that it is acyclic, that every node is
    /// reachable exactly once, that heap order holds, and that every cached
    /// aggregate matches a recomputation from its children.
    ///
    /// This is the check that catches a `split`/`merge` mistake immediately: a
    /// mishandled child pointer shows up either as a node reached twice (a cycle)
    /// or as a `sub_len` that disagrees with the node's own piece. Left in the
    /// crate rather than behind `cfg(test)` because it is the fastest way to
    /// diagnose a piece-table bug, and because tests in `tests/` cannot see
    /// `cfg(test)` items.
    pub fn check_invariants(&self) {
        let mut seen = vec![false; self.nodes.len()];
        let mut order: Vec<u32> = Vec::with_capacity(self.piece_count());

        // Iterative in-order walk, so a corrupt tree cannot blow the stack.
        let mut stack: Vec<u32> = Vec::new();
        let mut cur = self.root;
        while cur != NIL || !stack.is_empty() {
            while cur != NIL {
                assert!(
                    !seen[cur as usize],
                    "node {cur} reached twice: the tree has a cycle or a shared child"
                );
                seen[cur as usize] = true;
                stack.push(cur);
                cur = self.nodes[cur as usize].left;
            }
            let n = stack.pop().expect("loop condition guarantees a node");
            order.push(n);
            cur = self.nodes[n as usize].right;
        }

        assert_eq!(
            order.len(),
            self.piece_count(),
            "reachable nodes disagree with the live piece count"
        );

        for (i, &n) in order.iter().enumerate() {
            let x = &self.nodes[n as usize];
            assert!(x.piece.len > 0, "zero-length piece at index {i}");
            assert_ne!(x.piece.src, NIL, "piece with a sentinel source at index {i}");

            for c in [x.left, x.right] {
                if c != NIL {
                    let child = &self.nodes[c as usize];
                    assert!(
                        child.prio <= x.prio,
                        "heap order violated: parent node {n} (index {i}, prio {}) has child \
                         node {c} with prio {}; parent piece src{}[{}..{}] len {}, \
                         child piece src{}[{}..{}] len {}",
                        x.prio,
                        child.prio,
                        x.piece.src,
                        x.piece.start,
                        x.piece.start + x.piece.len,
                        x.piece.len,
                        child.piece.src,
                        child.piece.start,
                        child.piece.start + child.piece.len,
                        child.piece.len
                    );
                }
            }

            let (ll, lnl) = if x.left == NIL {
                (0, 0)
            } else {
                let y = &self.nodes[x.left as usize];
                (y.sub_len, y.sub_nl)
            };
            let (rl, rnl) = if x.right == NIL {
                (0, 0)
            } else {
                let y = &self.nodes[x.right as usize];
                (y.sub_len, y.sub_nl)
            };
            assert_eq!(
                x.sub_len,
                ll + x.piece.len + rl,
                "sub_len is stale at index {i}"
            );
            assert_eq!(
                x.sub_nl,
                lnl + x.piece.nl + rnl,
                "sub_nl is stale at index {i}"
            );
        }
    }

    /// Byte offset of the `k`-th newline (0-based), or `None` past the last one.
    pub fn newline_pos(&self, k: u32, src: &PieceSource) -> Option<u32> {
        let mut cur = self.root;
        let mut acc = 0u32;
        let mut k = k;
        while cur != NIL {
            let (l, r, piece) = {
                let x = &self.nodes[cur as usize];
                (x.left, x.right, x.piece)
            };
            let llen = self.sub_len(l);
            let lnl = self.sub_nl(l);
            if k < lnl {
                cur = l;
            } else if k >= lnl + piece.nl {
                acc += llen + piece.len;
                k -= lnl + piece.nl;
                cur = r;
            } else {
                acc += llen;
                let bytes = src.get(piece.src, piece.start, piece.len);
                let off = nth_newline(bytes, (k - lnl) as usize);
                return Some(acc + off as u32);
            }
        }
        None
    }

    /// Number of newlines strictly before byte offset `pos`.
    pub fn newlines_before(&self, pos: u32, src: &PieceSource) -> u32 {
        let mut cur = self.root;
        let mut n = 0u32;
        let mut pos = pos;
        while cur != NIL {
            let (l, r, piece) = {
                let x = &self.nodes[cur as usize];
                (x.left, x.right, x.piece)
            };
            let llen = self.sub_len(l);
            let lnl = self.sub_nl(l);
            if pos < llen {
                cur = l;
            } else if pos > llen + piece.len {
                n += lnl + piece.nl;
                pos -= llen + piece.len;
                cur = r;
            } else {
                n += lnl;
                let off = pos - llen;
                if off > 0 {
                    let bytes = src.get(piece.src, piece.start, off);
                    n += count_newlines(bytes);
                }
                return n;
            }
        }
        n
    }
}

/// Byte offset of the `k`-th `\n` within `bytes` (0-based).
fn nth_newline(bytes: &[u8], k: usize) -> usize {
    memchr::memchr_iter(b'\n', bytes)
        .nth(k)
        .expect("piece newline count disagrees with its bytes")
}
