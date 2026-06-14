//! Typed 3D node local transform.
//!
//! The transform half of the 3D node graph: where [`crate::light`] and
//! [`crate::material`] gave 3D-scene readers a typed landing place for
//! the *energy* (lights) and *surface response* (materials), this
//! module gives them the *placement* — the per-node local-space
//! transform and the rule for composing it up a parent chain into a
//! world-space (global) transform.
//!
//! The model is the glTF 2.0 core node transform, which we treat as the
//! canonical clean-room contract the same way the light / material
//! modules do. A node's local transform is given **either** as TRS
//! properties (a translation 3-vector, a rotation unit quaternion in
//! `XYZW` order with `W` the scalar, and a scale 3-vector) **or** as a
//! 4x4 matrix stored in column-major order. The two are unified by
//! [`NodeTransform`]:
//!
//! - [`NodeTransform::Trs`] — the animatable form. Composed to a local
//!   matrix by converting each property to a matrix and post-multiplying
//!   in `T * R * S` order: scale is applied to the vertices first, then
//!   the rotation, then the translation.
//! - [`NodeTransform::Matrix`] — a pre-baked column-major 4x4. Carried
//!   verbatim (the spec forbids skew / shear, so a writer that round-
//!   trips a matrix preserves it rather than decomposing).
//!
//! [`NodeTransform::local_matrix`] returns the 4x4 either form composes
//! to; [`Mat4`] holds it column-major (element `[col * 4 + row]`,
//! matching the glTF `matrix` accessor layout), and exposes the matrix
//! product so a consumer can fold a parent chain:
//!
//! > The global transformation matrix of a node is the product of the
//! > global transformation matrix of its parent node and its own local
//! > transformation matrix. When the node has no parent node, its
//! > global transformation matrix is identical to its local
//! > transformation matrix.
//!
//! Surface-only at this round, mirroring the lights / materials
//! bring-up: no renderer consumes node transforms yet — the type is
//! exposed so 3D-scene readers / writers have a typed landing place and
//! a single, spec-exact composition rule every consumer shares instead
//! of re-deriving the quaternion-to-matrix and `T * R * S` packing by
//! hand.
//!
//! The coordinate system is glTF's: right-handed, `+Y` up, `+Z`
//! forward, distances in meters, angles in radians, positive rotation
//! counter-clockwise.
//!
//! # Example
//!
//! ```
//! use oxideav_scene::node::{Mat4, NodeTransform};
//!
//! // A node translated 2 units along +X, with a quarter turn about +Y.
//! let local = NodeTransform::Trs {
//!     translation: [2.0, 0.0, 0.0],
//!     rotation: [0.0, (std::f32::consts::FRAC_PI_4).sin(), 0.0, (std::f32::consts::FRAC_PI_4).cos()],
//!     scale: [1.0, 1.0, 1.0],
//! };
//! let m = local.local_matrix();
//! // The translation lands in the 4th column (column-major).
//! let (x, y, z) = (m.col(3)[0], m.col(3)[1], m.col(3)[2]);
//! assert!((x - 2.0).abs() < 1e-6 && y.abs() < 1e-6 && z.abs() < 1e-6);
//! ```

