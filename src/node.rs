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

    /// The transpose: rows become columns. For a pure rotation the
    /// transpose is the inverse (orthonormal basis).
    pub fn transpose(&self) -> Mat4 {
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[row * 4 + col] = self.elements[col * 4 + row];
            }
        }
        Mat4 { elements: out }
    }

    /// The determinant of the 3x3 minor obtained by deleting
    /// `skip_row` / `skip_col`.
    fn minor3(&self, skip_row: usize, skip_col: usize) -> f32 {
        let mut sub = [0.0f32; 9];
        let mut i = 0;
        for col in 0..4 {
            if col == skip_col {
                continue;
            }
            for row in 0..4 {
                if row == skip_row {
                    continue;
                }
                sub[i] = self.get(row, col);
                i += 1;
            }
        }
        // sub is column-major 3x3.
        sub[0] * (sub[4] * sub[8] - sub[5] * sub[7]) - sub[3] * (sub[1] * sub[8] - sub[2] * sub[7])
            + sub[6] * (sub[1] * sub[5] - sub[2] * sub[4])
    }

    /// The determinant, by cofactor expansion along the first column.
    ///
    /// For a TRS matrix this is `sx * sy * sz` (rotation contributes
    /// `1`, translation nothing) — a negative value means the basis is
    /// mirrored, zero means the transform collapses a dimension (and
    /// [`Mat4::inverse`] returns `None`).
    pub fn determinant(&self) -> f32 {
        let mut det = 0.0;
        let mut sign = 1.0;
        for row in 0..4 {
            det += sign * self.get(row, 0) * self.minor3(row, 0);
            sign = -sign;
        }
        det
    }

    /// The inverse matrix, or `None` when the matrix is singular (zero
    /// determinant — e.g. a scale of zero on some axis) or non-finite.
    ///
    /// Computed via the adjugate: `inv[r][c] = (-1)^(r+c) *
    /// minor(c, r) / det`. `M.mul(&M.inverse().unwrap())` is the
    /// identity up to floating-point rounding.
    pub fn inverse(&self) -> Option<Mat4> {
        let det = self.determinant();
        if !det.is_finite() || det.abs() <= f32::MIN_POSITIVE {
            return None;
        }
        let inv_det = 1.0 / det;
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let sign = if (row + col) % 2 == 0 { 1.0 } else { -1.0 };
                // Adjugate: cofactor of the *transposed* position.
                out[col * 4 + row] = sign * self.minor3(col, row) * inv_det;
            }
        }
        Some(Mat4 { elements: out })
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

    /// Decompose an affine, shear-free matrix into
    /// `(translation, rotation quaternion XYZW, scale)` — the inverse
    /// of the `T * R * S` composition.
    ///
    /// The spec requires a node `matrix` to be decomposable to TRS
    /// properties (transformation matrices cannot skew or shear), so
    /// this returns `None` exactly when the input steps outside that
    /// contract:
    ///
    /// - the bottom row is not `(0, 0, 0, 1)` (not affine);
    /// - any component is non-finite;
    /// - a basis column has (near-)zero length (a collapsed axis has
    ///   no recoverable rotation);
    /// - the scale-normalised basis is not orthogonal (shear).
    ///
    /// A mirrored basis (negative determinant) is decomposed by
    /// negating the X scale. The returned quaternion is unit-length;
    /// note `q` and `-q` encode the same rotation, so round-trip
    /// comparisons should recompose to matrices rather than compare
    /// raw components.
    pub fn decompose_trs(&self) -> Option<([f32; 3], [f32; 4], [f32; 3])> {
        if !self.elements.iter().all(|v| v.is_finite()) {
            return None;
        }
        // Affine: bottom row must be (0, 0, 0, 1).
        let eps = 1e-5;
        let bottom = self.row(3);
        if (bottom[0]).abs() > eps
            || (bottom[1]).abs() > eps
            || (bottom[2]).abs() > eps
            || (bottom[3] - 1.0).abs() > eps
        {
            return None;
        }

        let translation = [self.elements[12], self.elements[13], self.elements[14]];

        // Basis columns of the upper-left 3x3.
        let mut cols = [
            [self.elements[0], self.elements[1], self.elements[2]],
            [self.elements[4], self.elements[5], self.elements[6]],
            [self.elements[8], self.elements[9], self.elements[10]],
        ];
        let mut scale = [0.0f32; 3];
        for (axis, col) in cols.iter().enumerate() {
            let len = (col[0] * col[0] + col[1] * col[1] + col[2] * col[2]).sqrt();
            scale[axis] = len;
        }
        if scale.iter().any(|&s| s <= f32::EPSILON) {
            return None; // collapsed axis: rotation unrecoverable
        }

        // Mirrored basis: fold the reflection into a negative X scale.
        let det3 = {
            let [c0, c1, c2] = cols;
            c0[0] * (c1[1] * c2[2] - c1[2] * c2[1]) - c1[0] * (c0[1] * c2[2] - c0[2] * c2[1])
                + c2[0] * (c0[1] * c1[2] - c0[2] * c1[1])
        };
        if det3 < 0.0 {
            scale[0] = -scale[0];
        }

        // Normalise columns into a candidate rotation basis.
        for (axis, col) in cols.iter_mut().enumerate() {
            let inv = 1.0 / scale[axis];
            col[0] *= inv;
            col[1] *= inv;
            col[2] *= inv;
        }

        // Shear check: the basis must be orthogonal.
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let ortho_eps = 1e-4;
        if dot(cols[0], cols[1]).abs() > ortho_eps
            || dot(cols[0], cols[2]).abs() > ortho_eps
            || dot(cols[1], cols[2]).abs() > ortho_eps
        {
            return None;
        }

        // Quaternion from the orthonormal rotation basis, branching on
        // the largest of (trace, m00, m11, m22) for numeric stability.
        let (m00, m11, m22) = (cols[0][0], cols[1][1], cols[2][2]);
        let (m01, m02) = (cols[1][0], cols[2][0]); // m[row][col]
        let (m10, m12) = (cols[0][1], cols[2][1]);
        let (m20, m21) = (cols[0][2], cols[1][2]);
        let trace = m00 + m11 + m22;
        let q = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0; // 4w
            [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
        } else if m00 >= m11 && m00 >= m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0; // 4x
            [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
        } else if m11 >= m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0; // 4y
            [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0; // 4z
            [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
        };
        // Normalise to absorb accumulated rounding.
        let qlen = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        let rotation = [q[0] / qlen, q[1] / qlen, q[2] / qlen, q[3] / qlen];

        Some((translation, rotation, scale))
    }
}

