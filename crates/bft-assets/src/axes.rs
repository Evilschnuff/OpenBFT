//! The one place the GameZ → Bevy coordinate change lives.
//!
//! GameZ is y-up with an x/z ground plane and 1 unit = 1 metre
//! (docs/data/coordinates.md), which is Bevy's convention already, so
//! [`AxisConvention::GAMEZ_DEFAULT`] is the identity. It is still written as a
//! general permutation-with-signs because docs/data/coordinates.md reached that
//! conclusion by inference and the viewer's `--axes` override exists so a wrong
//! call is one flag away from being disproved.
//!
//! # Why identity is right
//!
//! docs/data/coordinates.md first concluded "left-handed, so importing requires
//! negating one axis and reversing winding". Measuring the winding directly,
//! that half of the conclusion does not hold: the polygon winding in the
//! shipped vertex data is already counter-clockwise seen from outside under a
//! right-handed cross product (371 of 373 closed models by signed volume; 2108
//! of 2120 polygons with stored normals agree). Negating an axis would turn
//! every closed model inside out. See docs/data/models.md.
//!
//! [`AxisConvention::flips_winding`] is what keeps the general case honest:
//! a convention whose matrix has a negative determinant mirrors the model,
//! so [`crate::geometry::triangulate`] reverses each triangle to compensate.

use bevy::prelude::*;

/// Which source axis a destination axis reads from, and with what sign.
///
/// `Axis::NegZ` in the `z` slot means "Bevy's z is the source's z, negated".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
    NegX,
    NegY,
    NegZ,
}

impl Axis {
    /// Pull this axis's value out of a source vector.
    fn select(self, v: Vec3) -> f32 {
        match self {
            Axis::X => v.x,
            Axis::Y => v.y,
            Axis::Z => v.z,
            Axis::NegX => -v.x,
            Axis::NegY => -v.y,
            Axis::NegZ => -v.z,
        }
    }

    /// Which source component (0, 1, 2) this axis reads.
    fn source(self) -> usize {
        match self {
            Axis::X | Axis::NegX => 0,
            Axis::Y | Axis::NegY => 1,
            Axis::Z | Axis::NegZ => 2,
        }
    }

    /// `-1.0` for the negated variants, `1.0` otherwise.
    fn sign(self) -> f32 {
        match self {
            Axis::X | Axis::Y | Axis::Z => 1.0,
            Axis::NegX | Axis::NegY | Axis::NegZ => -1.0,
        }
    }

    /// Parse one axis of a `--axes` string: `x`, `-x`, `+x`, …
    pub fn parse(text: &str) -> Option<Axis> {
        match text.trim() {
            "x" | "+x" | "X" | "+X" => Some(Axis::X),
            "y" | "+y" | "Y" | "+Y" => Some(Axis::Y),
            "z" | "+z" | "Z" | "+Z" => Some(Axis::Z),
            "-x" | "-X" => Some(Axis::NegX),
            "-y" | "-Y" => Some(Axis::NegY),
            "-z" | "-Z" => Some(Axis::NegZ),
            _ => None,
        }
    }
}

/// The order Euler angles are applied in.
///
/// `Xyz` means "rotate about x, then y, then z", matching mech3ax's field
/// order. docs/data/coordinates.md confirms the angles are radians but leaves
/// the order inferred; only two nodes per level use the Euler transform form,
/// so it barely affects level import — the anim VM's `ObjectRotateState` is
/// where it matters (docs/data/anims.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum EulerOrder {
    #[default]
    Xyz,
    Zyx,
}

/// How source coordinates map onto Bevy's.
///
/// One value, used by both the mesh side and the sim side, so geometry and
/// colliders can never disagree about which way is up.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AxisConvention {
    /// Where Bevy's x comes from.
    pub x: Axis,
    /// Where Bevy's y comes from.
    pub y: Axis,
    /// Where Bevy's z comes from.
    pub z: Axis,
    pub euler: EulerOrder,
}

impl Default for AxisConvention {
    fn default() -> AxisConvention {
        AxisConvention::GAMEZ_DEFAULT
    }
}

impl AxisConvention {
    /// The shipped convention: identity, because GameZ is already y-up,
    /// x/z-ground and right-hand-consistent in its winding (see the module
    /// docs).
    pub const GAMEZ_DEFAULT: AxisConvention = AxisConvention {
        x: Axis::X,
        y: Axis::Y,
        z: Axis::Z,
        euler: EulerOrder::Xyz,
    };

    /// Convert a position.
    pub fn point(&self, v: Vec3) -> Vec3 {
        Vec3::new(self.x.select(v), self.y.select(v), self.z.select(v))
    }

    /// Convert a direction.
    ///
    /// The same as [`point`](Self::point) for a pure permutation-with-signs,
    /// and named separately so call sites read correctly — and so a future
    /// convention with a translation does not silently shift normals.
    pub fn direction(&self, v: Vec3) -> Vec3 {
        self.point(v)
    }

    /// Convert a Euler rotation (radians) into a Bevy quaternion.
    ///
    /// The axes are permuted first, then composed in the convention's order,
    /// so a rotation about the source's y stays a rotation about whichever
    /// Bevy axis y maps to.
    pub fn rotation(&self, euler: Vec3) -> Quat {
        let e = self.point(euler);

        match self.euler {
            // Bevy composes `from_euler(order, a, b, c)` in the order the
            // variant names, so `YXZ` with (y, x, z) is x-then-y-then-z
            // applied to a vector.
            EulerOrder::Xyz => Quat::from_euler(EulerRot::ZYX, e.z, e.y, e.x),
            EulerOrder::Zyx => Quat::from_euler(EulerRot::XYZ, e.x, e.y, e.z),
        }
    }