/// A 4x4 single-precision matrix stored in **column-major** order.
///
/// `elements[col * 4 + row]` addresses the entry at the given column
/// and row — the same packing glTF's `node.matrix` / matrix accessors
/// use, so the array round-trips a glTF matrix verbatim. Transforms a
/// **column vector** by post-multiplication (`M * v`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    /// Column-major storage: `elements[col * 4 + row]`.
    pub elements: [f32; 16],
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    /// The 4x4 identity — glTF's default `node.matrix`.
    pub const IDENTITY: Mat4 = Mat4 {
        elements: [
            1.0, 0.0, 0.0, 0.0, // column 0
            0.0, 1.0, 0.0, 0.0, // column 1
            0.0, 0.0, 1.0, 0.0, // column 2
            0.0, 0.0, 0.0, 1.0, // column 3
        ],
    };

    /// Build from a column-major array (the glTF `matrix` layout).
    pub fn from_columns(elements: [f32; 16]) -> Self {
        Mat4 { elements }
    }

    /// The entry at `(row, col)`.
    #[inline]
    pub fn get(&self, row: usize, col: usize) -> f32 {
        self.elements[col * 4 + row]
    }

    /// Column `col` as a 4-element array `[m_0c, m_1c, m_2c, m_3c]`.
    #[inline]
    pub fn col(&self, col: usize) -> [f32; 4] {
        let base = col * 4;
        [
            self.elements[base],
            self.elements[base + 1],
            self.elements[base + 2],
            self.elements[base + 3],
        ]
    }

    /// Row `row` as a 4-element array `[m_r0, m_r1, m_r2, m_r3]`.
    #[inline]
    pub fn row(&self, row: usize) -> [f32; 4] {
        [
            self.elements[row],
            self.elements[4 + row],
            self.elements[8 + row],
            self.elements[12 + row],
        ]
    }

    /// A pure translation matrix.
    pub fn from_translation(t: [f32; 3]) -> Self {
        let mut m = Mat4::IDENTITY;
        m.elements[12] = t[0];
        m.elements[13] = t[1];
        m.elements[14] = t[2];
        m
    }

    /// A pure (non-uniform) scale matrix.
    pub fn from_scale(s: [f32; 3]) -> Self {
        let mut m = Mat4::IDENTITY;
        m.elements[0] = s[0];
        m.elements[5] = s[1];
        m.elements[10] = s[2];
        m
    }

    /// A rotation matrix from a unit quaternion `[x, y, z, w]` (`XYZW`,
    /// `w` the scalar), as glTF stores `node.rotation`.
    ///
    /// The quaternion is normalised first so a slightly-denormalised
    /// input (the common case after interpolation / file round-trips)
    /// still yields an orthonormal basis. A zero-length quaternion
    /// falls back to identity.
    pub fn from_quaternion(q: [f32; 4]) -> Self {
        let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if len <= f32::EPSILON {
            return Mat4::IDENTITY;
        }
        let inv = 1.0 / len;
        let (x, y, z, w) = (q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv);

        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);

        // Column-major rotation matrix for a right-handed system.
        Mat4 {
            elements: [
                // column 0
                1.0 - 2.0 * (yy + zz),
                2.0 * (xy + wz),
                2.0 * (xz - wy),
                0.0,
                // column 1
                2.0 * (xy - wz),
                1.0 - 2.0 * (xx + zz),
                2.0 * (yz + wx),
                0.0,
                // column 2
                2.0 * (xz + wy),
                2.0 * (yz - wx),
                1.0 - 2.0 * (xx + yy),
                0.0,
                // column 3
                0.0,
                0.0,
                0.0,
                1.0,
            ],
        }
    }

    /// Matrix product `self * rhs` (post-multiplication).
    ///
    /// Composing transforms left-to-right means the right-hand operand
    /// is applied to a vertex first: `(A * B) * v == A * (B * v)`.
    pub fn mul(&self, rhs: &Mat4) -> Mat4 {
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    // self[row, k] * rhs[k, col]
                    sum += self.elements[k * 4 + row] * rhs.elements[col * 4 + k];
                }
                out[col * 4 + row] = sum;
            }
        }
        Mat4 { elements: out }
    }

    /// Transform a point `[x, y, z]` (implicit `w = 1`), returning the
    /// transformed `[x, y, z]` after the perspective divide. For an
    /// affine TRS / matrix transform `w` stays `1`, but the divide is
    /// applied defensively so a projection matrix also behaves.
    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let x = self.elements[0] * p[0]
            + self.elements[4] * p[1]
            + self.elements[8] * p[2]
            + self.elements[12];
        let y = self.elements[1] * p[0]
            + self.elements[5] * p[1]
            + self.elements[9] * p[2]
            + self.elements[13];
        let z = self.elements[2] * p[0]
            + self.elements[6] * p[1]
            + self.elements[10] * p[2]
            + self.elements[14];
        let w = self.elements[3] * p[0]
            + self.elements[7] * p[1]
            + self.elements[11] * p[2]
            + self.elements[15];
        if w != 0.0 && w != 1.0 {
            [x / w, y / w, z / w]
        } else {
            [x, y, z]
        }
    }

    /// Transform a direction `[x, y, z]` (implicit `w = 0`) — the
    /// translation column is ignored, so a unit axis is rotated /
    /// scaled but not displaced.
    pub fn transform_direction(&self, d: [f32; 3]) -> [f32; 3] {
        [
            self.elements[0] * d[0] + self.elements[4] * d[1] + self.elements[8] * d[2],
            self.elements[1] * d[0] + self.elements[5] * d[1] + self.elements[9] * d[2],
            self.elements[2] * d[0] + self.elements[6] * d[1] + self.elements[10] * d[2],
        ]
    }
}

