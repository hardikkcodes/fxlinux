use super::model::{Preset, factory_presets};
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

fn presets_dir() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("io.github", "fxlinux", "FxLinux")?;
    let dir = dirs.config_dir().join("presets");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Loads presets from disk, seeding the factory set on first run.
pub fn load_all() -> Vec<Preset> {
    let Some(dir) = presets_dir() else {
        return factory_presets();
    };

    let has_any = fs::read_dir(&dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false);

    if !has_any {
        for preset in factory_presets() {
            save(&preset);
        }
    }

    let mut presets: Vec<Preset> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<Preset>(&s).ok())
        .collect();

    if presets.is_empty() {
        presets = factory_presets();
    }

    // fs::read_dir has no ordering guarantee, so sort back to the factory
    // list's intended order (any user-created preset not in that list sorts
    // after, alphabetically, rather than at an arbitrary position).
    let factory_order: Vec<String> = factory_presets().into_iter().map(|p| p.name).collect();
    presets.sort_by(|a, b| {
        let key = |p: &Preset| {
            factory_order
                .iter()
                .position(|n| n == &p.name)
                .map(|i| (0, i))
                .unwrap_or((1, 0))
        };
        key(a).cmp(&key(b)).then_with(|| a.name.cmp(&b.name))
    });

    presets
}

pub fn save(preset: &Preset) {
    let Some(dir) = presets_dir() else { return };
    let path = dir.join(format!("{}.json", slug(&preset.name)));
    if let Ok(json) = serde_json::to_string_pretty(preset) {
        let _ = fs::write(path, json);
    }
}
