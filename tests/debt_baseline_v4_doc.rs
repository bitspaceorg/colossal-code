use std::fs;
use std::path::Path;

fn debt_baseline_v4_doc_contents() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = repo_root.join("docs/debt/baseline-v4.md");
    fs::read_to_string(path).expect("debt baseline v4 doc should be readable")
}

#[test]
fn debt_baseline_v4_doc_exists_with_required_sections() {
    let doc = debt_baseline_v4_doc_contents();

    for required in [
        "# Debt Baseline V4 Report",
        "## `cargo check --all-targets` warnings by crate",
        "## `cargo clippy --all-targets --no-deps` top findings",
        "## `src/main.rs` LOC",
        "## Reproduction",
        "`cocode`",
        "`agent_core`",
        "Total warnings: **261**",
        "`src/main.rs`: **11,682 LOC**",
    ] {
        assert!(
            doc.contains(required),
            "debt baseline v4 doc should include section or token: {required}"
        );
    }
}
