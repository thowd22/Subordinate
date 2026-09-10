//! The grade itself: the effect's parameters and what they do to a pixel.
//!
//! This file holds no WIT, no wgpu and no bindings, for two reasons. It can be
//! unit-tested on the host triple, the way `plugins/gain`'s DSP is; and it is
//! the *one* place the parameter table lives. `lib.rs` lifts [`PARAMS`] into
//! the `effect-types` records `describe` returns, and the host's golden render
//! test includes this same file with `#[path]` — no dependency edge into this
//! plugin's workspace — so the test binds the parameters the plugin declares
//! and compares the shader's output with [`Grade::apply`] rather than with a
//! second copy of the maths.
//!
//! Everything here works in *linear* light, which is what the shader sees: the
//! compositor samples an sRGB texture, so the value in the fragment shader is
//! already decoded, and the sRGB target encodes again on store.

/// Rec.709 luma weights, the same primaries the MVP composites in
/// (decision-3): the grey a colour desaturates towards.
pub const LUMA_WEIGHTS: [f32; 3] = [0.212_6, 0.715_2, 0.072_2];

/// Id of the tint colour parameter.
pub const TINT: &str = "tint";
/// Id of the tint strength parameter.
pub const TINT_AMOUNT: &str = "tint_amount";
/// Id of the exposure parameter, in stops.
pub const EXPOSURE: &str = "exposure";
/// Id of the saturation parameter, where 1 is unchanged.
pub const SATURATION: &str = "saturation";

/// What kind of value a parameter holds, with its range and default.
///
/// A subset of the WIT `param-kind` variant: this effect declares one colour
/// and three continuous controls, and a table that cannot spell the cases it
/// does not use cannot get them wrong either.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpecKind {
    /// A continuous value with an inclusive range.
    Float {
        /// Smallest accepted value.
        min: f32,
        /// Largest accepted value.
        max: f32,
        /// The value the host binds until the user changes it.
        default: f32,
    },
    /// A linear RGBA colour, each channel in `0.0..=1.0`.
    Color {
        /// The value the host binds until the user changes it.
        default: [f32; 4],
    },
}

/// One declared parameter: what the inspector shows and what the shader reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Stable key, also the name of the uniform member the shader reads.
    pub id: &'static str,
    /// Human-readable label for the inspector.
    pub label: &'static str,
    /// One-line explanation for a tooltip.
    pub doc: &'static str,
    /// The value's type, range and default.
    pub kind: SpecKind,
}

/// The effect's parameters, in the order the inspector shows them and in the
/// order the host lays them out in the uniform block.
///
/// The defaults are the identity grade: adding the effect to a clip changes no
/// pixel until a control is moved, which is what makes it safe for the host to
/// bind the effect the moment it is dropped on a clip.
pub const PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        id: TINT,
        label: "Tint",
        doc: "Colour the picture is multiplied by; white leaves it alone.",
        kind: SpecKind::Color {
            default: [1.0, 1.0, 1.0, 1.0],
        },
    },
    ParamSpec {
        id: TINT_AMOUNT,
        label: "Tint amount",
        doc: "How far towards the tinted picture the result moves.",
        kind: SpecKind::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
        },
    },
    ParamSpec {
        id: EXPOSURE,
        label: "Exposure",
        doc: "Stops of exposure: +1 doubles the light, -1 halves it.",
        kind: SpecKind::Float {
            min: -8.0,
            max: 8.0,
            default: 0.0,
        },
    },
    ParamSpec {
        id: SATURATION,
        label: "Saturation",
        doc: "0 is greyscale, 1 leaves the picture alone, 2 doubles it.",
        kind: SpecKind::Float {
            min: 0.0,
            max: 4.0,
            default: 1.0,
        },
    },
];

/// One set of bound parameter values: what a clip's instance of this effect is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grade {
    /// The colour the picture is multiplied by, RGBA; alpha is ignored.
    pub tint: [f32; 4],
    /// How far towards the tinted picture the result moves, `0.0..=1.0`.
    pub tint_amount: f32,
    /// Stops of exposure.
    pub exposure: f32,
    /// Saturation, where 1 leaves the picture alone.
    pub saturation: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Grade {
    /// The declared defaults: the grade that changes nothing.
    pub const IDENTITY: Self = Self {
        tint: [1.0, 1.0, 1.0, 1.0],
        tint_amount: 0.0,
        exposure: 0.0,
        saturation: 1.0,
    };

    /// The grade `src/effect.wgsl` applies, one pixel at a time, in linear
    /// light — and in the same order, which is the whole point of it living
    /// beside the shader:
    ///
    /// 1. exposure, as a power of two, so a stop is a stop at every level;
    /// 2. the tint, a multiply mixed in by its amount, so a tint darkens the
    ///    channels it is not made of rather than flooding the picture;
    /// 3. saturation, towards Rec.709 luma;
    /// 4. a floor at zero, because a saturation above one can push a channel
    ///    negative and negative light is not a colour.
    ///
    /// Alpha is never touched: an effect that changed coverage would change
    /// what the compositor blends underneath it.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let scale = self.exposure.exp2();
        let exposed = rgb.map(|channel| channel * scale);
        let tinted = [
            mix(exposed[0], exposed[0] * self.tint[0], self.tint_amount),
            mix(exposed[1], exposed[1] * self.tint[1], self.tint_amount),
            mix(exposed[2], exposed[2] * self.tint[2], self.tint_amount),
        ];
        let luma = LUMA_WEIGHTS[0].mul_add(
            tinted[0],
            LUMA_WEIGHTS[1].mul_add(tinted[1], LUMA_WEIGHTS[2] * tinted[2]),
        );
        tinted.map(|channel| mix(luma, channel, self.saturation).max(0.0))
    }
}

