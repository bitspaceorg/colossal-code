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

#[test]
fn root_lockfile_is_not_ignored() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lockfile_path = repo_root.join("Cargo.lock");
    let gitignore_path = repo_root.join(".gitignore");

    assert!(
        lockfile_path.exists(),
        "Cargo.lock should exist at the repository root"
    );

    let gitignore = fs::read_to_string(gitignore_path).expect(".gitignore should be readable");
    let ignores_root_lockfile = gitignore
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .any(|line| line == "Cargo.lock" || line == "/Cargo.lock");

    assert!(
        !ignores_root_lockfile,
        ".gitignore should not ignore the repository root Cargo.lock"
    );
}
