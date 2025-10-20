use regex_lite::Regex;
use std::borrow::Cow;
use std::path::Path;

lazy_static::lazy_static! {
    /// Regular expression that matches source file citations such as:
    ///
    /// ```text
    /// 【F:src/main.rs†L10-L20】
    /// ```
    ///
    /// Capture groups:
    /// 1. file path (anything except the dagger `†` symbol)
    /// 2. start line number (digits)
    /// 3. optional end line (digits or `?`)
    pub(crate) static ref CITATION_REGEX: Regex = Regex::new(
        r"【F:([^†]+)†L(\d+)(?:-L(\d+|\?))?】"
    ).expect("failed to compile citation regex");
}

/// Rewrite file citations with a URI scheme for clickable links.
///
/// Transforms citations like `【F:/src/main.rs†L42】` into markdown links
/// like `[/src/main.rs:42](vscode://file/src/main.rs:42)`.
pub(crate) fn rewrite_file_citations_with_scheme<'a>(
    src: &'a str,
    scheme_opt: Option<&str>,
    cwd: &Path,
) -> Cow<'a, str> {
    let scheme: &str = match scheme_opt {
        Some(s) => s,
        None => return Cow::Borrowed(src),
    };

    CITATION_REGEX.replace_all(src, |caps: &regex_lite::Captures<'_>| {
        let file = &caps[1];
        let start_line = &caps[2];

        // Resolve the path against `cwd` when it is relative.
        let absolute_path = {
            let p = Path::new(file);
            let absolute_path = if p.is_absolute() {
                path_clean::clean(p)
            } else {
                path_clean::clean(cwd.join(p))
            };
            // VS Code expects forward slashes even on Windows because URIs use
            // `/` as the path separator.
            absolute_path.to_string_lossy().replace('\\', "/")
        };

        // Render as a normal markdown link so the downstream renderer emits
        // the hyperlink escape sequence (when supported by the terminal).
        //
        // In practice, sometimes multiple citations for the same file, but with a
        // different line number, are shown sequentially, so we:
        // - include the line number in the label to disambiguate them
        // - add a space after the link to make it easier to read
        format!("[{file}:{start_line}]({scheme}://file{absolute_path}:{start_line}) ")
    })
}