    /// Whether this convention mirrors, so triangles must be reversed to
    /// keep front faces counter-clockwise.
    ///
    /// True exactly when the 3×3 permutation-with-signs matrix has a
    /// negative determinant. `GAMEZ_DEFAULT` is the identity, so false.
    pub fn flips_winding(&self) -> bool {
        self.determinant() < 0.0
    }

    /// The determinant of the mapping, `±1` for any valid permutation and
    /// `0` for a malformed one (two axes reading the same source).
    fn determinant(&self) -> f32 {
        let (a, b, c) = (self.x.source(), self.y.source(), self.z.source());

        if a == b || b == c || a == c {
            return 0.0;
        }

        // Sign of the permutation: even permutations of (0,1,2) are +1.
        let permutation =
            if (a, b, c) == (0, 1, 2) || (a, b, c) == (1, 2, 0) || (a, b, c) == (2, 0, 1) {
                1.0
            } else {
                -1.0
            };

        permutation * self.x.sign() * self.y.sign() * self.z.sign()
    }

    /// Whether every Bevy axis reads a different source axis.
    pub fn is_valid(&self) -> bool {
        self.determinant() != 0.0
    }

    /// Parse a `--axes` override such as `x,y,-z` or `x,z,y`.
    ///
    /// Three comma-separated axes naming where Bevy's x, y and z come from.
    /// Returns `None` if it does not parse or is not a permutation, so the
    /// caller can fall back to [`GAMEZ_DEFAULT`](Self::GAMEZ_DEFAULT) with a
    /// warning rather than importing a level through a degenerate mapping.
    pub fn parse(text: &str) -> Option<AxisConvention> {
        let mut parts = text.split(',');
        let x = Axis::parse(parts.next()?)?;
        let y = Axis::parse(parts.next()?)?;
        let z = Axis::parse(parts.next()?)?;

        if parts.next().is_some() {
            return None;
        }

        let convention = AxisConvention {
            x,
            y,
            z,
            euler: EulerOrder::Xyz,
        };

        convention.is_valid().then_some(convention)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_convention_is_the_identity_and_does_not_mirror() {
        let axes = AxisConvention::GAMEZ_DEFAULT;
        let v = Vec3::new(1.0, 2.0, 3.0);

        assert_eq!(axes.point(v), v);
        assert_eq!(axes.direction(v), v);
        assert!(!axes.flips_winding());
        assert!(axes.is_valid());
    }

    #[test]
    fn negating_one_axis_mirrors_and_so_flips_the_winding() {
        let axes = AxisConvention {
            z: Axis::NegZ,
            ..AxisConvention::GAMEZ_DEFAULT
        };

        assert_eq!(
            axes.point(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(1.0, 2.0, -3.0)
        );
        assert!(axes.flips_winding());
    }

    #[test]
    fn swapping_two_axes_also_mirrors_but_swapping_and_negating_does_not() {
        let swapped = AxisConvention {
            x: Axis::X,
            y: Axis::Z,
            z: Axis::Y,
            euler: EulerOrder::Xyz,
        };
        assert!(swapped.flips_winding(), "an odd permutation mirrors");

        let swapped_and_negated = AxisConvention {
            z: Axis::NegY,
            ..swapped
        };
        assert!(
            !swapped_and_negated.flips_winding(),
            "two mirrorings cancel"
        );
    }

    #[test]
    fn a_rotation_about_the_source_y_stays_about_the_axis_y_maps_to() {
        // Source y-up, mapped so that Bevy's z reads the source's y.
        let axes = AxisConvention {
            x: Axis::X,
            y: Axis::Z,
            z: Axis::Y,
            euler: EulerOrder::Xyz,
        };

        let quarter_turn_about_source_y =
            axes.rotation(Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0));
        // Which is a quarter turn about Bevy's z, since y maps there.
        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

        // Compared componentwise rather than with `angle_between`, which goes
        // through `acos` and reports ~2.5e-4 rad for two f32 quaternions that
        // are bit-identical.
        assert!(
            quarter_turn_about_source_y.abs_diff_eq(expected, 1e-6),
            "{quarter_turn_about_source_y:?} vs {expected:?}"
        );
    }

    #[test]
    fn the_identity_convention_rotates_exactly_as_bevy_would() {
        let axes = AxisConvention::GAMEZ_DEFAULT;
        // m1's one non-trivial Euler transform: bft_00 at rotate.y = -70deg.
        let euler = Vec3::new(0.0, -1.221_730_5, 0.0);

        let ours = axes.rotation(euler);
        let bevys = Quat::from_rotation_y(-1.221_730_5);

        assert!(ours.abs_diff_eq(bevys, 1e-6), "{ours:?} vs {bevys:?}");
    }

    #[test]
    fn the_override_parses_permutations_and_rejects_everything_else() {
        assert_eq!(
            AxisConvention::parse("x,y,z"),
            Some(AxisConvention::GAMEZ_DEFAULT)
        );
        assert_eq!(
            AxisConvention::parse("x,y,-z").map(|a| a.flips_winding()),
            Some(true)
        );
        assert_eq!(
            AxisConvention::parse("-z, x , y").map(|a| a.z),
            Some(Axis::Y)
        );

        // Not a permutation: y read twice, x never.
        assert_eq!(AxisConvention::parse("y,y,z"), None);
        assert_eq!(AxisConvention::parse("x,y"), None);
        assert_eq!(AxisConvention::parse("x,y,z,w"), None);
        assert_eq!(AxisConvention::parse("up,down,strange"), None);
    }
}
