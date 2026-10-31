//! Maps the four FxSound-style sliders (each 0.0..=1.0) onto the underlying
//! filter-chain plugin controls.
//!
//! The limits are based on the installed LV2 metadata. Calf documents
//! `amount` as 0..64 for Bass Enhancer and Exciter, Reverb wet amount as
//! 0..2, and Stereo Tools stereo base as -1..1. Its factory Reverb presets
//! use wet amounts from 0.184284 through 0.469761. Output remains on the
//! established limiter input gain range.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlId {
    Bass,
    Fidelity,
    Ambience,
    Output,
}

const BASS_MAX: f32 = 18.0;
const FIDELITY_MAX: f32 = 12.0;
const AMBIENCE_WIDTH_MAX: f32 = 0.60;
// Close to Calf's strongest factory Reverb preset (0.469761), and well below
// the plugin's documented maximum of 2.0.
const AMBIENCE_WET_MAX: f32 = 0.48;

fn normalized(slider: f64) -> f32 {
    slider.clamp(0.0, 1.0) as f32
}

/// A convex curve gives useful resolution near neutral while still reaching
/// a clearly different upper range without approaching Calf's 64x endpoint.
fn amount_curve(slider: f64, maximum: f32) -> f32 {
    maximum * normalized(slider).powf(1.6)
}

fn lerp(slider: f64, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * normalized(slider)
}

/// Returns the `("<node>:<control>", value)` pairs to push for a slider move.
pub fn updates_for(id: ControlId, slider: f64) -> Vec<(&'static str, f32)> {
    match id {
        ControlId::Bass => vec![("bass:amount", amount_curve(slider, BASS_MAX))],
        ControlId::Fidelity => vec![("exciter:amount", amount_curve(slider, FIDELITY_MAX))],
        ControlId::Ambience => {
            let slider = normalized(slider);
            vec![
                ("stereo:stereo_base", AMBIENCE_WIDTH_MAX * slider),
                // A gentler curve than the enhancers reaches the range used by
                // Calf's factory presets through the upper half of the slider.
                ("reverb:amount", AMBIENCE_WET_MAX * slider.powf(1.5)),
            ]
        }
        ControlId::Output => vec![("limiter:g_in", lerp(slider, 1.0, 1.3))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::model::factory_presets;

    fn value(id: ControlId, slider: f64, index: usize) -> f32 {
        updates_for(id, slider)[index].1
    }

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }

    #[test]
    fn endpoints_are_neutral_or_expected_minima_and_bounded_maxima() {
        close(value(ControlId::Bass, 0.0, 0), 0.0);
        close(value(ControlId::Bass, 1.0, 0), BASS_MAX);
        close(value(ControlId::Fidelity, 0.0, 0), 0.0);
        close(value(ControlId::Fidelity, 1.0, 0), FIDELITY_MAX);
        close(value(ControlId::Ambience, 0.0, 0), 0.0);
        close(value(ControlId::Ambience, 0.0, 1), 0.0);
        close(value(ControlId::Ambience, 1.0, 0), AMBIENCE_WIDTH_MAX);
        close(value(ControlId::Ambience, 1.0, 1), AMBIENCE_WET_MAX);
    }

    #[test]
    fn every_mapping_is_monotonic() {
        for id in [ControlId::Bass, ControlId::Fidelity, ControlId::Ambience] {
            let width = updates_for(id, 0.0).len();
            for index in 0..width {
                let mut previous = value(id, 0.0, index);
                for step in 1..=100 {
                    let current = value(id, f64::from(step) / 100.0, index);
                    assert!(current >= previous, "{id:?} port {index} fell at {step}");
                    previous = current;
                }
            }
        }
    }

    #[test]
    fn mappings_stay_inside_documented_lv2_bounds() {
        assert!((0.0..=64.0).contains(&value(ControlId::Bass, 1.0, 0)));
        assert!((0.0..=64.0).contains(&value(ControlId::Fidelity, 1.0, 0)));
        assert!((-1.0..=1.0).contains(&value(ControlId::Ambience, 1.0, 0)));
        assert!((0.0..=2.0).contains(&value(ControlId::Ambience, 1.0, 1)));

        // Out-of-range GTK or persisted values are clamped before mapping.
        assert_eq!(
            updates_for(ControlId::Bass, -1.0),
            updates_for(ControlId::Bass, 0.0)
        );
        assert_eq!(
            updates_for(ControlId::Bass, 2.0),
            updates_for(ControlId::Bass, 1.0)
        );
    }

    #[test]
    fn representative_factory_positions_land_in_useful_ranges() {
        let presets = factory_presets();
        let music = presets
            .iter()
            .find(|preset| preset.name == "Music")
            .unwrap();
        let movie = presets
            .iter()
            .find(|preset| preset.name == "Movie")
            .unwrap();

        // Music bass at 0.60 is well above Calf's default amount of 1, but
        // remains far below its documented maximum of 64.
        let music_bass = value(ControlId::Bass, music.bass, 0);
        assert!((7.9..8.0).contains(&music_bass));

        // Movie ambience at 0.70 is near Calf's 0.291183 Ambience preset.
        let movie_width = value(ControlId::Ambience, movie.ambience, 0);
        let movie_wet = value(ControlId::Ambience, movie.ambience, 1);
        assert!((0.41..0.43).contains(&movie_width));
        assert!((0.28..0.29).contains(&movie_wet));
    }

    #[test]
    fn output_mapping_remains_unchanged() {
        assert_eq!(
            updates_for(ControlId::Output, 0.0),
            vec![("limiter:g_in", 1.0)]
        );
        assert_eq!(
            updates_for(ControlId::Output, 1.0),
            vec![("limiter:g_in", 1.3)]
        );
    }
}
