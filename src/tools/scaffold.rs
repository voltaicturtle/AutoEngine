//! The starter game written by `autoengine new`.
//!
//! The files live in `templates/starter/` in the engine's repository and are
//! built into the library, so the command works wherever the binary is.

use std::path::Path;

/// The starter game's files, as path inside the new folder and contents.
pub const STARTER: &[(&str, &str)] = &[
    ("README.md", include_str!("../../templates/starter/README.md")),
    ("units.ron", include_str!("../../templates/starter/units.ron")),
    ("traits.ron", include_str!("../../templates/starter/traits.ron")),
    ("items.ron", include_str!("../../templates/starter/items.ron")),
    ("augments.ron", include_str!("../../templates/starter/augments.ron")),
    ("rules.ron", include_str!("../../templates/starter/rules.ron")),
    ("rounds.ron", include_str!("../../templates/starter/rounds.ron")),
    ("scripts/bonus_gold.rhai", include_str!("../../templates/starter/scripts/bonus_gold.rhai")),
    ("tests.ron", include_str!("../../templates/starter/tests.ron")),
    ("sample_match.ron", include_str!("../../templates/starter/sample_match.ron")),
    ("theme.ron", include_str!("../../templates/starter/theme.ron")),
];

/// Writes the starter game into `dir`, which must not exist yet or be
/// empty, so nothing is ever overwritten. Returns the files written.
pub fn write_starter(dir: &Path) -> Result<Vec<String>, String> {
    if dir.exists() {
        let empty = dir.read_dir().map(|mut entries| entries.next().is_none()).unwrap_or(false);
        if !empty {
            return Err(format!("{} already exists and is not empty", dir.display()));
        }
    }
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("mygame");
    let mut written = Vec::new();
    for (path, text) in STARTER {
        let target = dir.join(path);
        let parent = target.parent().unwrap_or(dir);
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        // The read-me talks about the folder by its real name.
        let text = if *path == "README.md" { text.replace("mygame", name) } else { text.to_string() };
        std::fs::write(&target, text).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
        written.push(path.to_string());
    }
    Ok(written)
}
