//! Installation and removal of FxLinux's per-user files.

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;

const FILTER_CHAIN_TEMPLATE: &str = include_str!("../../../data/filter-chain/99-fxlinux.conf");
const LEGACY_FILTER_CHAIN: &str = include_str!("../../../data/filter-chain/99-fxlinux-legacy.conf");
const TARGET_PLACEHOLDER: &str = "@FXLINUX_TARGET_OBJECT@";
const MANAGED_MARKER: &str = "# Managed by FxLinux. Changes will be replaced on upgrade.";
const AUTOSTART_MARKER: &str = "# Managed by FxLinux. Changes will be replaced on upgrade.";
const CONFIG_RELATIVE_PATH: &str = "pipewire/filter-chain.conf.d/99-fxlinux.conf";
const AUTOSTART_RELATIVE_PATH: &str = "autostart/io.github.fxlinux.FxLinux.desktop";
const APPLICATION_RELATIVE_PATH: &str = "applications/io.github.fxlinux.FxLinux.desktop";
const BINARY_STATE_RELATIVE_PATH: &str = "fxlinux/install-state-v1";

const LEGACY_DESKTOP_PREFIX: &str =
    "[Desktop Entry]\nType=Application\nName=FxLinux\nComment=Audio enhancer control panel\nExec=";
const LEGACY_DESKTOP_SUFFIX: &str =
    "\nIcon=multimedia-volume-control\nTerminal=false\nX-GNOME-Autostart-enabled=true\n";

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

