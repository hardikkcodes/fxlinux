//! Installation and removal of FxLinux's per-user files.

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;

const FILTER_CHAIN_TEMPLATE: &str = include_str!("../../../data/filter-chain/99-fxlinux.conf");
const TARGET_PLACEHOLDER: &str = "@FXLINUX_TARGET_OBJECT@";
const MANAGED_MARKER: &str = "# Managed by FxLinux. Changes will be replaced on upgrade.";
const AUTOSTART_MARKER: &str = "# Managed by FxLinux. Changes will be replaced on upgrade.";
const CONFIG_RELATIVE_PATH: &str = "pipewire/filter-chain.conf.d/99-fxlinux.conf";
const AUTOSTART_RELATIVE_PATH: &str = "autostart/io.github.fxlinux.FxLinux.desktop";

#[derive(Debug, PartialEq, Eq)]
struct Sink {
    name: String,
    description: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ManagedFileDecision {
    Create,
    Keep,
    Replace,
}

pub fn run(output: Option<&str>) -> Result<()> {
    // Finish every fallible validation before changing files or service state.
    let sink = select_sink(output)?;
    let rendered_config = render_filter_chain(&sink.name)?;
    let base = BaseDirs::new().context("could not resolve home directory")?;
    let config_dir = base.config_dir();
    let home_dir = base.home_dir();
    let config_path = config_dir.join(CONFIG_RELATIVE_PATH);
    let autostart_path = config_dir.join(AUTOSTART_RELATIVE_PATH);
    let binary_path = home_dir.join(".local/bin/fxlinux-app");
    let current_exe = std::env::current_exe().context("resolving current executable path")?;

    validate_executable(&current_exe)?;
    let config_decision = managed_file_decision(&config_path, &rendered_config, MANAGED_MARKER)?;
    let autostart = render_autostart(&binary_path)?;
    let autostart_decision = managed_file_decision(&autostart_path, &autostart, AUTOSTART_MARKER)?;
    validate_binary_destination(&binary_path)?;

    install_managed_file(&config_path, &rendered_config, config_decision)?;
    install_binary(&current_exe, &binary_path)?;
    install_managed_file(&autostart_path, &autostart, autostart_decision)?;
    enable_and_restart_filter_chain_service()?;

    println!(
        "FxLinux installed for output: {} ({})",
        sink.description, sink.name
    );
    println!("- DSP chain: {}", config_path.display());
    println!("- Application: {}", binary_path.display());
    println!("- filter-chain.service: enabled and active");
    println!("- Control panel autostart: {}", autostart_path.display());
    println!();
    println!("Select the FxLinux sink in your desktop sound settings to use the enhancer.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let base = BaseDirs::new().context("could not resolve home directory")?;
    let config_dir = base.config_dir();
    let config_path = config_dir.join(CONFIG_RELATIVE_PATH);
    let autostart_path = config_dir.join(AUTOSTART_RELATIVE_PATH);
    let binary_path = base.home_dir().join(".local/bin/fxlinux-app");

    let removed_config = remove_if_managed(&config_path, MANAGED_MARKER)?;
    remove_if_managed(&autostart_path, AUTOSTART_MARKER)?;
    remove_installed_binary(&binary_path)?;

    if removed_config {
        // This is a generic PipeWire service, so preserve its enablement for
        // any other filter-chain users while reloading away FxLinux's node.
        run_systemctl(&["try-restart", "filter-chain.service"])?;
    }

    println!("FxLinux uninstalled.");
    Ok(())
}

fn select_sink(requested_name: Option<&str>) -> Result<Sink> {
    let inspect_target = match requested_name {
        Some(name) => {
            validate_node_name(name)?;
            resolve_node_id(name)?.to_string()
        }
        None => "@DEFAULT_AUDIO_SINK@".to_owned(),
    };

    let output = Command::new("wpctl")
        .args(["inspect", &inspect_target])
        .output()
        .with_context(|| format!("inspecting output {inspect_target} with wpctl"))?;
    if !output.status.success() {
        bail!("wpctl could not inspect output {inspect_target}");
    }
    let inspection =
        std::str::from_utf8(&output.stdout).context("wpctl returned non-UTF-8 output")?;
    let sink = parse_sink_inspection(inspection)?;
    if let Some(requested) = requested_name
        && sink.name != requested
    {
        bail!(
            "wpctl resolved {requested} to a different node ({})",
            sink.name
        );
    }
    Ok(sink)
}

fn resolve_node_id(name: &str) -> Result<u32> {
    let output = Command::new("wpctl")
        .args(["status", "--name"])
        .output()
        .context("listing PipeWire nodes with wpctl")?;
    if !output.status.success() {
        bail!("wpctl could not list PipeWire nodes");
    }
    let status = std::str::from_utf8(&output.stdout).context("wpctl returned non-UTF-8 output")?;
    parse_node_id(status, name).with_context(|| format!("output node {name} was not found"))
}

fn parse_node_id(status: &str, wanted_name: &str) -> Result<u32> {
    for line in status.lines() {
        let Some((prefix, remainder)) = line.split_once(". ") else {
            continue;
        };
        let Some(id_text) = prefix.split_whitespace().last() else {
            continue;
        };
        let name = remainder.split(" [").next().unwrap_or(remainder).trim();
        if name == wanted_name {
            return id_text
                .parse::<u32>()
                .with_context(|| format!("invalid node ID {id_text}"));
        }
    }
    bail!("node not present in wpctl status")
}

fn parse_sink_inspection(inspection: &str) -> Result<Sink> {
    let name =
        inspection_property(inspection, "node.name").context("selected output has no node.name")?;
    let description = inspection_property(inspection, "node.description")
        .context("selected output has no node.description")?;
    let media_class = inspection_property(inspection, "media.class")
        .context("selected output has no media.class")?;
    let device_api = inspection_property(inspection, "device.api")
        .context("selected output is virtual, not a physical audio device")?;

    validate_node_name(&name)?;
    if name == "fxlinux_input" || name == "fxlinux_output" {
        bail!("FxLinux cannot route into its own virtual node");
    }
    if media_class != "Audio/Sink" {
        bail!("selected output is {media_class}, not Audio/Sink");
    }
    if !matches!(device_api.as_str(), "alsa" | "bluez5") {
        bail!("selected output uses unsupported or virtual device API {device_api}");
    }

    Ok(Sink { name, description })
}

fn inspection_property(inspection: &str, wanted_key: &str) -> Option<String> {
    inspection.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('*').trim_start();
        let (key, value) = line.split_once('=')?;
        if key.trim() != wanted_key {
            return None;
        }
        parse_quoted_value(value.trim())
    })
}

