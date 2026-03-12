use crate::app;
use crate::app::commands::ReviewOptions;
use crate::app::App;
use color_eyre::Result;
use ratatui::crossterm::event::KeyEvent;

impl App {
    pub(crate) fn dispatch_panel_key_from_runtime(&mut self, key: KeyEvent) -> bool {
        self.handle_panel_dispatch_key(&key)
    }

    pub(crate) fn try_handle_survey_number_input(&mut self, c: char) -> bool {
        let potential_input = format!("{}{}", self.input, c);
        if let Some(is_dismiss) = self.survey.check_number_input(&potential_input) {
            self.input.clear();
            self.reset_cursor();
            self.input_modified = false;

            self.survey.dismiss();
            if !is_dismiss {
                self.survey.show_thank_you();
            }
            return true;
        }

        false
    }

    pub(crate) fn get_history_file_path() -> Result<std::path::PathBuf> {
        let cwd = std::env::current_dir()?;
        app::persistence::history::history_file_path_for_cwd(&cwd)
    }

    pub(crate) fn load_history(history_file: &std::path::Path) -> Vec<String> {
        app::persistence::history::load_history(history_file)
    }

    pub(crate) fn build_review_prompt(&self, options: &ReviewOptions, context: &str) -> String {
        let mut prompt = String::new();
        prompt.push_str("Please review the following code changes:\n\n");
        prompt.push_str(context);

        prompt.push_str("\n## Review Instructions\n\n");
        prompt.push_str("Please analyze the changes and provide:\n");
        prompt.push_str("1. **Summary**: Brief overview of what changed\n");
        prompt.push_str("2. **Potential Issues**: Bugs, security concerns, performance issues\n");
        prompt.push_str("3. **Code Quality**: Style, readability, maintainability\n");
        prompt.push_str("4. **Suggestions**: Improvements or alternative approaches\n");

        if options.no_tool {
            prompt.push_str(
                "\n**IMPORTANT**: Provide your review based solely on the diff shown above. ",
            );
            prompt.push_str("Do NOT use any tools to explore the codebase further. ");
            prompt.push_str("Generate your review directly from the provided context.\n");
        } else {
            prompt.push_str("\n**Note**: You have access to read-only tools. ");
            prompt.push_str(
                "Feel free to explore the codebase further if needed to understand the context better. ",
            );
            prompt.push_str(
                "You can read files, search code, run tests, or execute build commands to verify the changes.\n",
            );
        }

        prompt
    }
}