#[derive(Debug, PartialEq, Eq)]
struct BinaryIdentity {
    size: u64,
    digest: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum BinaryInstallAction {
    Create,
    Adopt,
    Replace,
    Keep,
}

struct BinaryInstallPlan {
    action: BinaryInstallAction,
    bytes: Vec<u8>,
    identity: BinaryIdentity,
}

pub fn run(output: Option<&str>) -> Result<()> {
    // Finish every fallible validation before changing files or service state.
    let base = BaseDirs::new().context("could not resolve home directory")?;
    let config_dir = base.config_dir();
    let home_dir = base.home_dir();
    let config_path = config_dir.join(CONFIG_RELATIVE_PATH);
    let autostart_path = config_dir.join(AUTOSTART_RELATIVE_PATH);
    let application_path = base.data_dir().join(APPLICATION_RELATIVE_PATH);
    let binary_path = home_dir.join(".local/bin/fxlinux-app");
    let binary_state_path = base.data_dir().join(BINARY_STATE_RELATIVE_PATH);
    let current_exe = std::env::current_exe().context("resolving current executable path")?;

    let sink = select_sink(output, &config_path)?;
    let rendered_config = render_filter_chain(&sink.name)?;

    validate_executable(&current_exe)?;
    let config_decision =
        managed_file_decision(&config_path, &rendered_config, MANAGED_MARKER, |contents| {
            contents == LEGACY_FILTER_CHAIN
        })?;
    let desktop_entry = render_desktop_entry(&binary_path)?;
    let autostart_decision = managed_file_decision(
        &autostart_path,
        &desktop_entry,
        AUTOSTART_MARKER,
        is_legacy_desktop,
    )?;
    let application_decision =
        managed_file_decision(&application_path, &desktop_entry, AUTOSTART_MARKER, |_| {
            false
        })?;
    let binary_plan = plan_binary_install(&current_exe, &binary_path, &binary_state_path)?;

    install_managed_file(&config_path, &rendered_config, config_decision)?;
    install_binary(&current_exe, &binary_path, &binary_state_path, binary_plan)?;
    install_managed_file(&autostart_path, &desktop_entry, autostart_decision)?;
    install_managed_file(&application_path, &desktop_entry, application_decision)?;
    enable_and_restart_filter_chain_service()?;

    println!(
        "FxLinux installed for output: {} ({})",
        sink.description, sink.name
    );
    println!("- DSP chain: {}", config_path.display());
    println!("- Application: {}", binary_path.display());
    println!("- filter-chain.service: enabled and active");
    println!("- Control panel autostart: {}", autostart_path.display());
    println!("- Application launcher: {}", application_path.display());
    println!();
    println!("Select the FxLinux sink in your desktop sound settings to use the enhancer.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let base = BaseDirs::new().context("could not resolve home directory")?;
    let config_dir = base.config_dir();
    let config_path = config_dir.join(CONFIG_RELATIVE_PATH);
    let autostart_path = config_dir.join(AUTOSTART_RELATIVE_PATH);
    let application_path = base.data_dir().join(APPLICATION_RELATIVE_PATH);
    let binary_path = base.home_dir().join(".local/bin/fxlinux-app");
    let binary_state_path = base.data_dir().join(BINARY_STATE_RELATIVE_PATH);

    // Prove binary ownership before changing any file or service state.
    validate_binary_uninstall(&binary_path, &binary_state_path)?;

    let removed_config = remove_if_managed(&config_path, MANAGED_MARKER)?;
    remove_if_managed(&autostart_path, AUTOSTART_MARKER)?;
    remove_if_managed(&application_path, AUTOSTART_MARKER)?;
    remove_installed_binary(&binary_path, &binary_state_path)?;

    if removed_config {
        // This is a generic PipeWire service, so preserve its enablement for
        // any other filter-chain users while reloading away FxLinux's node.
        run_systemctl(&["try-restart", "filter-chain.service"])?;
    }

    println!("FxLinux uninstalled.");
    Ok(())
}

fn select_sink(requested_name: Option<&str>, config_path: &Path) -> Result<Sink> {
    if let Some(name) = requested_name {
        return inspect_named_sink(name);
    }

    let default_inspection = inspect_sink("@DEFAULT_AUDIO_SINK@")?;
    if inspection_property(&default_inspection, "node.name").as_deref() == Some("fxlinux_input") {
        let target = managed_target(config_path)?;
        return inspect_named_sink(&target).context(
            "the physical output saved in the FxLinux config is no longer available; rerun install with --output NODE_NAME",
        );
    }

    parse_sink_inspection(&default_inspection)
}

fn inspect_named_sink(name: &str) -> Result<Sink> {
    validate_node_name(name)?;
    let inspect_target = resolve_node_id(name)?.to_string();
    let inspection = inspect_sink(&inspect_target)?;
    let sink = parse_sink_inspection(&inspection)?;
    if sink.name != name {
        bail!("wpctl resolved {name} to a different node ({})", sink.name);
    }
    Ok(sink)
}

fn inspect_sink(inspect_target: &str) -> Result<String> {
    let output = Command::new("wpctl")
        .args(["inspect", inspect_target])
        .output()
        .with_context(|| format!("inspecting output {inspect_target} with wpctl"))?;
    if !output.status.success() {
        bail!("wpctl could not inspect output {inspect_target}");
    }
    Ok(std::str::from_utf8(&output.stdout)
        .context("wpctl returned non-UTF-8 output")?
        .to_owned())
}

fn managed_target(path: &Path) -> Result<String> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("reading managed FxLinux config at {}", path.display()))?;
    if contents == LEGACY_FILTER_CHAIN {
        bail!(
            "the default sink is the legacy FxLinux sink with no saved physical output; rerun install with --output NODE_NAME"
        );
    }
    if !is_managed(&contents, MANAGED_MARKER) {
        bail!(
            "the default sink is FxLinux, but {} is not managed by FxLinux; rerun install with --output NODE_NAME",
            path.display()
        );
    }

    for line in contents.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        if key.trim() != "target.object" {
            continue;
        }
        let target = parse_quoted_value(value.trim())
            .context("managed FxLinux config has an invalid target.object")?;
        validate_node_name(&target)?;
        return Ok(target);
    }
    bail!("managed FxLinux config has no target.object")
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

