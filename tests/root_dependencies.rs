use std::fs;
use std::path::Path;

fn dependencies_section(cargo_toml: &str) -> &str {
    let start = cargo_toml
        .find("[dependencies]\n")
        .expect("[dependencies] section should exist");
    let rest = &cargo_toml[start + "[dependencies]\n".len()..];

    if let Some(next_section) = rest.find("\n[") {
        &rest[..next_section]
    } else {
        rest
    }
}

#[test]
fn root_dependencies_do_not_include_removed_crates() {
    let cargo_toml_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let cargo_toml = fs::read_to_string(cargo_toml_path).expect("Cargo.toml should be readable");
    let dependencies = dependencies_section(&cargo_toml);

    assert!(!dependencies.contains("cli-clipboard"));
    assert!(!dependencies.contains("sqlx"));
    assert!(!dependencies.contains("chrono"));
}
