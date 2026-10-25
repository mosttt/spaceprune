//! Export versioned public JSON schemas from their authoritative Rust definitions.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas"));
    std::fs::create_dir_all(&directory)?;
    for (name, schema) in spaceprune_contracts::schema::artifacts() {
        let mut encoded = serde_json::to_string_pretty(&schema)?;
        encoded.push('\n');
        std::fs::write(directory.join(name), encoded)?;
    }
    Ok(())
}
