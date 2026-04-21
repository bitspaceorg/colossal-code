use agent_protocol::types::spec::{SpecSheet, SpecStep, StepStatus, TaskSummary};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationOutcome {
    Verified,
    NeedsRevision,
    Failed,
}

#[derive(Debug, Deserialize)]
pub struct VerificationToolPayload {
    pub status: String,
    #[serde(default)]
    pub feedback: Option<String>,
    #[serde(default)]
    pub end_convo: bool,
}

#[derive(Clone)]
pub struct VerificationContext {
    pub spec_id: String,
    pub spec_title: String,
    pub step: SpecStep,
    pub prefix: String,
    pub summary: TaskSummary,
    pub tool_log: String,
    pub workspace_root: String,
}

#[derive(Clone)]
pub struct SummarizerContext {
    pub spec_id: String,
    pub spec_title: String,
    pub step: SpecStep,
    pub prefix: String,
    pub summary: TaskSummary,
    pub tool_log: String,
    pub workspace_root: String,
}

pub enum StepDisposition {
    Retry,
    Success(TaskSummary),
    Fail(TaskSummary),
}

pub struct VerifierChain {
    verifiers: Vec<Box<dyn Verifier>>,
}

impl VerifierChain {
    pub fn new(verifiers: Vec<Box<dyn Verifier>>) -> Self {
        Self { verifiers }
    }

    pub fn default_chain() -> Self {
        Self::new(vec![
            Box::new(CommandVerifier::default()),
            Box::new(LintVerifier::default()),
            Box::new(PolicyVerifier::default()),
        ])
    }

    pub async fn run(
        &self,
        summary: &TaskSummary,
    ) -> std::result::Result<(), super::FeedbackEntry> {
        for verifier in &self.verifiers {
            if let Err(feedback) = verifier.verify(summary).await {
                return Err(feedback);
            }
        }
        Ok(())
    }

    pub fn map_verification_outcome(status: &str) -> VerificationOutcome {
        match status.trim().to_ascii_lowercase().as_str() {
            "verified" => VerificationOutcome::Verified,
            "needs_revision" | "needs-revision" | "revision" | "retry" => {
                VerificationOutcome::NeedsRevision
            }
            _ => VerificationOutcome::Failed,
        }
    }

    pub fn extract_verification_payload(task: &super::Task) -> Result<VerificationToolPayload> {
        let tool_log = task
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.extra.get("toolLog"))
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow!("Verifier did not record tool calls"))?;

        let entry = tool_log
            .iter()
            .rev()
            .find(|entry| {
                entry.get("name").and_then(|value| value.as_str()) == Some("submit_verification")
            })
            .ok_or_else(|| anyhow!("Verifier must call submit_verification"))?;

        let arguments = entry
            .get("arguments")
            .and_then(|value| value.as_str())
            .unwrap_or("{}");
        serde_json::from_str::<VerificationToolPayload>(arguments)
            .or_else(|_| serde_yaml::from_str::<VerificationToolPayload>(arguments))
            .context("Failed to parse submit_verification payload")
    }
}

impl Default for VerifierChain {
    fn default() -> Self {
        Self::default_chain()
    }
}

impl From<Vec<Box<dyn Verifier>>> for VerifierChain {
    fn from(value: Vec<Box<dyn Verifier>>) -> Self {
        Self::new(value)
    }
}

#[async_trait]
pub trait Verifier: Send + Sync {
    async fn verify(&self, summary: &TaskSummary) -> std::result::Result<(), super::FeedbackEntry>;
}

pub fn build_summarizer_spec(context: &SummarizerContext) -> SpecSheet {
    SpecSheet {
        id: format!("{}::summary", context.spec_id),
        title: format!("Summary for {}", context.spec_title),
        description: "Summarize implementation for verification".to_string(),
        steps: vec![build_summarizer_step(context)],
        created_by: "summarizer".to_string(),
        created_at: Utc::now(),
        metadata: Value::Null,
    }
}

pub fn build_summarizer_step(context: &SummarizerContext) -> SpecStep {
    SpecStep {
        index: "summary".to_string(),
        title: format!("Summarize – {}", context.step.title),
        instructions: build_summarizer_instructions(context),
        acceptance_criteria: vec![
            "Provide a concise, verifier-ready summary".to_string(),
            "Mention any tests run and files touched if known".to_string(),
        ],
        required_tools: vec![],
        constraints: vec![
            "Do not modify files during summarization".to_string(),
            "Use tools only for inspection if needed".to_string(),
        ],
        dependencies: vec![],
        is_parallel: false,
        requires_verification: false,
        max_parallelism: None,
        status: StepStatus::Pending,
        sub_spec: None,
        completed_at: None,
    }
}

