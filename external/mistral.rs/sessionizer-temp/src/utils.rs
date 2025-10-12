use crate::types::StreamOutput;

pub fn truncate_output(output: Vec<u8>, max_bytes: usize) -> StreamOutput<Vec<u8>> {
    if output.len() <= max_bytes {
        StreamOutput {
            text: output,
            truncated_after_lines: None,
        }
    } else {
        StreamOutput {
            text: output.into_iter().take(max_bytes).collect(),
            truncated_after_lines: None,
        }
    }
}
