# FxLinux

A from-scratch Linux audio enhancer inspired by [FxSound](https://www.fxsound.com/) (Windows-only),
built on PipeWire and open-source LV2 plugins. It is not a port or decompilation of FxSound's binary.
A simple GTK4 control panel (a few sliders + genre presets) sits on top of a tuned multi-stage DSP
chain that runs as a PipeWire virtual sink, so any application's audio gets processed system-wide.

## Features

- **Live DSP chain**, tuned by ear stage by stage:
  - Harmonic **bass enhancer** and **exciter** (Calf) for fuller low end and clearer highs
  - **Stereo widening + ambience** (Calf Stereo Tools + Reverb)
  - **Adaptive bass leveling** uses a multiband compressor to gently rein in already-hot bass
    automatically, so you don't have to retune the Bass slider per track
  - **8-band parametric EQ** (LSP), driving each preset's actual frequency-response curve
  - **True-peak-safe limiter** (LSP) with headroom margin, so nothing clips regardless of how
    far the sliders are pushed
- **5 presets**, each with its own EQ curve, not just slider values:
  | Preset | Character |
  |---|---|
  | Flat / Reference | Minimal processing, with the safety limiter as an A/B baseline |
  | Music | Warm bass, gentle treble lift, moderate width |
  | Movie | Dialogue-presence boost, stronger bass, wider ambience |
  | Voice / Podcast | Rumble/proximity cut, articulation boost, dry |
  | Gaming | Footstep/cue clarity, wide ambience for positional feel |
- **Live control** updates the running audio engine when you drag a slider or switch presets,
  instantly, no restart or audio interruption
- **Auto-reconnect** detects an audio-engine restart
  and automatically re-applies your last-selected preset
- **Self-installing** sets up system packages, builds the application, and connects it to
  PipeWire + your login session

## How it works

The DSP chain runs inside PipeWire's own `filter-chain` daemon (a small process PipeWire ships
specifically for this), loading Calf and LSP LV2 plugins per
[`data/filter-chain/99-fxlinux.conf`](data/filter-chain/99-fxlinux.conf). This creates a virtual
sink named **FxLinux** that any app's audio can be routed through. The GTK4 app
(`crates/fxlinux-app`) is a separate control panel: it talks to the *already-running* audio engine
over PipeWire's native protocol and updates plugin parameters live. It does not host any audio
processing itself.

## Requirements

- Debian, Ubuntu, Arch Linux, or CachyOS running **PipeWire** rather than PulseAudio alone.
  Check with `pactl info | grep -i pipewire` or `wpctl status`.
- `sudo` access, for one-time package installation

## Installation

```sh
./install.sh
```

The installer uses `apt` on Debian and Ubuntu, or `pacman` on Arch Linux and CachyOS. It installs
the required build tools, GTK4/libadwaita, PipeWire headers, and the Calf and LSP plugin sets. If
Rust is missing, it downloads the rustup installer to a temporary file before running it.

Before it creates the virtual sink, the installer records the current physical output and pins
the processed stream to that device. This prevents the session manager from sending processed
audio to an unrelated output after FxLinux becomes the default sink.

The install command then:

- installs the application at `~/.local/bin/fxlinux-app`
- writes a managed DSP config to `~/.config/pipewire/filter-chain.conf.d/99-fxlinux.conf`
- enables and restarts `filter-chain.service`, then checks that it is active
- adds an autostart entry for the installed application

FxLinux updates files marked as managed by FxLinux. It refuses to overwrite an existing DSP config
or autostart entry that it does not recognise.

To choose a physical output explicitly, copy its `node.name` from `wpctl status --name`:

```sh
./install.sh --output alsa_output.usb-example.analog-stereo
```

Re-running the installer while FxLinux is the default sink reuses the pinned physical output and
checks that it still exists. To change the physical output later, run:

```sh
fxlinux-app install --output <node.name>
```

### After installing

Set **FxLinux** as your default audio output in your desktop sound settings, or run:

```sh
wpctl status                 # find FxLinux's sink id under "Filters"
wpctl set-default <id>
```

Then launch the control panel. It will also start automatically on your next login.

```sh
fxlinux-app
```

## Usage

- The **preset dropdown** picks a full tuned sound (EQ curve + slider positions) in one go.
- The four sliders, **Bass Boost**, **Fidelity**, **Ambience**, and **Output**, fine-tune on top of
  whatever preset is selected, live.
- The status banner distinguishes a missing engine, missing output stream, unlinked output, and a
  route connected to an unusable sink. Start with `systemctl --user status filter-chain.service`
  and `wpctl status` when it reports a fault.

## Uninstall

```sh
fxlinux-app uninstall
```

Uninstall removes only files marked as managed by FxLinux. It leaves the generic PipeWire
`filter-chain.service` enabled for any other filter-chain configurations and reloads it after
removing the FxLinux node.

## Project structure

```
fxlinux/
  install.sh                              # bootstrap: packages, Rust, build, install
  data/filter-chain/99-fxlinux.conf       # the tuned DSP chain (source of truth for the installer)
  crates/fxlinux-app/
    src/main.rs                           # entry point, install, and uninstall commands
    src/install.rs                        # safe output selection and managed file lifecycle
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

Presets are plain JSON under `~/.config/fxlinux/presets/` and are seeded from the factory set on
first run. You can edit them or add files there to experiment with your own.
