//! Emit build metadata only; this example does not prepare or start a native service.
fn main() {
    let mut arguments = std::env::args().skip(1);
    let team = arguments
        .next()
        .expect("Expected a ten-character Apple team identifier");
    assert!(
        arguments.next().is_none(),
        "Only the team identifier is accepted"
    );
    println!(
        "{}",
        spaceprune_access::macos_package::package_layout(&team).expect("Invalid signing team")
    );
}
