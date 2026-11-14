//! The starter game written by `autoengine new`: a small Rust crate whose
//! content is defined in code and whose `main` runs the engine's command
//! line (see [`cli`](super::cli)).
//!
//! The files live in `templates/starter/` in the engine's repository and are
//! built into the library, so the command works wherever the binary is. The
//! new crate depends on the engine by path, pointing at the copy this binary
//! was built from.

use std::path::Path;

/// The starter game's files, as path inside the new folder and contents.
/// `mygame` is replaced by the crate's name.
pub const STARTER: &[(&str, &str)] = &[
    ("Cargo.toml", include_str!("../../templates/starter/Cargo.toml.in")),
    ("README.md", include_str!("../../templates/starter/README.md")),
    ("src/lib.rs", include_str!("../../templates/starter/src/lib.rs")),
    ("src/main.rs", include_str!("../../templates/starter/src/main.rs")),
    ("tests/content.rs", include_str!("../../templates/starter/tests/content.rs")),
    ("scripts/bonus_gold.rhai", include_str!("../../templates/starter/scripts/bonus_gold.rhai")),
];

/// A crate name made from a folder name: lowercase letters, digits and
/// underscores, starting with a letter.
pub fn crate_name(folder: &str) -> String {
    let mut name: String =
        folder.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name.insert_str(0, "game_");
    }
    name
}

/// Writes the starter game into `dir`, which must not exist yet or be
/// empty, so nothing is ever overwritten. Returns the files written.
pub fn write_starter(dir: &Path) -> Result<Vec<String>, String> {
    if dir.exists() {
        let empty = dir.read_dir().map(|mut entries| entries.next().is_none()).unwrap_or(false);
        if !empty {
            return Err(format!("{} already exists and is not empty", dir.display()));
        }
    }
    let name = crate_name(dir.file_name().and_then(|n| n.to_str()).unwrap_or("mygame"));
    let engine = env!("CARGO_MANIFEST_DIR").replace('\\', "/");
    let mut written = Vec::new();
    for (path, text) in STARTER {
        let target = dir.join(path);
        let parent = target.parent().unwrap_or(dir);
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        let text = text.replace("mygame", &name).replace("{engine}", &engine);
        std::fs::write(&target, text).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
        written.push(path.to_string());
    }
    std::fs::create_dir_all(dir.join("assets")).map_err(|e| format!("cannot create assets: {e}"))?;
    Ok(written)
}