/// Dot product of two quaternions (as plain 4-vectors, XYZW).
///
/// `1.0` for identical unit quaternions, `-1.0` for exact opposites
/// (which encode the *same* rotation — see [`quat_slerp`]'s
/// shortest-path handling).
pub fn quat_dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// Normalise a quaternion to unit length. A (near-)zero-length input
/// falls back to the identity quaternion `[0, 0, 0, 1]`, matching
/// [`Mat4::from_quaternion`]'s degenerate-input rule.
pub fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let len = quat_dot(q, q).sqrt();
    if len <= f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let inv = 1.0 / len;
    [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
}

/// The conjugate `[-x, -y, -z, w]` — for a unit quaternion, the
/// inverse rotation.
pub fn quat_conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Quaternion product `a * b` (XYZW storage, `w` scalar).
///
/// Composition order matches the matrix product: rotating by
/// `quat_mul(a, b)` applies `b` first, then `a`, i.e.
/// `Mat4::from_quaternion(quat_mul(a, b))` equals
/// `Mat4::from_quaternion(a).mul(&Mat4::from_quaternion(b))`.
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// A unit quaternion rotating by `angle` radians about `axis`
/// (right-handed, counter-clockwise looking down the axis toward the
/// origin). The axis is normalised first; a zero axis yields the
/// identity quaternion.
pub fn quat_from_axis_angle(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len <= f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let (s, c) = (angle * 0.5).sin_cos();
    let k = s / len;
    [axis[0] * k, axis[1] * k, axis[2] * k, c]
}

/// Spherical linear interpolation between two unit quaternions, the
/// rotation-channel rule of the animation sampler (Appendix C.4 of
/// the glTF 2.0 spec).
///
/// With `a = arccos(|v_k . v_{k+1}|)` and `s` the sign of the dot
/// product,
///
/// ```text
/// v_t = sin(a(1 - t)) / sin(a) * v_k  +  s * sin(a t) / sin(a) * v_{k+1}
/// ```
///
/// Taking the absolute value for the angle and multiplying the second
/// endpoint by the dot's sign keeps the interpolation on the short
/// path along the great circle. When the angle is close to zero the
/// spherical weights collapse to regular linear interpolation, which
/// is used directly (then re-normalised). `t` is clamped to
/// `[0, 1]`; inputs are normalised defensively.
pub fn quat_slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let qa = quat_normalize(a);
    let qb = quat_normalize(b);
    let dot = quat_dot(qa, qb);
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let angle = dot.abs().min(1.0).acos();
    let sin_angle = angle.sin();
    let (wa, wb) = if sin_angle > 1e-4 {
        (
            ((1.0 - t) * angle).sin() / sin_angle,
            sign * (t * angle).sin() / sin_angle,
        )
    } else {
        // Near-parallel endpoints: spherical turns into linear.
        (1.0 - t, sign * t)
    };
    quat_normalize([
        wa * qa[0] + wb * qb[0],
        wa * qa[1] + wb * qb[1],
        wa * qa[2] + wb * qb[2],
        wa * qa[3] + wb * qb[3],
    ])
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

    /// This transform as TRS properties — the animatable form.
    ///
    /// [`NodeTransform::Trs`] is returned as-is;
    /// [`NodeTransform::Matrix`] is decomposed via
    /// [`Mat4::decompose_trs`], returning `None` when the matrix
    /// steps outside the spec's decomposability contract (shear,
    /// collapsed axis, non-affine, non-finite).
    pub fn to_trs(&self) -> Option<NodeTransform> {
        match self {
            NodeTransform::Trs { .. } => Some(*self),
            NodeTransform::Matrix(m) => {
                let (translation, rotation, scale) = m.decompose_trs()?;
                Some(NodeTransform::Trs {
                    translation,
                    rotation,
                    scale,
                })
            }
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

/// A structural or numeric defect found by [`NodeGraph::validate`].
///
/// Every variant carries the index (or indices) involved so a caller
/// can point at the offending node in an imported file. The spec's
/// hierarchy contract is "a set of disjoint strict trees": each node
/// has at most one parent, no node is its own ancestor, and roots are
/// exactly the parentless nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeGraphError {
    /// `roots` lists an index past the end of `nodes`.
    RootOutOfRange {
        /// The offending root index.
        root: usize,
        /// Number of nodes in the graph.
        len: usize,
    },
    /// The same index appears in `roots` more than once.
    DuplicateRoot {
        /// The repeated root index.
        root: usize,
    },
    /// A node's `children` lists an index past the end of `nodes`.
    ChildOutOfRange {
        /// The node whose child list is broken.
        node: usize,
        /// The out-of-range child index it lists.
        child: usize,
        /// Number of nodes in the graph.
        len: usize,
    },
    /// A node is claimed as a child by two parents (or twice by the
    /// same parent) — the hierarchy must be a strict tree.
    MultipleParents {
        /// The doubly-claimed child.
        child: usize,
        /// The parent that claimed it first.
        first_parent: usize,
        /// The parent that claimed it again.
        second_parent: usize,
    },
    /// An index listed in `roots` is also some node's child — roots
    /// are by definition parentless.
    RootHasParent {
        /// The offending root index.
        root: usize,
        /// The node claiming it as a child.
        parent: usize,
    },
    /// A node is its own ancestor (the parent chain loops).
    Cycle {
        /// A node on the loop.
        node: usize,
    },
    /// A node's transform carries a NaN or infinite component.
    NonFiniteTransform {
        /// The offending node.
        node: usize,
    },
    /// A node's TRS rotation quaternion has (near-)zero length, so it
    /// cannot be normalised into the unit quaternion the spec
    /// requires.
    ZeroRotation {
        /// The offending node.
        node: usize,
    },
}

impl std::fmt::Display for NodeGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeGraphError::RootOutOfRange { root, len } => {
                write!(f, "root index {root} out of range (graph has {len} nodes)")
            }
            NodeGraphError::DuplicateRoot { root } => {
                write!(f, "root index {root} listed more than once")
            }
            NodeGraphError::ChildOutOfRange { node, child, len } => {
                write!(
                    f,
                    "node {node} lists child {child} out of range (graph has {len} nodes)"
                )
            }
            NodeGraphError::MultipleParents {
                child,
                first_parent,
                second_parent,
            } => {
                write!(
                    f,
                    "node {child} claimed as child by both node {first_parent} and node {second_parent}"
                )
            }
            NodeGraphError::RootHasParent { root, parent } => {
                write!(f, "root {root} is also a child of node {parent}")
            }
            NodeGraphError::Cycle { node } => {
                write!(f, "node {node} is its own ancestor (cycle)")
            }
            NodeGraphError::NonFiniteTransform { node } => {
                write!(f, "node {node} transform has a NaN / infinite component")
            }
            NodeGraphError::ZeroRotation { node } => {
                write!(
                    f,
                    "node {node} rotation quaternion has zero length (not normalisable)"
                )
            }
        }
    }
}

