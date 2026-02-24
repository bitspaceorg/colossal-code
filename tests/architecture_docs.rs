use std::fs;
use std::path::Path;

fn architecture_doc_contents() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = repo_root.join("docs/ARCHITECTURE_MODULE_BOUNDARIES.md");
    fs::read_to_string(path).expect("architecture module boundaries doc should be readable")
}

#[test]
fn architecture_doc_exists_and_covers_new_module_boundaries() {
    let doc = architecture_doc_contents();

    for required in [
        "# Architecture: Module Boundaries",
        "### `src/main.rs`",
        "### `src/commands/`",
        "### `src/persistence/`",
        "### `src/ui/`",
        "### `src/spec_cli.rs`",
        "### `src/model_context.rs`",
        "## Dependency Rules",
    ] {
        assert!(
            doc.contains(required),
            "architecture doc should include section: {required}"
        );
    }
}