/// WGSL's `mix`: `a` at `t == 0`, `b` at `t == 1`.
fn mix(a: f32, b: f32, t: f32) -> f32 {
    t.mul_add(b - a, a)
}

#[cfg(test)]
mod tests {
    use super::{EXPOSURE, Grade, PARAMS, SATURATION, SpecKind, TINT, TINT_AMOUNT};

    /// How far two channels may sit apart and still count as equal.
    const EPSILON: f32 = 1e-5;

    fn close(actual: [f32; 3], expected: [f32; 3]) {
        for (got, want) in actual.iter().zip(expected) {
            assert!((got - want).abs() < EPSILON, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn every_parameter_is_named_once_and_holds_its_default() {
        let ids: Vec<&str> = PARAMS.iter().map(|param| param.id).collect();
        assert_eq!(ids, vec![TINT, TINT_AMOUNT, EXPOSURE, SATURATION]);
        for param in &PARAMS {
            assert!(!param.label.is_empty());
            assert!(!param.doc.is_empty());
            match param.kind {
                SpecKind::Float { min, max, default } => {
                    assert!(min <= default && default <= max, "{}", param.id);
                }
                SpecKind::Color { default } => {
                    assert!(default.iter().all(|channel| (0.0..=1.0).contains(channel)));
                }
            }
        }
    }

    #[test]
    fn the_defaults_are_the_identity_grade() {
        let grade = Grade::IDENTITY;
        close(grade.apply([0.2, 0.5, 0.8]), [0.2, 0.5, 0.8]);
        // And they are the defaults the table declares, not a second opinion.
        for param in &PARAMS {
            let default = match param.kind {
                SpecKind::Float { default, .. } => default,
                SpecKind::Color { default } => default[0],
            };
            let bound = match param.id {
                TINT => grade.tint[0],
                TINT_AMOUNT => grade.tint_amount,
                EXPOSURE => grade.exposure,
                SATURATION => grade.saturation,
                other => panic!("unknown parameter {other}"),
            };
            assert!((default - bound).abs() < EPSILON, "{}", param.id);
        }
    }

    #[test]
    fn a_stop_of_exposure_doubles_the_light() {
        let grade = Grade {
            exposure: 1.0,
            ..Grade::IDENTITY
        };
        close(grade.apply([0.1, 0.2, 0.3]), [0.2, 0.4, 0.6]);
        let darker = Grade {
            exposure: -2.0,
            ..Grade::IDENTITY
        };
        close(darker.apply([0.4, 0.8, 0.0]), [0.1, 0.2, 0.0]);
    }

    #[test]
    fn the_tint_multiplies_by_its_amount_and_nothing_at_zero() {
        let full = Grade {
            tint: [1.0, 0.5, 0.0, 1.0],
            tint_amount: 1.0,
            saturation: 1.0,
            exposure: 0.0,
        };
        close(full.apply([0.4, 0.4, 0.4]), [0.4, 0.2, 0.0]);
        let half = Grade {
            tint_amount: 0.5,
            ..full
        };
        close(half.apply([0.4, 0.4, 0.4]), [0.4, 0.3, 0.2]);
        let none = Grade {
            tint_amount: 0.0,
            ..full
        };
        close(none.apply([0.4, 0.4, 0.4]), [0.4, 0.4, 0.4]);
    }

    #[test]
    fn zero_saturation_is_luma_on_every_channel() {
        let grade = Grade {
            saturation: 0.0,
            ..Grade::IDENTITY
        };
        let grey = grade.apply([0.25, 0.5, 0.75]);
        assert!((grey[0] - grey[1]).abs() < EPSILON);
        assert!((grey[1] - grey[2]).abs() < EPSILON);
        // A colour already grey is left where it is.
        close(grade.apply([0.5, 0.5, 0.5]), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn saturation_never_pushes_a_channel_below_zero() {
        let grade = Grade {
            saturation: 4.0,
            ..Grade::IDENTITY
        };
        let out = grade.apply([0.0, 0.5, 0.5]);
        assert!(out.iter().all(|channel| *channel >= 0.0), "{out:?}");
    }
}
