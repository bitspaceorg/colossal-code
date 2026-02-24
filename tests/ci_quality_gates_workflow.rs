use std::fs;
use std::path::Path;

fn workflow_contents() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow_path = repo_root.join(".github/workflows/quality-gates.yml");
    fs::read_to_string(workflow_path).expect("quality-gates workflow should be readable")
}

#[test]
fn quality_gates_workflow_exists_with_required_checks() {
    let workflow = workflow_contents();

    assert!(workflow.contains("cargo fmt --all --check"));
    assert!(workflow.contains("cargo check --all-targets"));
    assert!(workflow.contains("cargo clippy --all-targets --no-deps -- -D warnings"));
    assert!(workflow.contains("cargo test --all-targets"));
}

#[test]
fn quality_gates_workflow_scopes_to_first_party_paths() {
    let workflow = workflow_contents();

    for path in [
        "\"src/**\"",
        "\"crates/**\"",
        "\"tests/**\"",
        "\"Cargo.toml\"",
        "\"Cargo.lock\"",
    ] {
        assert!(
            workflow.contains(path),
            "workflow should include first-party path filter: {path}"
        );
    }
}