fn render_desktop_entry(binary_path: &Path) -> Result<String> {
    let path = binary_path
        .to_str()
        .context("installed executable path is not valid UTF-8")?;
    if path.bytes().any(|byte| byte == b'\n' || byte == b'\r') {
        bail!("installed executable path contains a newline");
    }
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(
        "{AUTOSTART_MARKER}\n[Desktop Entry]\nType=Application\nName=FxLinux\nComment=Audio enhancer control panel\nExec=\"{escaped}\"\nIcon=multimedia-volume-control\nTerminal=false\nCategories=AudioVideo;Audio;Settings;\nX-GNOME-Autostart-enabled=true\n"
    ))
}

fn managed_file_decision(
    path: &Path,
    desired: &str,
    marker: &str,
    is_legacy: impl FnOnce(&str) -> bool,
) -> Result<ManagedFileDecision> {
    let existing = match fs::read_to_string(path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedFileDecision::Create);
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if !is_managed(&existing, marker) && !is_legacy(&existing) {
        bail!("refusing to overwrite unrecognised file {}", path.display());
    }
    if existing == desired {
        Ok(ManagedFileDecision::Keep)
    } else {
        Ok(ManagedFileDecision::Replace)
    }
}

fn is_legacy_desktop(contents: &str) -> bool {
    let Some(exec) = contents
        .strip_prefix(LEGACY_DESKTOP_PREFIX)
        .and_then(|rest| rest.strip_suffix(LEGACY_DESKTOP_SUFFIX))
    else {
        return false;
    };
    if exec.is_empty() || exec.contains(['\n', '\r']) {
        return false;
    }
    let path = Path::new(exec);
    path.is_absolute() && path.ends_with("target/release/fxlinux-app")
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

fn validate_binary_destination(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            bail!(
                "installed executable path is not a regular file: {}",
                path.display()
            )
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("checking {}", path.display())),
    }
}

fn plan_binary_install(
    source: &Path,
    destination: &Path,
    state_path: &Path,
) -> Result<BinaryInstallPlan> {
    let bytes = fs::read(source).with_context(|| format!("reading {}", source.display()))?;
    let identity = binary_identity(&bytes);
    let destination_exists = validate_binary_destination(destination)?;
    let state = read_binary_state(state_path)?;

    let action = if !destination_exists {
        BinaryInstallAction::Create
    } else {
        let existing = fs::read(destination).with_context(|| {
            format!("reading installed executable at {}", destination.display())
        })?;
        match state {
            Some(recorded) => {
                if binary_identity(&existing) != recorded {
                    bail!(
                        "refusing to replace modified managed executable {}; restore it or remove it and {} before reinstalling",
                        destination.display(),
                        state_path.display()
                    );
                }
                if source == destination || existing == bytes {
                    BinaryInstallAction::Keep
                } else {
                    BinaryInstallAction::Replace
                }
            }
            None if existing == bytes => BinaryInstallAction::Adopt,
            None => bail!(
                "refusing to overwrite unowned executable {}; move it aside or make it byte-for-byte identical to the FxLinux executable being installed",
                destination.display()
            ),
        }
    };

    Ok(BinaryInstallPlan {
        action,
        bytes,
        identity,
    })
}

fn install_binary(
    source: &Path,
    destination: &Path,
    state_path: &Path,
    plan: BinaryInstallPlan,
) -> Result<()> {
    if matches!(plan.action, BinaryInstallAction::Keep) {
        println!("Kept {} (managed copy is current)", destination.display());
    } else if matches!(plan.action, BinaryInstallAction::Adopt) {
        println!("Adopted existing {}", destination.display());
    } else {
        atomic_write(destination, &plan.bytes)?;
        let permissions = fs::metadata(source)?.permissions();
        fs::set_permissions(destination, permissions)
            .with_context(|| format!("setting permissions on {}", destination.display()))?;
        println!("Wrote {}", destination.display());
    }
    atomic_write(state_path, format_binary_state(&plan.identity).as_bytes())?;
    Ok(())
}

