use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::time::Instant;
use rand::Rng;

/// Survey question with customizable options
#[derive(Clone)]
pub struct SurveyQuestion {
    /// The question text
    pub question: String,
    /// Whether the question is optional (can be dismissed)
    pub optional: bool,
    /// The answer options (index 0 will always be shown last as dismiss/cancel)
    /// Options 1..n will be shown in order
    pub options: Vec<String>,
}

impl SurveyQuestion {
    /// Create a new survey question
    pub fn new(question: String, optional: bool, options: Vec<String>) -> Self {
        Self {
            question,
            optional,
            options,
        }
    }
}

/// Survey state manager
pub struct Survey {
    /// Current active question (if any)
    active_question: Option<SurveyQuestion>,
    /// Number of messages after which survey can appear
    pub messages_threshold: usize,
    /// Probability of survey appearing (0.0 to 1.0)
    pub appearance_probability: f64,
    /// Counter for messages since last survey
    message_count: usize,
    /// Random seed for probability (using timestamp-based pseudo-random)
    last_check: Option<Instant>,
    /// Thank you message display (with timestamp for auto-dismiss)
    thank_you_message: Option<Instant>,
}

impl Survey {
    /// Create a new survey manager
    pub fn new(messages_threshold: usize, appearance_probability: f64) -> Self {
        Self {
            active_question: None,
            messages_threshold,
            appearance_probability: appearance_probability.clamp(0.0, 1.0),
            message_count: 0,
            last_check: None,
            thank_you_message: None,
        }
    }

    /// Check if survey is currently active
    pub fn is_active(&self) -> bool {
        self.active_question.is_some()
    }

    /// Check if thank you message should be displayed
    pub fn has_thank_you(&self) -> bool {
        self.thank_you_message.is_some()
    }

    /// Get the height needed for the current survey or thank you message (0 if none)
    pub fn get_height(&self) -> u16 {
        if self.active_question.is_some() {
            2 // Question line + options line
        } else if self.thank_you_message.is_some() {
            2 // Thank you message (2 lines)
        } else {
            0
        }
    }

    /// Increment message count and potentially trigger a survey
    pub fn on_message_sent(&mut self, question: Option<SurveyQuestion>) {
        // Don't count messages if survey is already active
        if self.is_active() {
            return;
        }

        self.message_count += 1;

        // Check if we've reached the threshold
        if self.message_count >= self.messages_threshold {
            // Use proper random number generation
            let mut rng = rand::thread_rng();
            let random_value: f64 = rng.gen_range(0.0..1.0); // Generates a random f64 between 0.0 and 1.0

            self.last_check = Some(Instant::now());

            // Check if we should show the survey
            if random_value < self.appearance_probability {
                if let Some(q) = question {
                    self.active_question = Some(q);
                    self.message_count = 0; // Reset counter
                }
            } else {
                // Reset counter even if we don't show survey
                self.message_count = 0;
            }
        }
    }

    /// Handle a key press for the survey (returns the choice index if valid)
    /// Does NOT dismiss the survey - caller should do that after handling the choice
    pub fn handle_key(&self, key_char: char) -> Option<usize> {
        if let Some(question) = &self.active_question {
            // Try to parse the key as a number
            if let Some(digit) = key_char.to_digit(10) {
                let choice = digit as usize;

                // 0 is always dismiss (if optional) or cancel
                if choice == 0 {
                    if question.optional {
                        return Some(0);
                    }
                }
                // Check if choice is valid (1 to options.len() - 1)
                else if choice > 0 && choice < question.options.len() {
                    return Some(choice);
                }
            }
        }
        None
    }

    /// Dismiss the current survey
    pub fn dismiss(&mut self) {
        self.active_question = None;
    }

