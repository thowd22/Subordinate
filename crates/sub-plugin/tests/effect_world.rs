//! The effect worlds describe a real effect.
//!
//! TASK-87 owns the compositor that compiles and runs a declaration; what this
//! proves now is that `wit/subordinate-plugin.wit` gives a plugin everything it
//! needs to declare one — every parameter kind with its range and default, the
//! WGSL and its entry point — and that the optional CPU path's frame carries
//! exact rational time like the rest of the timeline.

use sub_plugin::{
    WitBoolParam, WitColor, WitColorParam, WitEffectDesc, WitEnumParam, WitEnumVariant,
    WitFloatParam, WitFrame, WitIntParam, WitParamBinding, WitParamDesc, WitParamKind,
    WitParamValue, WitPixelFormat, WitRationalTime,
};
use sub_time::{Rational, RationalTime};

/// The rate the fixture frame sits at: not an integer, on purpose.
fn fps() -> Rational {
    Rational::FPS_23_976
}

/// A parameter, spelled the way a plugin would spell it.
fn param(id: &str, label: &str, kind: WitParamKind) -> WitParamDesc {
    WitParamDesc {
        id: id.to_owned(),
        label: label.to_owned(),
        doc: String::new(),
        kind,
    }
}

/// A declaration exercising all five parameter kinds.
fn desc() -> WitEffectDesc {
    WitEffectDesc {
        params: vec![
            param(
                "exposure",
                "Exposure",
                WitParamKind::Float(WitFloatParam {
                    min: -4.0,
                    max: 4.0,
                    default: 0.0,
                    step: Some(0.1),
                }),
            ),
            param(
                "samples",
                "Samples",
                WitParamKind::Int(WitIntParam {
                    min: 1,
                    max: 64,
                    default: 8,
                }),
            ),
            param(
                "preserve_luma",
                "Preserve luma",
                WitParamKind::Boolean(WitBoolParam { default: true }),
            ),
            param(
                "tint",
                "Tint",
                WitParamKind::Color(WitColorParam {
                    default: WitColor {
                        r: 1.0,
                        g: 0.5,
                        b: 0.25,
                        a: 1.0,
                    },
                }),
            ),
            param(
                "channel",
                "Channel",
                WitParamKind::Choice(WitEnumParam {
                    variants: vec![
                        WitEnumVariant {
                            id: "luma".to_owned(),
                            label: "Luma".to_owned(),
                        },
                        WitEnumVariant {
                            id: "chroma".to_owned(),
                            label: "Chroma".to_owned(),
                        },
                    ],
                    default: 0,
                }),
            ),
        ],
        shader: "@fragment fn fs_main() -> @location(0) vec4<f32> { \
                 return vec4<f32>(1.0); }"
            .to_owned(),
        entry: "fs_main".to_owned(),
    }
}

#[test]
fn a_declaration_carries_wgsl_its_entry_point_and_every_parameter_kind() {
    let desc = desc();

    assert!(desc.shader.contains(&desc.entry));
    assert_eq!(desc.entry, "fs_main");

    // Every parameter kind the schema promises is expressible, in the order
    // the inspector will show them.
    let ids: Vec<&str> = desc.params.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        ["exposure", "samples", "preserve_luma", "tint", "channel"]
    );
    assert!(matches!(desc.params[0].kind, WitParamKind::Float(_)));
    assert!(matches!(desc.params[1].kind, WitParamKind::Int(_)));
    assert!(matches!(desc.params[2].kind, WitParamKind::Boolean(_)));
    assert!(matches!(desc.params[3].kind, WitParamKind::Color(_)));
    assert!(matches!(desc.params[4].kind, WitParamKind::Choice(_)));
}

#[test]
fn every_declared_default_sits_inside_its_declared_range() {
    for param in desc().params {
        match param.kind {
            WitParamKind::Float(p) => {
                assert!(p.min <= p.max, "{}", param.id);
                assert!((p.min..=p.max).contains(&p.default), "{}", param.id);
            }
            WitParamKind::Int(p) => {
                assert!(p.min <= p.max, "{}", param.id);
                assert!((p.min..=p.max).contains(&p.default), "{}", param.id);
            }
            WitParamKind::Choice(p) => {
                assert!(!p.variants.is_empty(), "{}", param.id);
                assert!((p.default as usize) < p.variants.len(), "{}", param.id);
            }
            WitParamKind::Boolean(_) | WitParamKind::Color(_) => {}
        }
    }
}

#[test]
fn a_binding_pairs_one_value_with_the_kind_it_was_declared_with() {
    /// Whether a value is of the kind a parameter was declared with.
    fn matches(kind: &WitParamKind, value: &WitParamValue) -> bool {
        matches!(
            (kind, value),
            (WitParamKind::Float(_), WitParamValue::Float(_))
                | (WitParamKind::Int(_), WitParamValue::Int(_))
                | (WitParamKind::Boolean(_), WitParamValue::Boolean(_))
                | (WitParamKind::Color(_), WitParamValue::Color(_))
                | (WitParamKind::Choice(_), WitParamValue::Choice(_))
        )
    }

    let desc = desc();
    let bindings = [
        WitParamValue::Float(1.5),
        WitParamValue::Int(16),
        WitParamValue::Boolean(false),
        WitParamValue::Color(WitColor {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }),
        WitParamValue::Choice(1),
    ];

    // The CPU path gets one binding per declared parameter, in declaration
    // order, and each value is of the declared kind.
    let bindings: Vec<WitParamBinding> = desc
        .params
        .iter()
        .zip(bindings)
        .map(|(param, value)| {
            assert!(matches(&param.kind, &value), "{}", param.id);
            WitParamBinding {
                id: param.id.clone(),
                value,
            }
        })
        .collect();
    assert_eq!(bindings.len(), desc.params.len());

    // A value of the wrong kind is visibly not the declared one.
    assert!(!matches(&desc.params[0].kind, &WitParamValue::Int(1)));
}

#[test]
fn a_cpu_frame_carries_exact_rational_time_and_a_full_plane() {
    let time = RationalTime::from_frames(12, fps());
    let frame = WitFrame {
        width: 4,
        height: 2,
        stride: 4 * 4,
        format: WitPixelFormat::Rgba8Unorm,
        time: WitRationalTime::from(time),
        pixels: vec![0; 4 * 4 * 2],
    };

    assert_eq!(frame.pixels.len(), (frame.stride * frame.height) as usize);
    // 23.976 fps is 24000/1001: the instant crosses as a fraction and comes
    // back unchanged, never as seconds.
    assert_eq!(RationalTime::try_from(frame.time).unwrap(), time);
}