/// A node's local-space transform, in either of glTF's two forms.
///
/// A glTF node carries **either** TRS properties **or** a `matrix`,
/// never both; [`NodeTransform`] enforces that by being an enum.
/// [`NodeTransform::local_matrix`] collapses either form to the same
/// [`Mat4`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum NodeTransform {
    /// Translation / rotation / scale properties — the animatable form.
    ///
    /// `rotation` is a unit quaternion `[x, y, z, w]` (`XYZW`, `w`
    /// scalar). The local matrix is `T * R * S`.
    Trs {
        /// Local-space translation `[x, y, z]` (meters).
        translation: [f32; 3],
        /// Local-space rotation unit quaternion `[x, y, z, w]`.
        rotation: [f32; 4],
        /// Local-space scale `[x, y, z]`.
        scale: [f32; 3],
    },
    /// A pre-baked column-major 4x4 local matrix.
    Matrix(Mat4),
}

impl Default for NodeTransform {
    /// glTF's "no transform properties" node: the identity.
    fn default() -> Self {
        NodeTransform::IDENTITY
    }
}

impl NodeTransform {
    /// The identity TRS: zero translation, identity rotation, unit
    /// scale — what a glTF node with no transform properties resolves
    /// to.
    pub const IDENTITY: NodeTransform = NodeTransform::Trs {
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };

    /// A pure-translation TRS transform.
    pub fn from_translation(translation: [f32; 3]) -> Self {
        NodeTransform::Trs {
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    /// A pure-rotation TRS transform from a quaternion `[x, y, z, w]`.
    pub fn from_rotation(rotation: [f32; 4]) -> Self {
        NodeTransform::Trs {
            translation: [0.0, 0.0, 0.0],
            rotation,
            scale: [1.0, 1.0, 1.0],
        }
    }

    /// A pure-scale TRS transform.
    pub fn from_scale(scale: [f32; 3]) -> Self {
        NodeTransform::Trs {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale,
        }
    }

    /// `true` for the TRS form (the only form that may be animated).
    pub fn is_trs(&self) -> bool {
        matches!(self, NodeTransform::Trs { .. })
    }

    /// `true` for the pre-baked matrix form.
    pub fn is_matrix(&self) -> bool {
        matches!(self, NodeTransform::Matrix(_))
    }

    /// Compose the local-space 4x4 matrix this transform represents.
    ///
    /// For [`NodeTransform::Trs`] the result is `T * R * S` (scale
    /// applied first, then rotation, then translation). For
    /// [`NodeTransform::Matrix`] the carried matrix is returned
    /// verbatim.
    pub fn local_matrix(&self) -> Mat4 {
        match self {
            NodeTransform::Trs {
                translation,
                rotation,
                scale,
            } => {
                let t = Mat4::from_translation(*translation);
                let r = Mat4::from_quaternion(*rotation);
                let s = Mat4::from_scale(*scale);
                // T * R * S
                t.mul(&r).mul(&s)
            }
            NodeTransform::Matrix(m) => *m,
        }
    }
}

/// A node in a 3D node hierarchy: a name, a local transform, and the
/// indices of its children in some flat node array.
///
/// The graph itself is **flat and index-addressed** — a
/// [`NodeGraph`] owns the `Vec<SceneNode>`, and a node refers to its
/// children by their position in that vector. This mirrors how glTF
/// stores its node array (`node.children` is a list of indices into the
/// top-level `nodes` array) and keeps the type `Copy`-cheap to clone
/// without `Rc` cycles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneNode {
    /// Optional human-readable name (glTF `node.name`).
    pub name: String,
    /// This node's local-space transform.
    pub transform: NodeTransform,
    /// Indices of this node's children in the owning [`NodeGraph`].
    pub children: Vec<usize>,
}

impl SceneNode {
    /// A named identity node with no children.
    pub fn named(name: impl Into<String>) -> Self {
        SceneNode {
            name: name.into(),
            transform: NodeTransform::IDENTITY,
            children: Vec::new(),
        }
    }

    /// This node's local 4x4 matrix (`self.transform.local_matrix()`).
    pub fn local_matrix(&self) -> Mat4 {
        self.transform.local_matrix()
    }
}

/// A flat, index-addressed 3D node hierarchy.
///
/// Nodes live in `nodes`; `roots` lists the indices of nodes with no
/// parent (glTF `scene.nodes`). The hierarchy MUST be a set of disjoint
/// strict trees — no cycles, each node at most one parent — and the
/// traversal helpers below assume that invariant.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeGraph {
    /// All nodes, flat. Children reference each other by index.
    pub nodes: Vec<SceneNode>,
    /// Indices of root nodes (no parent).
    pub roots: Vec<usize>,
}

