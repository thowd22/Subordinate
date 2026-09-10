//! The reference `audio-effect` plugin: gain with a smoothing parameter.
//!
//! This is the worked example for `subordinate:plugin@0.1.0`'s `audio-effect`
//! world (docs/PLAN.md §6.2). It is deliberately the smallest useful effect —
//! a level control — so that everything visible in it is the world's contract
//! rather than the DSP:
//!
//! - `describe()` names the plugin, lists its parameters with their units and
//!   ranges, and claims a share of real time. The host takes the smaller of
//!   that claim and its own policy, times every block, and bypasses the plugin
//!   after repeated overruns, so a plugin that asks for more than it needs only
//!   gets itself bypassed sooner.
//! - `process()` receives one block of interleaved samples plus the channel
//!   count, sample rate and the current parameter values, and returns a block
//!   of exactly the same shape. It reuses the incoming buffer rather than
//!   allocating a second one, and holds its ramp state between calls, because
//!   a smoothed parameter is only smooth if it remembers where it was.
//!
//! Build it with `cargo build --release --target wasm32-wasip2`; the Rust
//! toolchain emits a component directly, so there is no `cargo component` or
//! `wasm-tools` step.

#![allow(missing_docs, clippy::all, clippy::pedantic)]

wit_bindgen::generate!({
    path: "../../wit",
    world: "audio-effect",
});

#[allow(clippy::all, clippy::pedantic)]
pub mod dsp;

use std::cell::RefCell;

// `EffectDescription` and `Param` are already at the crate root: the world
// `use`s them, so the generated bindings hoist them. The two types only the
// description mentions stay in the interface's module.
use crate::subordinate::plugin::audio::{ParamDescriptor, ParamUnit};

pub use dsp::SmoothedGain;

/// The parameter that sets the level, in decibels full scale.
pub const GAIN_DB: &str = "gain-db";
/// The parameter that sets how long the level takes to get there, in seconds.
pub const SMOOTHING: &str = "smoothing";

// The ramp state, held between `process` calls. A component instance is
// single-threaded, so a thread-local cell is the whole of the synchronisation
// story; nothing here locks.
thread_local! {
    static STATE: RefCell<SmoothedGain> = const { RefCell::new(SmoothedGain::new()) };
}

/// The plugin's self-description: the one place its parameters are defined.
pub fn description() -> EffectDescription {
    EffectDescription {
        id: "com.subordinate.gain".to_owned(),
        name: "Gain".to_owned(),
        params: vec![
            ParamDescriptor {
                name: GAIN_DB.to_owned(),
                label: "Gain".to_owned(),
                unit: ParamUnit::Decibel,
                minimum: -60.0,
                maximum: 12.0,
                default: 0.0,
            },
            ParamDescriptor {
                name: SMOOTHING.to_owned(),
                label: "Smoothing".to_owned(),
                unit: ParamUnit::Seconds,
                minimum: 0.0,
                maximum: 1.0,
                default: 0.02,
            },
        ],
        // Any block size: the effect is stateless in the block dimension.
        max_block_frames: 0,
        // A multiply per sample needs a sliver of real time. Claiming it, rather
        // than the host's default half, tells the host to bypass this plugin
        // early if it ever gets slow.
        budget_percent: 10,
    }
}

/// Reads one parameter, falling back to its described default and clamping to
/// its described range: a host is supposed to clamp, and a plugin that trusts
/// it anyway is one bad caller away from `NaN` in the mixer.
pub fn param_value(params: &[Param], name: &str) -> f32 {
    let descriptor = description()
        .params
        .into_iter()
        .find(|descriptor| descriptor.name == name)
        .expect("every parameter read here is one this plugin describes");
    let value = params
        .iter()
        .find(|param| param.name == name)
        .map_or(descriptor.default, |param| param.value);
    if value.is_nan() {
        return descriptor.default;
    }
    value.clamp(descriptor.minimum, descriptor.maximum)
}

