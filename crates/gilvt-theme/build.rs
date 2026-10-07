//! Embeds `themes/ghostty/*` as `BUILTIN: &[(&str, &str)]`, sorted by name.
use std::{env, fs, path::PathBuf};

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("themes/ghostty");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    let mut out = String::from("pub static BUILTIN: &[(&str, &str)] = &[\n");
    for n in &names {
        out.push_str(&format!("    ({n:?}, include_str!({:?})),\n", dir.join(n).display().to_string()));
    }
    out.push_str("];\n");
    fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("builtin.rs"), out).unwrap();
}
