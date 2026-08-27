//! Preset schema and factory presets.
//!
//! EQ bands correspond to the LSP 8-band parametric EQ's *fixed* default
//! center frequencies (confirmed against the installed plugin's own TTL,
//! not guessed): 16 / 40 / 100 / 250 / 630 / 1600 / 4000 / 10000 Hz — close
//! to a standard octave-ish graphic EQ layout, so no per-band frequency
//! control is needed, only gain.
//!
//! Gain is stored here in dB (human-editable) and converted to the plugin's
//! native linear multiplier (0dB = 1.0) only when pushed to PipeWire.
//!
//! Curves below follow established conventions rather than guesses: a
//! Harman-ish gentle bass/treble tilt for Music, a dialogue-presence boost
//! (1-4kHz) with reduced low-mid boom for Movie, a broadcast-voice curve
//! (rumble cut + 2-5kHz articulation boost) for Voice, and a footstep/cue
//! clarity boost for Gaming.
//!
//! Adaptive bass leveling (gentle downward compression on the multiband
//! compressor's 0-1kHz band, so already-hot bass gets reined in
//! automatically instead of needing the Bass slider retuned per track) is
//! static in the filter-chain config, not part of the preset schema — it
//! applies uniformly under whatever bass boost a preset or slider sets.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// Slider positions (0.0..=1.0), same semantics as audio::controls.
    pub bass: f64,
    pub fidelity: f64,
    pub ambience: f64,
    pub output: f64,
    /// Per-band gain in dB at 16/40/100/250/630/1600/4000/10000 Hz.
    pub eq_db: [f32; 8],
}

pub fn factory_presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "Flat / Reference".into(),
            bass: 0.0,
            fidelity: 0.0,
            ambience: 0.0,
            output: 0.3,
            eq_db: [0.0; 8],
        },
        Preset {
            name: "Music".into(),
            bass: 0.6,
            fidelity: 0.55,
            ambience: 0.5,
            output: 0.5,
            eq_db: [3.0, 5.0, 3.0, -1.0, 0.0, 0.0, 2.0, 3.0],
        },
        Preset {
            name: "Movie".into(),
            bass: 0.65,
            fidelity: 0.5,
            ambience: 0.7,
            output: 0.55,
            eq_db: [4.0, 5.0, 2.0, -2.0, 0.0, 3.0, 4.0, 1.0],
        },
        Preset {
            name: "Voice / Podcast".into(),
            bass: 0.0,
            fidelity: 0.4,
            ambience: 0.0,
            output: 0.6,
            eq_db: [-8.0, -6.0, -3.0, -2.0, 0.0, 2.0, 4.0, 1.0],
        },
        Preset {
            name: "Gaming".into(),
            bass: 0.5,
            fidelity: 0.6,
            ambience: 0.75,
            output: 0.55,
            eq_db: [2.0, 4.0, 2.0, -1.0, 0.0, 2.0, 3.0, 4.0],
        },
    ]
}
