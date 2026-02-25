use std::fs;
use std::path::Path;

fn debt_pass_v2_doc_contents() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = repo_root.join("docs/debt/debt-pass-v2.md");
    fs::read_to_string(path).expect("debt pass v2 doc should be readable")
}

#[test]
fn debt_pass_v2_doc_exists_with_required_sections() {
    let doc = debt_pass_v2_doc_contents();

    for required in [
        "# Debt Pass V2 Summary",
        "## Before vs After",
        "## Current Top Clippy Findings",
        "## Remaining Debt Register",
        "## Reproduction",
        "`cargo check` warnings (total)",
        "`cargo clippy` warnings (total)",
        "P0",
        "P1",
        "P2",
    ] {
        assert!(
            doc.contains(required),
            "debt pass v2 doc should include section or token: {required}"
        );
    }
}