/// Applies the smoothed gain to one block, in place, and hands the same buffer
/// back: the world returns a `list<f32>`, and reusing the input is how that
/// costs no second allocation.
pub fn process_block(
    mut block: Vec<f32>,
    channels: u32,
    rate: u32,
    params: &[Param],
    gain: &mut SmoothedGain,
) -> Vec<f32> {
    let target_db = param_value(params, GAIN_DB);
    let smoothing = param_value(params, SMOOTHING);
    gain.process(&mut block, channels as usize, rate, target_db, smoothing);
    block
}

/// The component's exports.
pub struct Gain;

impl Guest for Gain {
    fn describe() -> EffectDescription {
        description()
    }

    fn process(
        block: Vec<f32>,
        channels: u32,
        rate: u32,
        params: Vec<Param>,
    ) -> Result<Vec<f32>, Error> {
        Ok(STATE.with_borrow_mut(|gain| process_block(block, channels, rate, &params, gain)))
    }
}

export!(Gain);

#[cfg(test)]
mod tests {
    use super::{GAIN_DB, Param, SMOOTHING, SmoothedGain, description, param_value, process_block};

    fn param(name: &str, value: f32) -> Param {
        Param {
            name: name.to_owned(),
            value,
        }
    }

    #[test]
    fn the_description_names_both_parameters_with_usable_ranges() {
        let description = description();
        assert_eq!(description.id, "com.subordinate.gain");
        let names: Vec<&str> = description
            .params
            .iter()
            .map(|descriptor| descriptor.name.as_str())
            .collect();
        assert_eq!(names, vec![GAIN_DB, SMOOTHING]);
        for descriptor in &description.params {
            assert!(descriptor.minimum <= descriptor.default);
            assert!(descriptor.default <= descriptor.maximum);
            assert!(!descriptor.label.is_empty());
        }
        // The claim is a share of real time the host can honour.
        assert!((1..=100).contains(&description.budget_percent));
        assert_eq!(description.max_block_frames, 0);
    }

    #[test]
    fn a_missing_out_of_range_or_nan_parameter_falls_back_to_the_description() {
        assert_eq!(param_value(&[], GAIN_DB), 0.0);
        assert_eq!(param_value(&[], SMOOTHING), 0.02);
        assert_eq!(param_value(&[param(GAIN_DB, 99.0)], GAIN_DB), 12.0);
        assert_eq!(param_value(&[param(GAIN_DB, -400.0)], GAIN_DB), -60.0);
        assert_eq!(param_value(&[param(GAIN_DB, f32::NAN)], GAIN_DB), 0.0);
    }

    #[test]
    fn a_block_comes_back_with_exactly_the_shape_it_went_in_with() {
        let mut gain = SmoothedGain::new();
        let block = vec![1.0_f32; 1_024];
        let out = process_block(block, 2, 48_000, &[param(GAIN_DB, -6.0)], &mut gain);
        assert_eq!(out.len(), 1_024);
        assert!(out.iter().all(|sample| sample.is_finite()));
        assert!((out[0] - 0.501_187).abs() < 1e-3, "{}", out[0]);
    }

    #[test]
    fn the_ramp_carries_across_block_boundaries() {
        let mut gain = SmoothedGain::new();
        // Settle at unity, then ask for silence with a long smoothing time.
        let params = [param(GAIN_DB, 0.0), param(SMOOTHING, 0.5)];
        process_block(vec![1.0; 16], 1, 48_000, &params, &mut gain);
        let params = [param(GAIN_DB, -60.0), param(SMOOTHING, 0.5)];

        let first = process_block(vec![1.0; 480], 1, 48_000, &params, &mut gain);
        let second = process_block(vec![1.0; 480], 1, 48_000, &params, &mut gain);
        // The second block picks up where the first left off rather than
        // restarting the ramp: it is strictly quieter throughout.
        assert!(second[0] < first[first.len() - 1]);
        assert!(second[second.len() - 1] < second[0]);
        // And 960 frames of a 500 ms ramp is nowhere near silence yet.
        assert!(second[second.len() - 1] > 0.9);
    }

    #[test]
    fn zero_channels_and_an_empty_block_are_survivable() {
        let mut gain = SmoothedGain::new();
        assert!(process_block(Vec::new(), 2, 48_000, &[], &mut gain).is_empty());
        assert_eq!(
            process_block(vec![1.0, 1.0], 0, 48_000, &[], &mut gain),
            vec![1.0, 1.0]
        );
    }
}