fn binary_identity(bytes: &[u8]) -> BinaryIdentity {
    // FNV-1a is stable and dependency-free. This identity prevents accidental
    // data loss. It is not intended to provide cryptographic authentication.
    let mut digest = 0xcbf29ce484222325_u64;
    for byte in bytes {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x100000001b3);
    }
    BinaryIdentity {
        size: bytes.len() as u64,
        digest,
    }
}

fn format_binary_state(identity: &BinaryIdentity) -> String {
    format!(
        "version=1\nsize={}\nfnv1a64={:016x}\n",
        identity.size, identity.digest
    )
}

fn parse_binary_state(contents: &str) -> Result<BinaryIdentity> {
    let mut lines = contents.lines();
    if lines.next() != Some("version=1") {
        bail!("unsupported or invalid FxLinux binary state version");
    }
    let size = lines
        .next()
        .and_then(|line| line.strip_prefix("size="))
        .context("FxLinux binary state has no size")?
        .parse::<u64>()
        .context("FxLinux binary state has an invalid size")?;
    let digest_text = lines
        .next()
        .and_then(|line| line.strip_prefix("fnv1a64="))
        .context("FxLinux binary state has no digest")?;
    if digest_text.len() != 16 || !digest_text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("FxLinux binary state has an invalid digest");
    }
    let digest = u64::from_str_radix(digest_text, 16)
        .context("FxLinux binary state has an invalid digest")?;
    if lines.next().is_some() || !contents.ends_with('\n') {
        bail!("FxLinux binary state has unexpected content");
    }
    Ok(BinaryIdentity { size, digest })
}

