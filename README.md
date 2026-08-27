# FxLinux

A from-scratch Linux audio enhancer inspired by [FxSound](https://www.fxsound.com/) (Windows-only)
— built on PipeWire and open-source LV2 plugins, not a port or decompilation of FxSound's binary.
A simple GTK4 control panel (a few sliders + genre presets) sits on top of a tuned multi-stage DSP
chain that runs as a PipeWire virtual sink, so any application's audio gets processed system-wide.

## Features

- **Live DSP chain**, tuned by ear stage by stage:
  - Harmonic **bass enhancer** and **exciter** (Calf) for fuller low end and clearer highs
  - **Stereo widening + ambience** (Calf Stereo Tools + Reverb)
  - **Adaptive bass leveling** — a multiband compressor gently reins in already-hot bass
    automatically, so you don't have to retune the Bass slider per track
  - **8-band parametric EQ** (LSP), driving each preset's actual frequency-response curve
  - **True-peak-safe limiter** (LSP) with headroom margin, so nothing clips regardless of how
    far the sliders are pushed
- **5 presets**, each with its own EQ curve, not just slider values:
  | Preset | Character |
  |---|---|
  | Flat / Reference | Minimal processing, just the safety limiter — baseline for A/B |
  | Music | Warm bass, gentle treble lift, moderate width |
  | Movie | Dialogue-presence boost, stronger bass, wider ambience |
  | Voice / Podcast | Rumble/proximity cut, articulation boost, dry |
  | Gaming | Footstep/cue clarity, wide ambience for positional feel |
- **Live control** — dragging a slider or switching presets updates the running audio engine
  instantly, no restart or audio interruption
- **Auto-reconnect** — if the audio engine restarts for any reason, the control panel detects it
  and automatically re-applies your last-selected preset
- **Self-installing** — one script sets up system packages, builds, and wires everything into
  PipeWire + your login session

## How it works

The DSP chain runs inside PipeWire's own `filter-chain` daemon (a small process PipeWire ships
specifically for this), loading Calf and LSP LV2 plugins per
[`data/filter-chain/99-fxlinux.conf`](data/filter-chain/99-fxlinux.conf). This creates a virtual
sink named **FxLinux** that any app's audio can be routed through. The GTK4 app
(`crates/fxlinux-app`) is a separate control panel: it talks to the *already-running* audio engine
over PipeWire's native protocol, live-updating plugin parameters — it does not host any audio
processing itself.

## Requirements

- Ubuntu or another apt-based distro running **PipeWire** (not PulseAudio-only — check with
  `pactl info | grep -i pipewire` or `wpctl status`)
- `sudo` access, for one-time package installation

## Installation

```sh
./install.sh
```

This installs the required system packages (build tools, GTK4/libadwaita, PipeWire dev headers,
the Calf and LSP plugin sets), installs Rust via [rustup](https://rustup.rs) if not already
present, builds FxLinux in release mode, and runs `fxlinux-app install` to:

- write the DSP chain config to `~/.config/pipewire/filter-chain.conf.d/99-fxlinux.conf`
  (skipped if one already exists, so it won't clobber any hand-tuning)
- enable and start `filter-chain.service`
- add an autostart entry so the control panel launches at login

Safe to re-run at any time.

### After installing

Set **FxLinux** as your default audio output — GNOME Sound Settings, or:

```sh
wpctl status                 # find FxLinux's sink id under "Filters"
wpctl set-default <id>
```

Then launch the control panel (or just log back in — it autostarts from then on):

```sh
./target/release/fxlinux-app
```

## Usage

- The **preset dropdown** picks a full tuned sound (EQ curve + slider positions) in one go.
- The four sliders — **Bass Boost**, **Fidelity**, **Ambience**, **Output** — fine-tune on top of
  whatever preset is selected, live.
- If the banner "Audio engine not connected" appears, `filter-chain.service` isn't running —
  check `systemctl --user status filter-chain.service`.

## Project structure

```
fxlinux/
  install.sh                              # bootstrap: packages, Rust, build, install
  data/filter-chain/99-fxlinux.conf       # the tuned DSP chain (source of truth for the installer)
  crates/fxlinux-app/
    src/main.rs                           # entry point + `install` subcommand
    src/install.rs                        # writes config/service/autostart
    src/audio/controls.rs                 # slider -> plugin control mapping
    src/audio/pw_worker.rs                # PipeWire connection, live control, reconnect handling
    src/presets/model.rs                  # preset schema + the 5 factory presets
    src/presets/store.rs                  # JSON persistence under ~/.config/fxlinux/presets/
    src/ui/window.rs                      # GTK4 + libadwaita UI
    examples/set_control.rs               # debug CLI: poke one control live from the terminal
```

## Development

```sh
cargo build                                          # debug build
cargo run --example set_control -- bass:amount 3.0   # poke a live control directly, for debugging
```

Presets are plain JSON under `~/.config/fxlinux/presets/` (seeded from the factory set on first
run) — hand-edit or add files there to experiment with your own.
