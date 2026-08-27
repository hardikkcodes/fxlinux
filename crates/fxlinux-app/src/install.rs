//! `fxlinux-app install`: sets up everything needed to run FxLinux without
//! having gone through this session's manual setup —
//! `~/.config/pipewire/filter-chain.conf.d/99-fxlinux.conf` (the ear-tuned
//! DSP chain), `filter-chain.service` enabled, and an XDG autostart entry
//! for this control-panel binary.

use anyhow::{Context, Result};
use directories::BaseDirs;
use std::fs;
use std::process::Command;

const FILTER_CHAIN_CONF: &str = include_str!("../../../data/filter-chain/99-fxlinux.conf");

pub fn run() -> Result<()> {
    let base = BaseDirs::new().context("could not resolve home directory")?;
    let config_dir = base.config_dir();

    install_filter_chain_conf(config_dir)?;
    enable_filter_chain_service()?;
    install_autostart_entry(config_dir)?;

    println!("FxLinux installed.");
    println!("- DSP chain: {}/pipewire/filter-chain.conf.d/99-fxlinux.conf", config_dir.display());
    println!("- filter-chain.service: enabled and started");
    println!("- Control panel autostart: {}/autostart/io.github.fxlinux.FxLinux.desktop", config_dir.display());
    println!();
    println!("Set the \"FxLinux\" sink as your default output (e.g. `wpctl set-default <id>`");
    println!("or via your desktop's Sound settings), then launch the control panel now with");
    println!("`fxlinux-app`, or just log back in — it will start automatically from now on.");
    Ok(())
}

fn install_filter_chain_conf(config_dir: &std::path::Path) -> Result<()> {
    let dir = config_dir.join("pipewire/filter-chain.conf.d");
    fs::create_dir_all(&dir).context("creating filter-chain.conf.d")?;
    let path = dir.join("99-fxlinux.conf");
    if path.exists() {
        println!("Skipping {} (already exists)", path.display());
    } else {
        fs::write(&path, FILTER_CHAIN_CONF).context("writing 99-fxlinux.conf")?;
        println!("Wrote {}", path.display());
    }
    Ok(())
}

fn enable_filter_chain_service() -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "enable", "--now", "filter-chain.service"])
        .status()
        .context("running systemctl --user enable --now filter-chain.service")?;
    if !status.success() {
        eprintln!(
            "Warning: `systemctl --user enable --now filter-chain.service` exited with {status}. \
             You may need to run it yourself, or restart PipeWire's user services."
        );
    }
    Ok(())
}

fn install_autostart_entry(config_dir: &std::path::Path) -> Result<()> {
    let dir = config_dir.join("autostart");
    fs::create_dir_all(&dir).context("creating autostart dir")?;

    let exe = std::env::current_exe().context("resolving current executable path")?;
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=FxLinux\n\
         Comment=Audio enhancer control panel\n\
         Exec={}\n\
         Icon=multimedia-volume-control\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    );

    let path = dir.join("io.github.fxlinux.FxLinux.desktop");
    fs::write(&path, entry).context("writing autostart .desktop entry")?;
    println!("Wrote {}", path.display());
    Ok(())
}
