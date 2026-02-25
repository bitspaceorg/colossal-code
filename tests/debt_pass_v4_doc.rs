use std::fs;
use std::path::Path;

fn debt_pass_v4_doc_contents() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = repo_root.join("docs/debt/debt-pass-v4.md");
    fs::read_to_string(path).expect("debt pass v4 doc should be readable")
}

#[test]
fn debt_pass_v4_doc_exists_with_required_sections() {
    let doc = debt_pass_v4_doc_contents();

    for required in [
        "# Debt Pass V4 Summary",
        "docs/debt/baseline-v4.md",
        "## Before vs After",
        "## Current Top Clippy Findings",
        "## Remaining Debt Register",
        "## Follow-up Plan",
        "## Reproduction",
        "`cargo check` warnings (total)",
        "`cargo clippy` warnings (total)",
        "P0",
        "P1",
        "P2",
        "261",
        "174",
    ] {
        assert!(
            doc.contains(required),
            "debt pass v4 doc should include section or token: {required}"
        );
    }
}
