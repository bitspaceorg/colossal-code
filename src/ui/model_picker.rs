use crate::ModelInfo;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

const LINES_PER_MODEL: usize = 3;

pub fn visible_model_bounds(
    list_height: usize,
    selected_index: usize,
    total_models: usize,
) -> (usize, usize) {
    let visible_models = (list_height / LINES_PER_MODEL).max(1);
    let scroll_offset = if selected_index >= visible_models {
        selected_index - visible_models + 1
    } else {
        0
    };
    let end_index = (scroll_offset + visible_models).min(total_models);
    (scroll_offset, end_index)
}

fn format_model_size(size_mb: f64) -> String {
    if size_mb >= 1024.0 {
        format!("{:.1}GB", size_mb / 1024.0)
    } else {
        format!("{:.0}MB", size_mb)
    }
}

pub fn primary_metadata_line<F>(model: &ModelInfo, format_compact_number: F) -> String
where
    F: Fn(usize) -> String,
{
    let mut metadata_parts = Vec::new();

    if let Some(ref arch) = model.architecture {
        if let Some(ref params) = model.parameter_count {
            metadata_parts.push(format!("{} {}", arch, params));
        } else {
            metadata_parts.push(arch.clone());
        }
    } else if let Some(ref params) = model.parameter_count {
        metadata_parts.push(params.clone());
    }

    metadata_parts.push(format_model_size(model.size_mb));

    if let Some(ref quant) = model.quantization {
        metadata_parts.push(quant.clone());
    }

    if let Some(ctx) = model.context_length {
        metadata_parts.push(format!("{} ctx", format_compact_number(ctx)));
    }

    metadata_parts.join(" · ")
}

pub fn secondary_metadata_line(model: &ModelInfo) -> Option<String> {
    let mut metadata_parts = Vec::new();

    if let Some(ref author) = model.author {
        metadata_parts.push(author.clone());
    }
    if let Some(ref version) = model.version {
        metadata_parts.push(format!("ver {}", version));
    }
    if let Some(ref hash) = model.file_hash {
        metadata_parts.push(format!("hash {}", hash));
    }

    if metadata_parts.is_empty() {
        None
    } else {
        Some(metadata_parts.join(" · "))
    }
}

pub fn model_list_item<F>(
    model: &ModelInfo,
    is_selected: bool,
    is_current: bool,
    format_compact_number: F,
) -> ListItem<'static>
where
    F: Fn(usize) -> String,
{
    let indicator = if is_selected { ">  " } else { "   " };
    let title_color = if is_selected {
        Color::Blue
    } else {
        Color::White
    };
    let prefix_color = if is_selected {
        Color::Blue
    } else {
        Color::White
    };
    let current_suffix = if is_current { " ✔" } else { "" };

    let title_line = Line::from(vec![
        Span::styled(indicator, Style::default().fg(prefix_color)),
        Span::styled(model.display_name.clone(), Style::default().fg(title_color)),
        Span::styled(current_suffix, Style::default().fg(Color::Green)),
    ]);

    let metadata_line1 = Line::from(vec![
        Span::raw("   "),
        Span::styled(
            primary_metadata_line(model, format_compact_number),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let mut lines = vec![title_line, metadata_line1];

    if let Some(metadata2) = secondary_metadata_line(model) {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                metadata2,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    ListItem::new(lines)
}

#[cfg(test)]
mod tests {
    use super::{primary_metadata_line, secondary_metadata_line, visible_model_bounds};
    use crate::ModelInfo;

    fn sample_model() -> ModelInfo {
        ModelInfo {
            filename: "test.gguf".to_string(),
            display_name: "test".to_string(),
            size_mb: 8192.0,
            quantization: Some("Q4_K_M".to_string()),
            architecture: Some("Llama".to_string()),
            parameter_count: Some("7B".to_string()),
            file_hash: Some("abc12345".to_string()),
            author: Some("meta".to_string()),
            version: Some("3.1".to_string()),
            context_length: Some(32768),
        }
    }

    #[test]
    fn visible_model_bounds_keeps_selected_visible() {
        assert_eq!(visible_model_bounds(9, 5, 10), (3, 6));
    }

    #[test]
    fn primary_metadata_includes_arch_size_quant_and_context() {
        let model = sample_model();
        let metadata = primary_metadata_line(&model, |n| format!("{}k", n / 1000));
        assert_eq!(metadata, "Llama 7B · 8.0GB · Q4_K_M · 32k ctx");
    }

    #[test]
    fn secondary_metadata_joins_optional_fields() {
        let model = sample_model();
        let metadata = secondary_metadata_line(&model);
        assert_eq!(metadata, Some("meta · ver 3.1 · hash abc12345".to_string()));
    }

    #[test]
    fn secondary_metadata_absent_when_no_fields_present() {
        let mut model = sample_model();
        model.author = None;
        model.version = None;
        model.file_hash = None;

        assert_eq!(secondary_metadata_line(&model), None);
    }
}