fn build_summarizer_instructions(context: &SummarizerContext) -> String {
    let acceptance = if context.step.acceptance_criteria.is_empty() {
        "(no acceptance criteria provided)".to_string()
    } else {
        context
            .step
            .acceptance_criteria
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let constraints = if context.step.constraints.is_empty() {
        "(no additional constraints)".to_string()
    } else {
        context
            .step
            .constraints
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## Role\nYou are summarizing spec \"{}\" step {} – {} for verification.\n\n### Workspace Root\n{}\n\n### Step Instructions\n{}\n\n### Acceptance Criteria\n{}\n\n### Constraints\n{}\n\n### Implementor Summary\n{}\n\n### Tool Calls Used\n{}\n\nProduce a concise summary that a verifier can use to validate the changes. Focus on the key modifications, commands run, and artifacts touched.",
        context.spec_title,
        context.step.index,
        context.step.title,
        context.workspace_root,
        context.step.instructions,
        acceptance,
        constraints,
        context.summary.summary_text,
        context.tool_log,
    )
}

pub fn build_verifier_spec(context: &VerificationContext) -> SpecSheet {
    SpecSheet {
        id: format!("{}::verifier", context.spec_id),
        title: format!("Verification for {}", context.spec_title),
        description: "Verification step".to_string(),
        steps: vec![build_verifier_step(context)],
        created_by: "verifier".to_string(),
        created_at: Utc::now(),
        metadata: Value::Null,
    }
}

pub fn build_verifier_step(context: &VerificationContext) -> SpecStep {
    SpecStep {
        index: "1".to_string(),
        title: format!("Verify – {}", context.step.title),
        instructions: build_verifier_instructions(context),
        acceptance_criteria: vec![
            "Inspect the implementation using tools and run relevant checks".to_string(),
            "Call submit_verification with the final outcome".to_string(),
        ],
        required_tools: vec!["submit_verification".to_string()],
        constraints: vec![
            "Do not modify files during verification".to_string(),
            "Use tools to read files and run commands as needed".to_string(),
        ],
        dependencies: vec![],
        is_parallel: false,
        requires_verification: false,
        max_parallelism: None,
        status: StepStatus::Pending,
        sub_spec: None,
        completed_at: None,
    }
}

fn build_verifier_instructions(context: &VerificationContext) -> String {
    let acceptance = if context.step.acceptance_criteria.is_empty() {
        "(no acceptance criteria provided)".to_string()
    } else {
        context
            .step
            .acceptance_criteria
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let constraints = if context.step.constraints.is_empty() {
        "(no additional constraints)".to_string()
    } else {
        context
            .step
            .constraints
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## Role\nYou are verifying spec \"{}\" step {} – {}.\n\n### Workspace Root\n{}\n\n### Step Instructions\n{}\n\n### Acceptance Criteria\n{}\n\n### Constraints\n{}\n\n### Implementor Summary\n{}\n\n### Tool Calls Used\n{}\n\nUse tools to inspect the codebase, run any relevant checks (tests, builds, linters), and confirm the implementation matches the instructions. Do not modify files.\n\nWhen verification is complete, call submit_verification with status verified, needs_revision, or failed, include feedback, and set end_convo to true.",
        context.spec_title,
        context.step.index,
        context.step.title,
        context.workspace_root,
        context.step.instructions,
        acceptance,
        constraints,
        context.summary.summary_text,
        context.tool_log,
    )
}

#[derive(Default)]
pub struct CommandVerifier;

#[async_trait]
impl Verifier for CommandVerifier {
    async fn verify(
        &self,
        _summary: &TaskSummary,
    ) -> std::result::Result<(), super::FeedbackEntry> {
        Ok(())
    }
}

#[derive(Default)]
pub struct LintVerifier;

#[async_trait]
impl Verifier for LintVerifier {
    async fn verify(
        &self,
        _summary: &TaskSummary,
    ) -> std::result::Result<(), super::FeedbackEntry> {
        Ok(())
    }
}

#[derive(Default)]
pub struct PolicyVerifier;

#[async_trait]
impl Verifier for PolicyVerifier {
    async fn verify(
        &self,
        _summary: &TaskSummary,
    ) -> std::result::Result<(), super::FeedbackEntry> {
        Ok(())
    }
}

pub fn format_tool_log(task: &super::Task) -> String {
    task.metadata
        .as_ref()
        .and_then(|metadata| metadata.extra.get("toolLog"))
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let name = entry.get("name").and_then(|v| v.as_str())?;
                    let args = entry
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    Some(format!("- {} {}", name, args))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "(no tool calls recorded)".to_string())
}
