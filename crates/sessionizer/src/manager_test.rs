#[cfg(test)]
mod tests {
    use regex::Regex;

    fn clean_ansi_codes(output: &str) -> String {
        // Updated regex to handle sequences with spaces like \x1b[6 q (cursor style)
        let ansi_regex = Regex::new(r"\x1b\[[0-9;]*[ ]?[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b\][^\x1b]*\x1b\\|\x1bk[^\x1b]*\x1b\\").unwrap();
        let without_ansi = ansi_regex.replace_all(output, "");

        // Also remove other control characters except newlines
        let control_regex = Regex::new(r"[\x00-\x08\x0B-\x1F\x7F]").unwrap();
        control_regex.replace_all(&without_ansi, "").to_string()
    }

    #[test]
    fn test_basic_ansi_codes() {
        let input = "\x1b[31mRed\x1b[0m";
        let output = clean_ansi_codes(input);
        assert_eq!(output, "Red", "Should remove basic ANSI color codes");
    }

    #[test]
    fn test_cursor_style_codes() {
        // Test the problematic \x1b[6 q sequence
        let input = "\x1b[6 qHello";
        let output = clean_ansi_codes(input);
        assert_eq!(
            output, "Hello",
            "Should remove cursor style codes with space"
        );
    }

    #[test]
    fn test_cursor_codes_between_chars() {
        // Simulate the pattern from user's output: p[6 qy[6 qt...
        let input = "p\x1b[6 qy\x1b[6 qt\x1b[6 qh\x1b[6 qo\x1b[6 qn";
        let output = clean_ansi_codes(input);
        assert_eq!(
            output, "python",
            "Should remove cursor codes between characters"
        );
    }

    #[test]
    fn test_realistic_python_output() {
        // Simulate full python command with cursor codes
        let input = "p\x1b[6 qy\x1b[6 qt\x1b[6 qh\x1b[6 qo\x1b[6 qn\x1b[6 q \x1b[6 q.\x1b[6 q/\x1b[6 qn\x1b[6 qu\x1b[6 qm\x1b[6 qe\x1b[6 qr\x1b[6 qi\x1b[6 qc\x1b[6 qa\x1b[6 ql\x1b[6 q_\x1b[6 qo\x1b[6 qp\x1b[6 qs\x1b[6 q.\x1b[6 qp\x1b[6 qy";
        let output = clean_ansi_codes(input);
        assert_eq!(
            output, "python ./numerical_ops.py",
            "Should clean python command"
        );
    }

    #[test]
    fn test_clean_output() {
        let input = "5\n";
        let output = clean_ansi_codes(input);
        assert_eq!(output, "5\n", "Should preserve clean output");
    }

    #[test]
    fn test_mixed_ansi_codes() {
        let input = "\x1b[1;31mBold Red\x1b[0m\x1b[6 q Text";
        let output = clean_ansi_codes(input);
        assert_eq!(output, "Bold Red Text", "Should remove mixed ANSI codes");
    }
}
