use std::fs;
use std::path::Path;

fn debt_baseline_doc_contents() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = repo_root.join("docs/debt/baseline.md");
    fs::read_to_string(path).expect("debt baseline doc should be readable")
}

#[test]
fn debt_baseline_doc_exists_with_required_sections() {
    let doc = debt_baseline_doc_contents();

    for required in [
        "# Debt Baseline Report",
        "## `cargo check --all-targets` warnings by crate",
        "## `cargo clippy --all-targets --no-deps` top findings",
        "## `src/main.rs` LOC",
        "## Reproduction",
        "`cocode`",
        "`agent_core`",
    ] {
        assert!(
            doc.contains(required),
            "debt baseline doc should include section: {required}"
        );
    }
}
