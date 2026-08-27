//! Maps the four FxSound-style sliders (each 0.0..=1.0) onto the underlying
//! filter-chain plugin controls declared in
//! ~/.config/pipewire/filter-chain.conf.d/99-fxlinux.conf.
//!
//! Ranges below are centered on the values confirmed by ear during M1 tuning
//! (bass:amount=2.0, exciter:amount=1.5, stereo:stereo_base=0.15,
//! reverb:amount=0.08), so a slider at 50% reproduces the already-approved
//! sound and 0%/100% back off to the plugin's neutral default / roughly 2x
//! the tuned amount.
//!
//! Output's range was pulled back from an earlier 1.0..1.6 (default 1.3):
//! that pushed the limiter hard enough on transients (with only a 5ms
//! attack) to produce audible crackle on loud passages. 1.0..1.3 keeps the
//! same "more loudness" direction but caps how hard the limiter has to
//! work; see also the limiter's own `lk` (lookahead, raised to 10ms) and
//! `th` (threshold, -1dB true-peak headroom) in the filter-chain config.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlId {
    Bass,
    Fidelity,
    Ambience,
    Output,
}

/// Linear interpolation from `slider` (0.0..=1.0) to [lo, hi].
fn lerp(slider: f64, lo: f32, hi: f32) -> f32 {
    let slider = slider.clamp(0.0, 1.0) as f32;
    lo + (hi - lo) * slider
}

/// Returns the `("<node>:<control>", value)` pairs to push for a slider move.
pub fn updates_for(id: ControlId, slider: f64) -> Vec<(&'static str, f32)> {
    match id {
        ControlId::Bass => vec![("bass:amount", lerp(slider, 0.0, 4.0))],
        ControlId::Fidelity => vec![("exciter:amount", lerp(slider, 0.0, 3.0))],
        ControlId::Ambience => vec![
            ("stereo:stereo_base", lerp(slider, 0.0, 0.30)),
            ("reverb:amount", lerp(slider, 0.0, 0.16)),
        ],
        ControlId::Output => vec![("limiter:g_in", lerp(slider, 1.0, 1.3))],
    }
}