fn parse_quoted_value(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut parsed = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            parsed.push(chars.next()?);
        } else {
            parsed.push(ch);
        }
    }
    Some(parsed)
}

fn validate_node_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        bail!("output node.name contains unsupported characters");
    }
    Ok(())
}

fn render_filter_chain(target: &str) -> Result<String> {
    validate_node_name(target)?;
    if FILTER_CHAIN_TEMPLATE.matches(TARGET_PLACEHOLDER).count() != 1 {
        bail!("filter-chain template must contain exactly one target placeholder");
    }
    Ok(FILTER_CHAIN_TEMPLATE.replacen(TARGET_PLACEHOLDER, target, 1))
}

fn render_autostart(binary_path: &Path) -> Result<String> {
    let path = binary_path
        .to_str()
        .context("installed executable path is not valid UTF-8")?;
    if path.bytes().any(|byte| byte == b'\n' || byte == b'\r') {
        bail!("installed executable path contains a newline");
    }
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(
        "{AUTOSTART_MARKER}\n[Desktop Entry]\nType=Application\nName=FxLinux\nComment=Audio enhancer control panel\nExec=\"{escaped}\"\nIcon=multimedia-volume-control\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    ))
}

fn managed_file_decision(path: &Path, desired: &str, marker: &str) -> Result<ManagedFileDecision> {
    let existing = match fs::read_to_string(path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedFileDecision::Create);
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if !is_managed(&existing, marker) {
        bail!("refusing to overwrite unrecognised file {}", path.display());
    }
    if existing == desired {
        Ok(ManagedFileDecision::Keep)
    } else {
        Ok(ManagedFileDecision::Replace)
    }
}

fn is_managed(contents: &str, marker: &str) -> bool {
    contents.lines().next() == Some(marker)
}

fn install_managed_file(path: &Path, contents: &str, decision: ManagedFileDecision) -> Result<()> {
    match decision {
        ManagedFileDecision::Keep => println!("Kept {}", path.display()),
        ManagedFileDecision::Create | ManagedFileDecision::Replace => {
            atomic_write(path, contents.as_bytes())?;
            println!("Wrote {}", path.display());
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("managed file path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let file_name = path.file_name().context("managed file path has no name")?;

    for attempt in 0..10 {
        let temporary = parent.join(format!(
            ".{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            attempt
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", temporary.display()));
            }
        };
        if let Err(error) = file.write_all(contents).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| format!("writing {}", temporary.display()));
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| format!("replacing {}", path.display()));
        }
        return Ok(());
    }
    bail!(
        "could not create a temporary file beside {}",
        path.display()
    )
}

fn validate_executable(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("reading current executable metadata at {}", path.display()))?;
    if !metadata.is_file() {
        bail!("current executable is not a regular file");
    }
    Ok(())
}

fn validate_binary_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            bail!(
                "installed executable path is not a regular file: {}",
                path.display()
            )
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("checking {}", path.display())),
    }
}