impl NodeGraph {
    /// An empty graph.
    pub fn new() -> Self {
        NodeGraph::default()
    }

    /// Append a node and return its index. Does **not** mark it a root
    /// or attach it as anyone's child — the caller wires `roots` /
    /// `children` explicitly.
    pub fn push(&mut self, node: SceneNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(node);
        idx
    }

    /// Append a node and record it as a root. Returns its index.
    pub fn push_root(&mut self, node: SceneNode) -> usize {
        let idx = self.push(node);
        self.roots.push(idx);
        idx
    }

    /// Number of nodes in the graph.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// `true` when the graph holds no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node at `index`, or `None` when out of range — so a stale
    /// index from an external file can't panic.
    pub fn node(&self, index: usize) -> Option<&SceneNode> {
        self.nodes.get(index)
    }

    /// The global (world-space) matrix of the node at `index`: the
    /// product of its parent chain's local matrices down to it,
    /// outermost first (`root_local * … * node_local`).
    ///
    /// Per the spec, the global matrix of a node is the product of its
    /// parent's global matrix and its own local matrix; a root node's
    /// global matrix is its local matrix. Returns `None` for an
    /// out-of-range index or when `index` is not reachable from any
    /// root (an orphan / unparented non-root node has no defined world
    /// placement).
    pub fn global_matrix(&self, index: usize) -> Option<Mat4> {
        if index >= self.nodes.len() {
            return None;
        }
        // Walk down from every root, accumulating the matrix product,
        // until we reach `index`. Disjoint strict trees guarantee a
        // single path if one exists.
        for &root in &self.roots {
            if let Some(m) = self.descend(root, Mat4::IDENTITY, index) {
                return Some(m);
            }
        }
        None
    }

    /// Depth-first search from `current` (whose accumulated parent
    /// matrix is `acc_parent`) for `target`, returning the target's
    /// global matrix when found.
    fn descend(&self, current: usize, acc_parent: Mat4, target: usize) -> Option<Mat4> {
        let node = self.nodes.get(current)?;
        let global = acc_parent.mul(&node.local_matrix());
        if current == target {
            return Some(global);
        }
        for &child in &node.children {
            // Guard against a malformed self-referential child to avoid
            // unbounded recursion on a cyclic input.
            if child == current {
                continue;
            }
            if let Some(found) = self.descend(child, global, target) {
                return Some(found);
            }
        }
        None
    }

    /// Visit every node reachable from the roots in depth-first paint
    /// order, invoking `visit(index, &node, global_matrix)`.
    ///
    /// The global matrix is accumulated on the way down, so each node is
    /// visited exactly once with its correct world transform — far
    /// cheaper than calling [`Self::global_matrix`] per node (which
    /// re-walks from the roots each time).
    pub fn visit<F: FnMut(usize, &SceneNode, Mat4)>(&self, mut visit: F) {
        for &root in &self.roots {
            self.visit_from(root, Mat4::IDENTITY, &mut visit);
        }
    }

