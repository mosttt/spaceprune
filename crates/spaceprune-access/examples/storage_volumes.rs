//! Print OS capacity records without scanning or changing files.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&spaceprune_access::storage::local_storage_volumes()?)?
    );
    Ok(())
}