fn install_binary(source: &Path, destination: &Path) -> Result<()> {
    if source == destination {
        println!(
            "Kept {} (already running installed copy)",
            destination.display()
        );
        return Ok(());
    }
    let bytes = fs::read(source).with_context(|| format!("reading {}", source.display()))?;
    atomic_write(destination, &bytes)?;
    let permissions = fs::metadata(source)?.permissions();
    fs::set_permissions(destination, permissions)
        .with_context(|| format!("setting permissions on {}", destination.display()))?;
    println!("Wrote {}", destination.display());
    Ok(())
}

fn enable_and_restart_filter_chain_service() -> Result<()> {
    run_systemctl(&["enable", "filter-chain.service"])?;
    run_systemctl(&["restart", "filter-chain.service"])?;
    run_systemctl(&["is-active", "--quiet", "filter-chain.service"])
        .context("filter-chain.service did not become active")
}

fn run_systemctl(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .status()
        .with_context(|| format!("running systemctl --user {}", arguments.join(" ")))?;
    if !status.success() {
        bail!(
            "systemctl --user {} exited with {status}",
            arguments.join(" ")
        );
    }
    Ok(())
}

fn remove_if_managed(path: &Path, marker: &str) -> Result<bool> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if !is_managed(&contents, marker) {
        println!("Kept unrecognised file {}", path.display());
        return Ok(false);
    }
    fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    println!("Removed {}", path.display());
    Ok(true)
}

fn remove_installed_binary(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => println!("Removed {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("removing {}", path.display())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    const PHYSICAL_SINK: &str = r#"
id 40, type PipeWire:Interface:Node
  * node.name = "alsa_output.usb-Test-00.analog-stereo"
    node.description = "Test USB Audio"
    media.class = "Audio/Sink"
    device.api = "alsa"
"#;

    fn test_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fxlinux-{name}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn parses_physical_sink_inspection() {
        assert_eq!(
            parse_sink_inspection(PHYSICAL_SINK).unwrap(),
            Sink {
                name: "alsa_output.usb-Test-00.analog-stereo".into(),
                description: "Test USB Audio".into(),
            }
        );
    }

    #[test]
    fn rejects_virtual_and_non_sink_nodes() {
        let virtual_sink =
            PHYSICAL_SINK.replace("device.api = \"alsa\"", "device.api = \"support\"");
        assert!(parse_sink_inspection(&virtual_sink).is_err());
        let source = PHYSICAL_SINK.replace("Audio/Sink", "Audio/Source");
        assert!(parse_sink_inspection(&source).is_err());
        let self_sink =
            PHYSICAL_SINK.replace("alsa_output.usb-Test-00.analog-stereo", "fxlinux_input");
        assert!(parse_sink_inspection(&self_sink).is_err());
    }

    #[test]
    fn rejects_config_injection_in_node_name() {
        assert!(validate_node_name("safe.output-name_1:2").is_ok());
        assert!(validate_node_name("sink\"\ncontext.modules = []").is_err());
    }

    #[test]
    fn renders_target_and_no_fallback_properties() {
        let rendered = render_filter_chain("alsa_output.test").unwrap();
        assert!(rendered.contains("target.object      = \"alsa_output.test\""));
        assert!(rendered.lines().any(|line| {
            line.split_whitespace().collect::<Vec<_>>() == ["node.dont-fallback", "=", "true"]
        }));
        assert!(rendered.lines().any(|line| {
            line.split_whitespace().collect::<Vec<_>>() == ["node.dont-reconnect", "=", "true"]
        }));
        assert!(!rendered.contains(TARGET_PLACEHOLDER));
    }

    #[test]
    fn parses_named_node_id_from_status() {
        let status = " ├─ Sinks:\n │  *   40. alsa_output.first [vol: 0.59]\n │      45. alsa_output.second [vol: 1.00]\n";
        assert_eq!(parse_node_id(status, "alsa_output.second").unwrap(), 45);
        assert!(parse_node_id(status, "alsa_output.missing").is_err());
    }

    #[test]
    fn decides_create_keep_replace_and_rejects_user_file() {
        let path = test_path("managed-decision");
        assert_eq!(
            managed_file_decision(&path, "desired", MANAGED_MARKER).unwrap(),
            ManagedFileDecision::Create
        );
        fs::write(&path, format!("{MANAGED_MARKER}\ndesired")).unwrap();
        let desired = format!("{MANAGED_MARKER}\ndesired");
        assert_eq!(
            managed_file_decision(&path, &desired, MANAGED_MARKER).unwrap(),
            ManagedFileDecision::Keep
        );
        assert_eq!(
            managed_file_decision(&path, &format!("{MANAGED_MARKER}\nnew"), MANAGED_MARKER)
                .unwrap(),
            ManagedFileDecision::Replace
        );
        fs::write(&path, "user managed").unwrap();
        assert!(managed_file_decision(&path, "desired", MANAGED_MARKER).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_write_replaces_complete_file() {
        let path = test_path("atomic-write");
        fs::write(&path, "old").unwrap();
        atomic_write(&path, b"new contents").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new contents");
        fs::remove_file(path).unwrap();
    }
}