    fn visit_from<F: FnMut(usize, &SceneNode, Mat4)>(
        &self,
        current: usize,
        acc_parent: Mat4,
        visit: &mut F,
    ) {
        let Some(node) = self.nodes.get(current) else {
            return;
        };
        let global = acc_parent.mul(&node.local_matrix());
        visit(current, node, global);
        for &child in &node.children {
            if child == current {
                continue;
            }
            self.visit_from(child, global, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn approx3(a: [f32; 3], b: [f32; 3]) -> bool {
        approx(a[0], b[0]) && approx(a[1], b[1]) && approx(a[2], b[2])
    }

    #[test]
    fn identity_defaults() {
        assert_eq!(Mat4::default(), Mat4::IDENTITY);
        assert_eq!(NodeTransform::default(), NodeTransform::IDENTITY);
        // Identity TRS composes to the identity matrix.
        assert_eq!(NodeTransform::IDENTITY.local_matrix(), Mat4::IDENTITY);
    }

    #[test]
    fn column_major_layout() {
        // Translation lands in the 4th column for a column-major store.
        let m = Mat4::from_translation([2.0, 3.0, 4.0]);
        assert_eq!(m.col(3), [2.0, 3.0, 4.0, 1.0]);
        assert_eq!(m.get(0, 3), 2.0);
        assert_eq!(m.get(1, 3), 3.0);
        assert_eq!(m.get(2, 3), 4.0);
        // get(row, col) and row()/col() agree.
        assert_eq!(m.row(3), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn identity_quaternion_is_identity() {
        assert_eq!(Mat4::from_quaternion([0.0, 0.0, 0.0, 1.0]), Mat4::IDENTITY);
        // Degenerate (zero) quaternion falls back to identity.
        assert_eq!(Mat4::from_quaternion([0.0, 0.0, 0.0, 0.0]), Mat4::IDENTITY);
    }

    #[test]
    fn quaternion_normalised_before_use() {
        // A non-unit quaternion (2x identity scalar) still yields the
        // identity rotation after internal normalisation.
        let m = Mat4::from_quaternion([0.0, 0.0, 0.0, 2.0]);
        for i in 0..16 {
            assert!(approx(m.elements[i], Mat4::IDENTITY.elements[i]));
        }
    }

    #[test]
    fn rotate_about_y_quarter_turn() {
        // +90° about +Y (right-handed, CCW looking down -Y): +X axis
        // maps toward -Z.
        let q = [0.0, FRAC_PI_4.sin(), 0.0, FRAC_PI_4.cos()];
        let m = Mat4::from_quaternion(q);
        let x_axis = m.transform_direction([1.0, 0.0, 0.0]);
        assert!(approx3(x_axis, [0.0, 0.0, -1.0]));
        let z_axis = m.transform_direction([0.0, 0.0, 1.0]);
        assert!(approx3(z_axis, [1.0, 0.0, 0.0]));
    }

    #[test]
    fn rotate_about_z_half_turn() {
        // 180° about +Z: +X -> -X, +Y -> -Y.
        let q = [0.0, 0.0, 1.0, 0.0]; // (sin90, cos90) -> (0,0,1,0)
        let m = Mat4::from_quaternion(q);
        assert!(approx3(
            m.transform_direction([1.0, 0.0, 0.0]),
            [-1.0, 0.0, 0.0]
        ));
        assert!(approx3(
            m.transform_direction([0.0, 1.0, 0.0]),
            [0.0, -1.0, 0.0]
        ));
        assert!(approx3(
            m.transform_direction([0.0, 0.0, 1.0]),
            [0.0, 0.0, 1.0]
        ));
    }

    #[test]
    fn trs_order_scale_then_rotate_then_translate() {
        // Scale (2,2,2), rotate 90° about +Z, translate (10,0,0).
        // A point at local (1,0,0):
        //   scale  -> (2,0,0)
        //   rotate -> (0,2,0)
        //   transl -> (10,2,0)
        let t = NodeTransform::Trs {
            translation: [10.0, 0.0, 0.0],
            rotation: [0.0, 0.0, FRAC_PI_4.sin(), FRAC_PI_4.cos()],
            scale: [2.0, 2.0, 2.0],
        };
        let m = t.local_matrix();
        let p = m.transform_point([1.0, 0.0, 0.0]);
        assert!(approx3(p, [10.0, 2.0, 0.0]), "{p:?}");
    }

    #[test]
    fn matrix_form_returned_verbatim() {
        let baked = Mat4::from_columns([
            2.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 4.0, 0.0, 5.0, 6.0, 7.0, 1.0,
        ]);
        let t = NodeTransform::Matrix(baked);
        assert!(t.is_matrix());
        assert!(!t.is_trs());
        assert_eq!(t.local_matrix(), baked);
    }

    #[test]
    fn matrix_product_associates_with_composition() {
        let a = Mat4::from_translation([1.0, 0.0, 0.0]);
        let b = Mat4::from_scale([2.0, 2.0, 2.0]);
        // (A * B) applied to v == A * (B * v): B (scale) first.
        let ab = a.mul(&b);
        let v = [1.0, 1.0, 1.0];
        let direct = ab.transform_point(v);
        let stepwise = a.transform_point(b.transform_point(v));
        assert!(approx3(direct, stepwise));
        // scale then translate: (1,1,1) -> (2,2,2) -> (3,2,2)
        assert!(approx3(direct, [3.0, 2.0, 2.0]));
    }

    #[test]
    fn identity_is_multiplicative_unit() {
        let m = Mat4::from_translation([7.0, 8.0, 9.0]);
        assert_eq!(Mat4::IDENTITY.mul(&m), m);
        assert_eq!(m.mul(&Mat4::IDENTITY), m);
    }

    #[test]
    fn constructors_match_trs() {
        assert_eq!(
            NodeTransform::from_translation([1.0, 2.0, 3.0]).local_matrix(),
            Mat4::from_translation([1.0, 2.0, 3.0])
        );
        assert_eq!(
            NodeTransform::from_scale([2.0, 3.0, 4.0]).local_matrix(),
            Mat4::from_scale([2.0, 3.0, 4.0])
        );
        let q = [0.0, 0.0, FRAC_PI_4.sin(), FRAC_PI_4.cos()];
        assert_eq!(
            NodeTransform::from_rotation(q).local_matrix(),
            Mat4::from_quaternion(q)
        );
    }

    #[test]
    fn graph_global_matrix_composes_parent_chain() {
        // root (translate +X by 10) -> child (translate +X by 5).
        // child global places a local origin at world (15, 0, 0).
        let mut g = NodeGraph::new();
        let child = g.push(SceneNode {
            name: "child".into(),
            transform: NodeTransform::from_translation([5.0, 0.0, 0.0]),
            children: vec![],
        });
        let root = g.push(SceneNode {
            name: "root".into(),
            transform: NodeTransform::from_translation([10.0, 0.0, 0.0]),
            children: vec![child],
        });
        g.roots.push(root);

        let gm = g.global_matrix(child).unwrap();
        assert!(approx3(
            gm.transform_point([0.0, 0.0, 0.0]),
            [15.0, 0.0, 0.0]
        ));

        // Root's global == its local.
        let rm = g.global_matrix(root).unwrap();
        assert_eq!(rm, g.nodes[root].local_matrix());
    }

    #[test]
    fn graph_parent_rotation_applies_to_child_translation() {
        // root rotates 90° about +Z; child is translated +X by 1 in the
        // root's local space, so its world origin lands at (0, 1, 0).
        let mut g = NodeGraph::new();
        let child = g.push(SceneNode {
            name: "child".into(),
            transform: NodeTransform::from_translation([1.0, 0.0, 0.0]),
            children: vec![],
        });
        // 90° about +Z is the quaternion (0, 0, sin(45°), cos(45°)).
        g.push_root(SceneNode {
            name: "root".into(),
            transform: NodeTransform::from_rotation([0.0, 0.0, FRAC_PI_4.sin(), FRAC_PI_4.cos()]),
            children: vec![child],
        });
        let gm = g.global_matrix(child).unwrap();
        assert!(approx3(
            gm.transform_point([0.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));
    }

    #[test]
    fn graph_orphan_and_oob_return_none() {
        let mut g = NodeGraph::new();
        // Pushed but never wired as root or child: orphan.
        let orphan = g.push(SceneNode::named("orphan"));
        assert!(g.global_matrix(orphan).is_none());
        assert!(g.global_matrix(99).is_none());
        assert!(g.node(99).is_none());
    }

    #[test]
    fn graph_visit_hits_every_node_once_with_world_transform() {
        let mut g = NodeGraph::new();
        let c = g.push(SceneNode {
            name: "c".into(),
            transform: NodeTransform::from_translation([5.0, 0.0, 0.0]),
            children: vec![],
        });
        let r = g.push_root(SceneNode {
            name: "r".into(),
            transform: NodeTransform::from_translation([10.0, 0.0, 0.0]),
            children: vec![c],
        });

        let mut seen: Vec<(usize, [f32; 3])> = Vec::new();
        g.visit(|idx, _node, gm| {
            seen.push((idx, gm.transform_point([0.0, 0.0, 0.0])));
        });
        assert_eq!(seen.len(), 2);
        // Root visited first (paint order), then child.
        assert_eq!(seen[0].0, r);
        assert!(approx3(seen[0].1, [10.0, 0.0, 0.0]));
        assert_eq!(seen[1].0, c);
        assert!(approx3(seen[1].1, [15.0, 0.0, 0.0]));
    }

    #[test]
    fn graph_cycle_guard_does_not_recurse_forever() {
        // A node listing itself as a child must not hang the traversal.
        let mut g = NodeGraph::new();
        let idx = g.push(SceneNode::named("self"));
        g.nodes[idx].children.push(idx);
        g.roots.push(idx);
        // Both global_matrix and visit terminate.
        assert!(g.global_matrix(idx).is_some());
        let mut count = 0;
        g.visit(|_, _, _| count += 1);
        assert_eq!(count, 1);
    }
}