impl std::error::Error for NodeGraphError {}

/// A flat, index-addressed 3D node hierarchy.
///
/// Nodes live in `nodes`; `roots` lists the indices of nodes with no
/// parent (glTF `scene.nodes`). The hierarchy MUST be a set of disjoint
/// strict trees — no cycles, each node at most one parent — and the
/// traversal helpers below assume that invariant.
/// [`NodeGraph::validate`] checks it for untrusted input.
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

    /// The parent of every node, computed by scanning each node's
    /// `children` list: `result[i]` is `Some(p)` when node `p` lists
    /// `i` as a child, `None` for roots / orphans.
    ///
    /// When a node is claimed by several parents (invalid — see
    /// [`NodeGraph::validate`]) the first claimant in iteration order
    /// wins. O(total child references).
    pub fn parent_indices(&self) -> Vec<Option<usize>> {
        let mut parents = vec![None; self.nodes.len()];
        for (i, node) in self.nodes.iter().enumerate() {
            for &child in &node.children {
                if let Some(slot) = parents.get_mut(child) {
                    if slot.is_none() {
                        *slot = Some(i);
                    }
                }
            }
        }
        parents
    }

    /// The parent of the node at `index`, or `None` when the node is a
    /// root / orphan or `index` is out of range.
    ///
    /// Convenience over a full [`NodeGraph::parent_indices`] scan —
    /// callers resolving many nodes should compute the map once.
    pub fn parent_index(&self, index: usize) -> Option<usize> {
        if index >= self.nodes.len() {
            return None;
        }
        self.parent_indices()[index]
    }

    /// Check the strict-tree hierarchy invariant plus per-node numeric
    /// sanity, returning the first defect found.
    ///
    /// Verified, in order:
    ///
    /// 1. every index in `roots` is in range and listed once
    ///    ([`NodeGraphError::RootOutOfRange`] /
    ///    [`NodeGraphError::DuplicateRoot`]);
    /// 2. every child index is in range
    ///    ([`NodeGraphError::ChildOutOfRange`]);
    /// 3. no node has two parents
    ///    ([`NodeGraphError::MultipleParents`]);
    /// 4. no root is also somebody's child
    ///    ([`NodeGraphError::RootHasParent`]);
    /// 5. no node is its own ancestor ([`NodeGraphError::Cycle`]);
    /// 6. every transform component is finite
    ///    ([`NodeGraphError::NonFiniteTransform`]) and every TRS
    ///    rotation quaternion is normalisable
    ///    ([`NodeGraphError::ZeroRotation`]).
    ///
    /// Orphan nodes (parentless but not listed as roots) are legal —
    /// they simply have no world placement (see
    /// [`NodeGraph::global_matrix`]).
    pub fn validate(&self) -> Result<(), NodeGraphError> {
        let len = self.nodes.len();

        // 1. Roots in range, no duplicates.
        let mut is_root = vec![false; len];
        for &root in &self.roots {
            if root >= len {
                return Err(NodeGraphError::RootOutOfRange { root, len });
            }
            if is_root[root] {
                return Err(NodeGraphError::DuplicateRoot { root });
            }
            is_root[root] = true;
        }

        // 2 + 3. Children in range; at most one parent each.
        let mut parents: Vec<Option<usize>> = vec![None; len];
        for (i, node) in self.nodes.iter().enumerate() {
            for &child in &node.children {
                if child >= len {
                    return Err(NodeGraphError::ChildOutOfRange {
                        node: i,
                        child,
                        len,
                    });
                }
                match parents[child] {
                    Some(first_parent) => {
                        return Err(NodeGraphError::MultipleParents {
                            child,
                            first_parent,
                            second_parent: i,
                        });
                    }
                    None => parents[child] = Some(i),
                }
            }
        }

        // 4. Roots are parentless.
        for (child, parent) in parents.iter().enumerate() {
            if let Some(parent) = *parent {
                if is_root[child] {
                    return Err(NodeGraphError::RootHasParent {
                        root: child,
                        parent,
                    });
                }
            }
        }

        // 5. Parent chains terminate. With at-most-one-parent already
        // established, a chain longer than `len` steps must loop.
        for start in 0..len {
            let mut current = start;
            let mut steps = 0usize;
            while let Some(parent) = parents[current] {
                steps += 1;
                if steps > len {
                    return Err(NodeGraphError::Cycle { node: start });
                }
                current = parent;
            }
        }

        // 6. Numeric sanity per node.
        for (i, node) in self.nodes.iter().enumerate() {
            match &node.transform {
                NodeTransform::Trs {
                    translation,
                    rotation,
                    scale,
                } => {
                    let finite = translation.iter().all(|v| v.is_finite())
                        && rotation.iter().all(|v| v.is_finite())
                        && scale.iter().all(|v| v.is_finite());
                    if !finite {
                        return Err(NodeGraphError::NonFiniteTransform { node: i });
                    }
                    let len_sq: f32 = rotation.iter().map(|v| v * v).sum();
                    if len_sq.sqrt() <= f32::EPSILON {
                        return Err(NodeGraphError::ZeroRotation { node: i });
                    }
                }
                NodeTransform::Matrix(m) => {
                    if !m.elements.iter().all(|v| v.is_finite()) {
                        return Err(NodeGraphError::NonFiniteTransform { node: i });
                    }
                }
            }
        }

        Ok(())
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
        let mut found = None;
        self.visit(|i, _node, gm| {
            if i == index && found.is_none() {
                found = Some(gm);
            }
        });
        found
    }

    /// The global (world-space) matrix of **every** node, resolved in
    /// one pass over the hierarchy.
    ///
    /// `result[i]` is `Some(global)` when node `i` is reachable from a
    /// root, `None` for orphans. Prefer this over per-node
    /// [`NodeGraph::global_matrix`] calls when resolving more than one
    /// node — the whole graph is walked exactly once.
    pub fn global_matrices(&self) -> Vec<Option<Mat4>> {
        let mut out = vec![None; self.nodes.len()];
        self.visit(|i, _node, gm| {
            if out[i].is_none() {
                out[i] = Some(gm);
            }
        });
        out
    }

    /// Visit every node reachable from the roots in depth-first paint
    /// order, invoking `visit(index, &node, global_matrix)`.
    ///
    /// The global matrix is accumulated on the way down, so each node is
    /// visited exactly once with its correct world transform — far
    /// cheaper than calling [`Self::global_matrix`] per node (which
    /// re-walks from the roots each time). Malformed inputs (cycles,
    /// diamond shares) are tolerated: an already-visited node is never
    /// entered twice, so traversal always terminates.
    pub fn visit<F: FnMut(usize, &SceneNode, Mat4)>(&self, mut visit: F) {
        let mut visited = vec![false; self.nodes.len()];
        for &root in &self.roots {
            self.visit_from(root, Mat4::IDENTITY, &mut visited, &mut visit);
        }
    }

    /// Visit the subtree hanging off `start` (inclusive) in depth-first
    /// order, invoking `visit(index, &node, matrix)`.
    ///
    /// Matrices are **relative to `start`'s parent space**: the
    /// accumulator begins at identity, so `start` itself is visited
    /// with its local matrix. To get world-space matrices for the
    /// subtree, pre-multiply each result by
    /// `global_matrix(parent_of(start))`. No-op when `start` is out of
    /// range. Cycle-safe like [`NodeGraph::visit`].
    pub fn visit_subtree<F: FnMut(usize, &SceneNode, Mat4)>(&self, start: usize, mut visit: F) {
        let mut visited = vec![false; self.nodes.len()];
        self.visit_from(start, Mat4::IDENTITY, &mut visited, &mut visit);
    }

    fn visit_from<F: FnMut(usize, &SceneNode, Mat4)>(
        &self,
        current: usize,
        acc_parent: Mat4,
        visited: &mut [bool],
        visit: &mut F,
    ) {
        let Some(node) = self.nodes.get(current) else {
            return;
        };
        // Guard against malformed cycles / multi-parent shares: enter
        // each node at most once so traversal terminates.
        if visited[current] {
            return;
        }
        visited[current] = true;
        let global = acc_parent.mul(&node.local_matrix());
        visit(current, node, global);
        for &child in &node.children {
            self.visit_from(child, global, visited, visit);
        }
    }

    /// The indices of the subtree rooted at `start`, inclusive, in
    /// depth-first order (`start` first). Empty when `start` is out of
    /// range.
    pub fn descendants(&self, start: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.visit_subtree(start, |i, _, _| out.push(i));
        out
    }

    /// The ancestor chain of `index`, nearest parent first, ending at
    /// the chain's topmost node. Empty for roots / orphans / an
    /// out-of-range index. Terminates on malformed cyclic input.
    pub fn ancestors(&self, index: usize) -> Vec<usize> {
        let parents = self.parent_indices();
        let mut out = Vec::new();
        if index >= self.nodes.len() {
            return out;
        }
        let mut current = index;
        while let Some(parent) = parents[current] {
            if out.len() > self.nodes.len() {
                break; // cycle guard
            }
            out.push(parent);
            current = parent;
        }
        out
    }

    /// The path from the containing root down to `index`, inclusive
    /// (`path[0]` is the root, `path.last()` is `index`). `None` when
    /// `index` is out of range or not reachable from any root.
    pub fn path_from_root(&self, index: usize) -> Option<Vec<usize>> {
        if index >= self.nodes.len() {
            return None;
        }
        let mut path: Vec<usize> = self.ancestors(index);
        path.reverse();
        path.push(index);
        // Reachability: the chain's top must be a listed root.
        if self.roots.contains(&path[0]) {
            Some(path)
        } else {
            None
        }
    }

    /// The index of the first node (in storage order) whose name
    /// matches `name` exactly, or `None`.
    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
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
    fn transpose_swaps_rows_and_columns() {
        let m = Mat4::from_translation([1.0, 2.0, 3.0]);
        let t = m.transpose();
        assert_eq!(t.row(3), [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(t.col(3), [0.0, 0.0, 0.0, 1.0]);
        // Involution.
        assert_eq!(t.transpose(), m);
        // Rotation transpose is its inverse.
        let r = Mat4::from_quaternion([0.0, FRAC_PI_4.sin(), 0.0, FRAC_PI_4.cos()]);
        let prod = r.mul(&r.transpose());
        for i in 0..16 {
            assert!(approx(prod.elements[i], Mat4::IDENTITY.elements[i]));
        }
    }

    #[test]
    fn determinant_closed_forms() {
        assert!(approx(Mat4::IDENTITY.determinant(), 1.0));
        // Scale: product of the axes.
        assert!(approx(
            Mat4::from_scale([2.0, 3.0, 4.0]).determinant(),
            24.0
        ));
        // Mirrored basis: negative.
        assert!(approx(
            Mat4::from_scale([-1.0, 1.0, 1.0]).determinant(),
            -1.0
        ));
        // Rotation: 1. Translation: no contribution.
        let r = Mat4::from_quaternion([0.5, 0.5, 0.5, 0.5]);
        assert!(approx(r.determinant(), 1.0));
        assert!(approx(
            Mat4::from_translation([9.0, -3.0, 7.0]).determinant(),
            1.0
        ));
        // TRS: det == sx * sy * sz.
        let trs = NodeTransform::Trs {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, FRAC_PI_4.sin(), FRAC_PI_4.cos()],
            scale: [2.0, 0.5, 3.0],
        };
        assert!(approx(trs.local_matrix().determinant(), 3.0));
    }

    #[test]
    fn inverse_round_trips_trs() {
        let m = NodeTransform::Trs {
            translation: [4.0, -2.0, 9.0],
            rotation: [0.0, FRAC_PI_4.sin(), 0.0, FRAC_PI_4.cos()],
            scale: [2.0, 3.0, 0.5],
        }
        .local_matrix();
        let inv = m.inverse().unwrap();
        let prod = m.mul(&inv);
        for i in 0..16 {
            assert!(
                approx(prod.elements[i], Mat4::IDENTITY.elements[i]),
                "element {i}: {}",
                prod.elements[i]
            );
        }
        // A point maps there and back.
        let p = [1.5, -7.0, 2.25];
        let back = inv.transform_point(m.transform_point(p));
        assert!(approx3(back, p), "{back:?}");
    }

    #[test]
    fn inverse_of_singular_is_none() {
        // Zero scale on one axis collapses a dimension.
        assert!(Mat4::from_scale([1.0, 0.0, 1.0]).inverse().is_none());
        // Non-finite input.
        let mut m = Mat4::IDENTITY;
        m.elements[0] = f32::NAN;
        assert!(m.inverse().is_none());
        // Identity inverts to itself.
        assert_eq!(Mat4::IDENTITY.inverse(), Some(Mat4::IDENTITY));
    }

    #[test]
    fn quat_mul_matches_matrix_composition() {
        let a = quat_from_axis_angle([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2);
        let b = quat_from_axis_angle([1.0, 0.0, 0.0], std::f32::consts::FRAC_PI_3);
        let via_quat = Mat4::from_quaternion(quat_mul(a, b));
        let via_mats = Mat4::from_quaternion(a).mul(&Mat4::from_quaternion(b));
        assert_mat_approx(&via_quat, &via_mats);
        // Identity is the unit of the product.
        let id = [0.0, 0.0, 0.0, 1.0];
        assert_eq!(quat_mul(id, b), b);
        assert_eq!(quat_mul(a, id), a);
    }

    #[test]
    fn quat_conjugate_inverts_rotation() {
        let q = quat_from_axis_angle([0.3, -0.7, 0.65], 1.234);
        let prod = quat_mul(q, quat_conjugate(q));
        assert!(approx(prod[3].abs(), 1.0), "{prod:?}");
        assert!(approx(prod[0], 0.0) && approx(prod[1], 0.0) && approx(prod[2], 0.0));
    }

    #[test]
    fn quat_axis_angle_matches_hand_built() {
        // 90° about +Y — same quaternion the earlier tests build by hand.
        let q = quat_from_axis_angle([0.0, 2.0, 0.0], std::f32::consts::FRAC_PI_2);
        assert!(approx(q[0], 0.0));
        assert!(approx(q[1], FRAC_PI_4.sin()));
        assert!(approx(q[2], 0.0));
        assert!(approx(q[3], FRAC_PI_4.cos()));
        // Degenerate axis: identity.
        assert_eq!(quat_from_axis_angle([0.0; 3], 1.0), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn quat_normalize_and_dot() {
        let q = quat_normalize([0.0, 0.0, 0.0, 2.0]);
        assert_eq!(q, [0.0, 0.0, 0.0, 1.0]);
        // Zero-length falls back to identity.
        assert_eq!(quat_normalize([0.0; 4]), [0.0, 0.0, 0.0, 1.0]);
        let a = quat_from_axis_angle([0.0, 0.0, 1.0], 0.5);
        assert!(approx(quat_dot(a, a), 1.0));
    }

    #[test]
    fn slerp_endpoints_and_midpoint() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let quarter = quat_from_axis_angle([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2);
        // Endpoints returned exactly (up to normalisation).
        let s0 = quat_slerp(id, quarter, 0.0);
        let s1 = quat_slerp(id, quarter, 1.0);
        for i in 0..4 {
            assert!(approx(s0[i], id[i]));
            assert!(approx(s1[i], quarter[i]));
        }
        // Midpoint of identity -> 90° about Z is 45° about Z.
        let mid = quat_slerp(id, quarter, 0.5);
        let expected = quat_from_axis_angle([0.0, 0.0, 1.0], FRAC_PI_4);
        for i in 0..4 {
            assert!(approx(mid[i], expected[i]), "{mid:?} vs {expected:?}");
        }
    }

    #[test]
    fn slerp_takes_the_short_path() {
        // b and -b encode the same rotation; slerp(a, -b, t) must
        // produce the same *rotation* as slerp(a, b, t).
        let a = quat_from_axis_angle([0.0, 1.0, 0.0], 0.3);
        let b = quat_from_axis_angle([0.0, 1.0, 0.0], 1.1);
        let neg_b = [-b[0], -b[1], -b[2], -b[3]];
        let s = quat_slerp(a, b, 0.25);
        let s_neg = quat_slerp(a, neg_b, 0.25);
        assert_mat_approx(&Mat4::from_quaternion(s), &Mat4::from_quaternion(s_neg));
    }

    #[test]
    fn slerp_near_parallel_falls_back_to_lerp() {
        let a = quat_from_axis_angle([1.0, 0.0, 0.0], 0.0);
        let b = quat_from_axis_angle([1.0, 0.0, 0.0], 1e-6);
        let mid = quat_slerp(a, b, 0.5);
        // Still unit length, still (approximately) the identity.
        assert!(approx(quat_dot(mid, mid), 1.0));
        assert!(approx(mid[3], 1.0));
    }

    /// Assert two matrices agree element-wise within tolerance.
    fn assert_mat_approx(a: &Mat4, b: &Mat4) {
        for i in 0..16 {
            assert!(
                (a.elements[i] - b.elements[i]).abs() < 1e-4,
                "element {i}: {} vs {}",
                a.elements[i],
                b.elements[i]
            );
        }
    }

    #[test]
    fn decompose_round_trips_trs_matrices() {
        // A spread of rotations (per-axis + arbitrary), non-uniform
        // scales, translations. q and -q are the same rotation, so
        // compare via recomposition.
        let half = FRAC_PI_4; // 90° rotations
        let third = std::f32::consts::FRAC_PI_6; // 60° rotations
        let cases: Vec<([f32; 3], [f32; 4], [f32; 3])> = vec![
            ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
            (
                [1.0, 2.0, 3.0],
                [half.sin(), 0.0, 0.0, half.cos()],
                [2.0; 3],
            ),
            (
                [-5.0, 0.5, 9.0],
                [0.0, third.sin(), 0.0, third.cos()],
                [2.0, 3.0, 0.25],
            ),
            (
                [0.0, -1.0, 4.0],
                [0.0, 0.0, third.sin(), third.cos()],
                [1.0, 5.0, 1.0],
            ),
            // Arbitrary-axis rotation (normalised inside from_quaternion).
            ([7.0, 7.0, 7.0], [0.5, 0.5, 0.5, 0.5], [0.5, 2.5, 4.0]),
        ];
        for (t, q, s) in cases {
            let m = NodeTransform::Trs {
                translation: t,
                rotation: q,
                scale: s,
            }
            .local_matrix();
            let (dt, dq, ds) = m.decompose_trs().unwrap();
            assert!(approx3(dt, t), "{dt:?} vs {t:?}");
            assert!(approx3(ds, s), "{ds:?} vs {s:?}");
            let recomposed = NodeTransform::Trs {
                translation: dt,
                rotation: dq,
                scale: ds,
            }
            .local_matrix();
            assert_mat_approx(&recomposed, &m);
            // Returned quaternion is unit-length.
            let qlen: f32 = dq.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!(approx(qlen, 1.0));
        }
    }

    #[test]
    fn decompose_handles_mirrored_basis() {
        // Negative-determinant matrix: reflection folded into scale.x.
        let m = Mat4::from_scale([-2.0, 3.0, 4.0]);
        let (t, q, s) = m.decompose_trs().unwrap();
        assert!(approx3(t, [0.0; 3]));
        assert!(s[0] < 0.0, "{s:?}");
        let recomposed = NodeTransform::Trs {
            translation: t,
            rotation: q,
            scale: s,
        }
        .local_matrix();
        assert_mat_approx(&recomposed, &m);
    }

    #[test]
    fn decompose_rejects_out_of_contract_matrices() {
        // Shear: column 1 leans into column 0.
        let mut sheared = Mat4::IDENTITY;
        sheared.elements[4] = 0.5; // m[0][1]
        assert!(sheared.decompose_trs().is_none());

        // Collapsed axis (zero scale).
        assert!(Mat4::from_scale([1.0, 0.0, 1.0]).decompose_trs().is_none());

        // Non-affine bottom row (projection-like).
        let mut proj = Mat4::IDENTITY;
        proj.elements[11] = -1.0; // m[3][2]
        assert!(proj.decompose_trs().is_none());

        // Non-finite.
        let mut nan = Mat4::IDENTITY;
        nan.elements[5] = f32::NAN;
        assert!(nan.decompose_trs().is_none());
    }

    #[test]
    fn to_trs_converts_matrix_form() {
        // TRS form: returned verbatim.
        let trs = NodeTransform::from_translation([1.0, 2.0, 3.0]);
        assert_eq!(trs.to_trs(), Some(trs));

        // Matrix form: decomposed to an equivalent TRS.
        let baked = NodeTransform::Trs {
            translation: [3.0, -1.0, 2.0],
            rotation: [0.0, FRAC_PI_4.sin(), 0.0, FRAC_PI_4.cos()],
            scale: [2.0, 2.0, 2.0],
        };
        let m = NodeTransform::Matrix(baked.local_matrix());
        let converted = m.to_trs().unwrap();
        assert!(converted.is_trs());
        assert_mat_approx(&converted.local_matrix(), &baked.local_matrix());

        // Sheared matrix: not convertible.
        let mut sheared = Mat4::IDENTITY;
        sheared.elements[4] = 0.5;
        assert_eq!(NodeTransform::Matrix(sheared).to_trs(), None);
    }

    /// A 4-node fixture: root -> (a -> leaf, b), plus an orphan.
    /// Root translates +X 10, a translates +Y 5, leaf translates +Z 2.
    fn fixture() -> (NodeGraph, usize, usize, usize, usize, usize) {
        let mut g = NodeGraph::new();
        let leaf = g.push(SceneNode {
            name: "leaf".into(),
            transform: NodeTransform::from_translation([0.0, 0.0, 2.0]),
            children: vec![],
        });
        let a = g.push(SceneNode {
            name: "a".into(),
            transform: NodeTransform::from_translation([0.0, 5.0, 0.0]),
            children: vec![leaf],
        });
        let b = g.push(SceneNode {
            name: "b".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![],
        });
        let root = g.push_root(SceneNode {
            name: "root".into(),
            transform: NodeTransform::from_translation([10.0, 0.0, 0.0]),
            children: vec![a, b],
        });
        let orphan = g.push(SceneNode::named("orphan"));
        (g, root, a, b, leaf, orphan)
    }

    #[test]
    fn two_node_cycle_terminates() {
        // a -> b and b -> a: malformed, but traversal must not hang or
        // overflow the stack (regression: the old guard only caught
        // self-loops).
        let mut g = NodeGraph::new();
        let a = g.push(SceneNode::named("a"));
        let b = g.push(SceneNode::named("b"));
        g.nodes[a].children.push(b);
        g.nodes[b].children.push(a);
        g.roots.push(a);
        let mut count = 0;
        g.visit(|_, _, _| count += 1);
        assert_eq!(count, 2); // each node entered exactly once
        assert!(g.global_matrix(b).is_some());
        assert_eq!(g.descendants(a), vec![a, b]);
        // ancestors() also terminates on the cyclic parent chain.
        let anc = g.ancestors(a);
        assert!(anc.len() <= g.len() + 1);
    }

    #[test]
    fn global_matrices_resolves_whole_graph_in_one_pass() {
        let (g, root, a, b, leaf, orphan) = fixture();
        let gms = g.global_matrices();
        assert_eq!(gms.len(), g.len());
        // Reachable nodes match per-node global_matrix.
        for idx in [root, a, b, leaf] {
            assert_eq!(gms[idx], g.global_matrix(idx), "node {idx}");
        }
        // Orphan has no world placement.
        assert_eq!(gms[orphan], None);
        // Closed form: leaf origin = (10, 5, 2).
        let p = gms[leaf].unwrap().transform_point([0.0, 0.0, 0.0]);
        assert!(approx3(p, [10.0, 5.0, 2.0]), "{p:?}");
    }

    #[test]
    fn visit_subtree_is_relative_to_parent_space() {
        let (g, _root, a, _b, leaf, _orphan) = fixture();
        // Subtree at `a`: matrices are relative to a's parent space, so
        // a's own matrix is its local (+Y 5) and leaf's is (+Y 5, +Z 2).
        let mut seen = Vec::new();
        g.visit_subtree(a, |i, _, m| seen.push((i, m.transform_point([0.0; 3]))));
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, a);
        assert!(approx3(seen[0].1, [0.0, 5.0, 0.0]));
        assert_eq!(seen[1].0, leaf);
        assert!(approx3(seen[1].1, [0.0, 5.0, 2.0]));
        // Out-of-range start is a no-op.
        let mut n = 0;
        g.visit_subtree(99, |_, _, _| n += 1);
        assert_eq!(n, 0);
    }

    #[test]
    fn descendants_and_ancestors() {
        let (g, root, a, b, leaf, orphan) = fixture();
        assert_eq!(g.descendants(root), vec![root, a, leaf, b]);
        assert_eq!(g.descendants(leaf), vec![leaf]);
        assert_eq!(g.descendants(99), Vec::<usize>::new());

        assert_eq!(g.ancestors(leaf), vec![a, root]);
        assert_eq!(g.ancestors(a), vec![root]);
        assert_eq!(g.ancestors(root), Vec::<usize>::new());
        assert_eq!(g.ancestors(orphan), Vec::<usize>::new());
        assert_eq!(g.ancestors(99), Vec::<usize>::new());
    }

    #[test]
    fn path_from_root_walks_top_down() {
        let (g, root, a, _b, leaf, orphan) = fixture();
        assert_eq!(g.path_from_root(leaf), Some(vec![root, a, leaf]));
        assert_eq!(g.path_from_root(root), Some(vec![root]));
        // Orphans and out-of-range have no root path.
        assert_eq!(g.path_from_root(orphan), None);
        assert_eq!(g.path_from_root(99), None);
        // A parented chain whose top is not a listed root is unreachable.
        let mut g2 = NodeGraph::new();
        let c = g2.push(SceneNode::named("c"));
        let _p = g2.push(SceneNode {
            name: "p".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![c],
        });
        // p is never pushed as root.
        assert_eq!(g2.path_from_root(c), None);
    }

    #[test]
    fn find_by_name_first_match() {
        let (g, root, _a, _b, leaf, _orphan) = fixture();
        assert_eq!(g.find_by_name("root"), Some(root));
        assert_eq!(g.find_by_name("leaf"), Some(leaf));
        assert_eq!(g.find_by_name("nope"), None);
    }

    #[test]
    fn validate_accepts_well_formed_graph() {
        let mut g = NodeGraph::new();
        let c = g.push(SceneNode::named("c"));
        let r = g.push_root(SceneNode {
            name: "r".into(),
            transform: NodeTransform::from_translation([1.0, 2.0, 3.0]),
            children: vec![c],
        });
        // An orphan is legal (no world placement, but not a defect).
        let _orphan = g.push(SceneNode::named("orphan"));
        assert_eq!(g.validate(), Ok(()));
        assert_eq!(g.parent_index(c), Some(r));
        assert_eq!(g.parent_index(r), None);
        // Empty graph is trivially valid.
        assert_eq!(NodeGraph::new().validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_bad_roots() {
        let mut g = NodeGraph::new();
        g.roots.push(3);
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::RootOutOfRange { root: 3, len: 0 })
        );

        let mut g = NodeGraph::new();
        let r = g.push_root(SceneNode::named("r"));
        g.roots.push(r);
        assert_eq!(g.validate(), Err(NodeGraphError::DuplicateRoot { root: r }));
    }

    #[test]
    fn validate_rejects_child_out_of_range() {
        let mut g = NodeGraph::new();
        let r = g.push_root(SceneNode::named("r"));
        g.nodes[r].children.push(7);
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::ChildOutOfRange {
                node: r,
                child: 7,
                len: 1
            })
        );
    }

    #[test]
    fn validate_rejects_multiple_parents() {
        let mut g = NodeGraph::new();
        let c = g.push(SceneNode::named("c"));
        let p1 = g.push_root(SceneNode {
            name: "p1".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![c],
        });
        let p2 = g.push_root(SceneNode {
            name: "p2".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![c],
        });
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::MultipleParents {
                child: c,
                first_parent: p1,
                second_parent: p2
            })
        );

        // Same parent listing a child twice is the same defect.
        let mut g = NodeGraph::new();
        let c = g.push(SceneNode::named("c"));
        let p = g.push_root(SceneNode {
            name: "p".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![c, c],
        });
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::MultipleParents {
                child: c,
                first_parent: p,
                second_parent: p
            })
        );
    }

    #[test]
    fn validate_rejects_root_with_parent() {
        let mut g = NodeGraph::new();
        let a = g.push_root(SceneNode::named("a"));
        let b = g.push_root(SceneNode::named("b"));
        g.nodes[a].children.push(b);
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::RootHasParent { root: b, parent: a })
        );
    }

    #[test]
    fn validate_rejects_cycles() {
        // Self-loop.
        let mut g = NodeGraph::new();
        let a = g.push(SceneNode::named("a"));
        g.nodes[a].children.push(a);
        assert_eq!(g.validate(), Err(NodeGraphError::Cycle { node: a }));

        // Two-node loop: a -> b -> a.
        let mut g = NodeGraph::new();
        let a = g.push(SceneNode::named("a"));
        let b = g.push(SceneNode::named("b"));
        g.nodes[a].children.push(b);
        g.nodes[b].children.push(a);
        let err = g.validate().unwrap_err();
        assert!(matches!(err, NodeGraphError::Cycle { .. }), "{err:?}");
    }

    #[test]
    fn validate_rejects_non_finite_and_zero_rotation() {
        let mut g = NodeGraph::new();
        let a = g.push_root(SceneNode {
            name: "nan".into(),
            transform: NodeTransform::from_translation([f32::NAN, 0.0, 0.0]),
            children: vec![],
        });
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::NonFiniteTransform { node: a })
        );

        let mut g = NodeGraph::new();
        let mut m = Mat4::IDENTITY;
        m.elements[5] = f32::INFINITY;
        let a = g.push_root(SceneNode {
            name: "inf".into(),
            transform: NodeTransform::Matrix(m),
            children: vec![],
        });
        assert_eq!(
            g.validate(),
            Err(NodeGraphError::NonFiniteTransform { node: a })
        );

        let mut g = NodeGraph::new();
        let a = g.push_root(SceneNode {
            name: "zeroq".into(),
            transform: NodeTransform::from_rotation([0.0, 0.0, 0.0, 0.0]),
            children: vec![],
        });
        assert_eq!(g.validate(), Err(NodeGraphError::ZeroRotation { node: a }));
    }

    #[test]
    fn parent_indices_maps_whole_graph() {
        let mut g = NodeGraph::new();
        let c1 = g.push(SceneNode::named("c1"));
        let c2 = g.push(SceneNode::named("c2"));
        let r = g.push_root(SceneNode {
            name: "r".into(),
            transform: NodeTransform::IDENTITY,
            children: vec![c1, c2],
        });
        let orphan = g.push(SceneNode::named("orphan"));
        let parents = g.parent_indices();
        assert_eq!(parents[c1], Some(r));
        assert_eq!(parents[c2], Some(r));
        assert_eq!(parents[r], None);
        assert_eq!(parents[orphan], None);
        // Out-of-range lookup is None, not a panic.
        assert_eq!(g.parent_index(99), None);
    }

    #[test]
    fn node_graph_error_display_is_informative() {
        let e = NodeGraphError::MultipleParents {
            child: 2,
            first_parent: 0,
            second_parent: 1,
        };
        let s = e.to_string();
        assert!(s.contains('2') && s.contains('0') && s.contains('1'), "{s}");
        // It is a std error.
        let _: &dyn std::error::Error = &e;
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