fn read_binary_state(path: &Path) -> Result<Option<BinaryIdentity>> {
    match fs::read_to_string(path) {
        Ok(contents) => parse_binary_state(&contents)
            .with_context(|| format!("reading managed binary state at {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
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

fn validate_binary_uninstall(path: &Path, state_path: &Path) -> Result<()> {
    let Some(recorded) = read_binary_state(state_path)? else {
        if fs::symlink_metadata(path).is_ok() {
            bail!(
                "refusing to remove unowned executable {}; no valid managed state exists at {}",
                path.display(),
                state_path.display()
            );
        }
        return Ok(());
    };
    validate_binary_destination(path).with_context(|| {
        format!(
            "managed binary state exists at {}, but the executable is not safe to remove",
            state_path.display()
        )
    })?;
    let bytes = fs::read(path).with_context(|| {
        format!(
            "managed binary state exists at {}, but {} cannot be read; restore the managed binary before uninstalling",
            state_path.display(),
            path.display()
        )
    })?;
    if binary_identity(&bytes) != recorded {
        bail!(
            "refusing to remove modified managed executable {}; restore the recorded binary before uninstalling",
            path.display()
        );
    }
    Ok(())
}

fn remove_installed_binary(path: &Path, state_path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_err() && !state_path.exists() {
        return Ok(());
    }
    fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    println!("Removed {}", path.display());
    fs::remove_file(state_path)
        .with_context(|| format!("removing binary state at {}", state_path.display()))?;
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
    fn reads_target_from_managed_config() {
        let path = test_path("managed-target");
        fs::write(
            &path,
            format!(
                "{MANAGED_MARKER}\nplayback.props = {{\n    target.object = \"alsa_output.test\"\n}}\n"
            ),
        )
        .unwrap();
        assert_eq!(managed_target(&path).unwrap(), "alsa_output.test");

        fs::write(&path, "user managed").unwrap();
        assert!(managed_target(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn renders_indexable_desktop_entry() {
        let entry = render_desktop_entry(Path::new("/home/test/.local/bin/fxlinux-app")).unwrap();
        assert!(entry.starts_with(AUTOSTART_MARKER));
        assert!(entry.contains("Name=FxLinux\n"));
        assert!(entry.contains("Exec=\"/home/test/.local/bin/fxlinux-app\"\n"));
        assert!(entry.contains("Categories=AudioVideo;Audio;Settings;\n"));
    }

    #[test]
    fn decides_create_keep_replace_and_rejects_user_file() {
        let path = test_path("managed-decision");
        assert_eq!(
            managed_file_decision(&path, "desired", MANAGED_MARKER, |_| false).unwrap(),
            ManagedFileDecision::Create
        );
        fs::write(&path, format!("{MANAGED_MARKER}\ndesired")).unwrap();
        let desired = format!("{MANAGED_MARKER}\ndesired");
        assert_eq!(
            managed_file_decision(&path, &desired, MANAGED_MARKER, |_| false).unwrap(),
            ManagedFileDecision::Keep
        );
        assert_eq!(
            managed_file_decision(
                &path,
                &format!("{MANAGED_MARKER}\nnew"),
                MANAGED_MARKER,
                |_| false,
            )
            .unwrap(),
            ManagedFileDecision::Replace
        );
        fs::write(&path, "user managed").unwrap();
        assert!(managed_file_decision(&path, "desired", MANAGED_MARKER, |_| false).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn accepts_only_exact_legacy_filter_chain_and_replaces_it() {
        let path = test_path("legacy-filter-chain");
        let desired = render_filter_chain("alsa_output.test").unwrap();
        fs::write(&path, LEGACY_FILTER_CHAIN).unwrap();
        let decision = managed_file_decision(&path, &desired, MANAGED_MARKER, |contents| {
            contents == LEGACY_FILTER_CHAIN
        })
        .unwrap();
        assert_eq!(decision, ManagedFileDecision::Replace);
        install_managed_file(&path, &desired, decision).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), desired);

        let mut changed = LEGACY_FILTER_CHAIN.as_bytes().to_vec();
        changed[0] ^= 1;
        fs::write(&path, changed).unwrap();
        assert!(
            managed_file_decision(&path, &desired, MANAGED_MARKER, |contents| {
                contents == LEGACY_FILTER_CHAIN
            })
            .is_err()
        );
        fs::write(&path, format!("{LEGACY_FILTER_CHAIN}# extra\n")).unwrap();
        assert!(
            managed_file_decision(&path, &desired, MANAGED_MARKER, |contents| {
                contents == LEGACY_FILTER_CHAIN
            })
            .is_err()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn accepts_only_exact_legacy_desktop_shape() {
        let exact = format!(
            "{LEGACY_DESKTOP_PREFIX}/home/test/fxlinux/target/release/fxlinux-app{LEGACY_DESKTOP_SUFFIX}"
        );
        assert!(is_legacy_desktop(&exact));

        let mut changed = exact.clone().into_bytes();
        changed[0] ^= 1;
        assert!(!is_legacy_desktop(std::str::from_utf8(&changed).unwrap()));
        assert!(!is_legacy_desktop(&format!("{exact}Extra=true\n")));
        assert!(!is_legacy_desktop(&exact.replace(
            "/target/release/fxlinux-app",
            "/target/debug/fxlinux-app"
        )));
        assert!(!is_legacy_desktop(&exact.replacen("/home", "home", 1)));
    }

    fn binary_paths(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = test_path(name);
        (
            root.join("source"),
            root.join("bin/fxlinux-app"),
            root.join("data/install-state-v1"),
        )
    }

    fn write_test_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn remove_test_root(path: &Path) {
        let root = path
            .ancestors()
            .find(|ancestor| {
                ancestor
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("fxlinux-binary-"))
            })
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installs_binary_at_absent_destination() {
        let (source, destination, state) = binary_paths("binary-create");
        write_test_file(&source, b"new binary");
        let plan = plan_binary_install(&source, &destination, &state).unwrap();
        assert_eq!(plan.action, BinaryInstallAction::Create);
        install_binary(&source, &destination, &state, plan).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new binary");
        assert_eq!(
            read_binary_state(&state).unwrap(),
            Some(binary_identity(b"new binary"))
        );
        remove_test_root(&destination);
    }

    #[test]
    fn adopts_only_identical_unmanaged_binary() {
        let (source, destination, state) = binary_paths("binary-adopt");
        write_test_file(&source, b"same binary");
        write_test_file(&destination, b"same binary");
        let plan = plan_binary_install(&source, &destination, &state).unwrap();
        assert_eq!(plan.action, BinaryInstallAction::Adopt);
        install_binary(&source, &destination, &state, plan).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"same binary");
        assert!(state.exists());
        remove_test_root(&destination);
    }

    #[test]
    fn refuses_unrelated_unmanaged_binary() {
        let (source, destination, state) = binary_paths("binary-unowned");
        write_test_file(&source, b"new binary");
        write_test_file(&destination, b"someone else's file");
        assert!(plan_binary_install(&source, &destination, &state).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"someone else's file");
        remove_test_root(&destination);
    }

    #[test]
    fn upgrades_verified_managed_binary() {
        let (source, destination, state) = binary_paths("binary-upgrade");
        write_test_file(&source, b"new binary");
        write_test_file(&destination, b"old binary");
        write_test_file(
            &state,
            format_binary_state(&binary_identity(b"old binary")).as_bytes(),
        );
        let plan = plan_binary_install(&source, &destination, &state).unwrap();
        assert_eq!(plan.action, BinaryInstallAction::Replace);
        install_binary(&source, &destination, &state, plan).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new binary");
        assert_eq!(
            read_binary_state(&state).unwrap(),
            Some(binary_identity(b"new binary"))
        );
        remove_test_root(&destination);
    }

    #[test]
    fn refuses_modified_managed_binary() {
        let (source, destination, state) = binary_paths("binary-modified");
        write_test_file(&source, b"new binary");
        write_test_file(&destination, b"modified binary");
        write_test_file(
            &state,
            format_binary_state(&binary_identity(b"old binary")).as_bytes(),
        );
        assert!(plan_binary_install(&source, &destination, &state).is_err());
        assert!(validate_binary_uninstall(&destination, &state).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"modified binary");
        remove_test_root(&destination);
    }

    #[test]
    fn safely_uninstalls_verified_binary_then_state() {
        let (_source, destination, state) = binary_paths("binary-uninstall");
        write_test_file(&destination, b"managed binary");
        write_test_file(
            &state,
            format_binary_state(&binary_identity(b"managed binary")).as_bytes(),
        );
        validate_binary_uninstall(&destination, &state).unwrap();
        remove_installed_binary(&destination, &state).unwrap();
        assert!(!destination.exists());
        assert!(!state.exists());
        remove_test_root(&destination);
    }

    #[test]
    fn parses_only_canonical_binary_state() {
        let identity = binary_identity(b"binary");
        let state = format_binary_state(&identity);
        assert_eq!(parse_binary_state(&state).unwrap(), identity);
        assert!(parse_binary_state(&format!("{state}extra=true\n")).is_err());
        assert!(parse_binary_state(&state.replace("version=1", "version=2")).is_err());
        assert!(parse_binary_state(state.trim_end()).is_err());
        assert!(parse_binary_state("version=1\nsize=x\nfnv1a64=0000000000000000\n").is_err());
    }

    #[test]
    fn keeps_current_executable_when_it_is_destination() {
        let (source, _destination, state) = binary_paths("binary-current");
        write_test_file(&source, b"current binary");
        write_test_file(
            &state,
            format_binary_state(&binary_identity(b"current binary")).as_bytes(),
        );
        let plan = plan_binary_install(&source, &source, &state).unwrap();
        assert_eq!(plan.action, BinaryInstallAction::Keep);
        install_binary(&source, &source, &state, plan).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"current binary");
        remove_test_root(&source);
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