    /// Render the survey lines or thank you message
    pub fn render(&self) -> Vec<Line<'static>> {
        if let Some(question) = &self.active_question {
            let mut lines = Vec::new();

            // First line: bullet + question + (optional) tag
            let mut question_spans = vec![
                Span::styled("● ", Style::default().fg(Color::Cyan)),
                Span::raw(question.question.clone()),
            ];
            if question.optional {
                question_spans.push(Span::styled(" (optional)", Style::default().fg(Color::DarkGray)));
            }
            lines.push(Line::from(question_spans));

            // Second line: options
            let mut option_spans = vec![Span::raw("  ")]; // Indent

            // Show options 1..n
            for i in 1..question.options.len() {
                if i > 1 {
                    option_spans.push(Span::raw("   "));
                }
                option_spans.push(Span::styled(
                    format!("{}: ", i),
                    Style::default().fg(Color::Yellow),
                ));
                option_spans.push(Span::raw(question.options[i].clone()));
            }

            // Show option 0 last (dismiss/cancel)
            if !question.options.is_empty() {
                option_spans.push(Span::raw("   "));
                option_spans.push(Span::styled(
                    "0: ",
                    Style::default().fg(Color::Yellow),
                ));
                option_spans.push(Span::raw(question.options[0].clone()));
            }

            lines.push(Line::from(option_spans));

            lines
        } else if self.thank_you_message.is_some() {
            // Show thank you message
            vec![
                Line::from(Span::styled(
                    "Thanks for making Nite better",
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(Span::styled(
                    "(use /feedback to give suggestions or bug reports)",
                    Style::default().fg(Color::DarkGray),
                )),
            ]
        } else {
            Vec::new()
        }
    }

    /// Force show a survey question
    pub fn show_question(&mut self, question: SurveyQuestion) {
        self.active_question = Some(question);
    }

    /// Get the text of the selected option
    pub fn get_option_text(&self, choice: usize) -> String {
        if let Some(question) = &self.active_question {
            if choice < question.options.len() {
                return question.options[choice].clone();
            }
        }
        String::new()
    }

    /// Check if the input is a valid number choice for the survey
    /// Returns Some(is_dismiss) if it's a valid choice, None otherwise
    pub fn check_number_input(&self, input: &str) -> Option<bool> {
        if let Some(question) = &self.active_question {
            // Try to parse input as a number
            if let Ok(choice) = input.parse::<usize>() {
                // Check if it's a valid choice (0 to options.len() - 1)
                if choice < question.options.len() {
                    // 0 is always the dismiss option
                    return Some(choice == 0);
                }
            }
        }
        None
    }

    /// Show the thank you message (auto-dismisses after 3 seconds)
    pub fn show_thank_you(&mut self) {
        self.thank_you_message = Some(Instant::now());
    }

    /// Update the survey state (check for auto-dismiss of thank you message)
    pub fn update(&mut self) {
        if let Some(shown_at) = self.thank_you_message {
            if shown_at.elapsed().as_secs() >= 1 {
                self.thank_you_message = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_survey_creation() {
        let survey = Survey::new(10, 0.33);
        assert_eq!(survey.messages_threshold, 10);
        assert_eq!(survey.appearance_probability, 0.33);
        assert!(!survey.is_active());
    }

    #[test]
    fn test_survey_height() {
        let mut survey = Survey::new(10, 1.0);
        assert_eq!(survey.get_height(), 0);

        let question = SurveyQuestion::new(
            "How is it?".to_string(),
            true,
            vec!["Dismiss".to_string(), "Bad".to_string(), "Good".to_string()],
        );
        survey.show_question(question);
        assert_eq!(survey.get_height(), 2);
    }

    #[test]
    fn test_survey_dismiss() {
        let mut survey = Survey::new(10, 1.0);
        let question = SurveyQuestion::new(
            "Test?".to_string(),
            true,
            vec!["Dismiss".to_string(), "Yes".to_string()],
        );
        survey.show_question(question);
        assert!(survey.is_active());

        survey.dismiss();
        assert!(!survey.is_active());
    }
}
