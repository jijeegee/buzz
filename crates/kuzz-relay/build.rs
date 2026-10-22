//! Embeds every `locales/<lang>.json` and the shared `i18n/languages.json`,
//! so adding a language is adding a file (see `i18n/TRANSLATING.md`).

use std::path::Path;
use std::{env, fs};

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let locales = manifest.join("locales");
    let languages = manifest.join("../../i18n/languages.json");
    println!("cargo:rerun-if-changed={}", locales.display());
    println!("cargo:rerun-if-changed={}", languages.display());

    let mut files: Vec<(String, String)> = fs::read_dir(&locales)
        .expect("crates/kuzz-relay/locales")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let lang = path.file_stem()?.to_str()?.to_owned();
            (path.extension()? == "json").then(|| (lang, path.display().to_string()))
        })
        .collect();
    files.sort();

    let mut out = String::from("pub(crate) static LOCALE_FILES: &[(&str, &str)] = &[\n");
    for (lang, path) in &files {
        out.push_str(&format!("    ({lang:?}, include_str!({path:?})),\n"));
    }
    out.push_str("];\n");
    out.push_str(&format!(
        "pub(crate) static LANGUAGES_JSON: &str = include_str!({:?});\n",
        languages.display().to_string()
    ));
    let dest = Path::new(&env::var("OUT_DIR").unwrap()).join("locales.rs");
    fs::write(dest, out).unwrap();
}
