use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub type TaskId = String;
pub type AgentId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpecSheet {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub steps: Vec<SpecStep>,
    pub created_by: AgentId,
    pub created_at: DateTime<Utc>,
    pub metadata: Value,
}

impl SpecSheet {
    pub fn validate(&self) -> Result<(), SpecValidationError> {
        if self.steps.is_empty() {
            return Err(SpecValidationError::EmptySteps);
        }

        let mut known_indexes = Vec::with_capacity(self.steps.len());
        for (idx, step) in self.steps.iter().enumerate() {
            let expected_index = (idx + 1).to_string();
            if step.index != expected_index {
                return Err(SpecValidationError::InvalidIndex {
                    expected: expected_index,
                    found: step.index.clone(),
                });
            }
            if let Some(max_parallelism) = step.max_parallelism {
                if max_parallelism == 0 {
                    return Err(SpecValidationError::InvalidParallelism {
                        step: step.index.clone(),
                        value: max_parallelism,
                    });
                }
            }
            if let Some(sub_spec) = &step.sub_spec {
                sub_spec.validate()?;
            }
            known_indexes.push(step.index.clone());
        }

        let valid_indexes: HashSet<&str> = known_indexes.iter().map(|idx| idx.as_str()).collect();
        for step in &self.steps {
            for dependency in &step.dependencies {
                if !valid_indexes.contains(dependency.as_str()) {
                    return Err(SpecValidationError::UnknownDependency {
                        step: step.index.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpecStep {
    pub index: String,
    pub title: String,
    pub instructions: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub is_parallel: bool,
    #[serde(default)]
    pub requires_verification: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallelism: Option<usize>,
    #[serde(default)]
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_spec: Option<Box<SpecSheet>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl Default for StepStatus {
    fn default() -> Self {
        StepStatus::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpecStepRef {
    pub index: String,
    pub instructions: String,
    pub spec_id: TaskId,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub task_id: TaskId,
    pub step_index: String,
    pub summary_text: String,
    #[serde(default)]
    pub artifacts_touched: Vec<String>,
    #[serde(default)]
    pub tests_run: Vec<TestRun>,
    pub verification: TaskVerification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeMetadata>,
}

/// Worktree metadata recorded with task summaries so downstream agents
/// know which branch/path contains the step's changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeMetadata {
    pub branch: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub name: String,
    pub result: TestResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestResult {
    Pass,
    Fail,
    Skip,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskVerification {
    pub status: VerificationStatus,
    #[serde(default)]
    pub feedback: Vec<FeedbackEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Pending,
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEntry {
    pub author: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum SpecValidationError {
    #[error("spec sheet must include at least one step")]
    EmptySteps,
    #[error("invalid step index: expected {expected}, found {found}")]
    InvalidIndex { expected: String, found: String },
    #[error("step {step} references unknown dependency {dependency}")]
    UnknownDependency { step: String, dependency: String },
    #[error("step {step} has invalid max_parallelism value {value} (must be at least 1)")]
    InvalidParallelism { step: String, value: usize },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use serde_json::json;

    fn build_step(index: usize) -> SpecStep {
        SpecStep {
            index: index.to_string(),
            title: format!("Step {index}"),
            instructions: "Do the thing".to_string(),
            acceptance_criteria: vec![],
            required_tools: vec![],
            constraints: vec![],
            dependencies: vec![],
            is_parallel: false,
            requires_verification: false,
            max_parallelism: None,
            status: StepStatus::Pending,
            sub_spec: None,
            completed_at: None,
        }
    }

    fn build_spec(step_count: usize) -> SpecSheet {
        let steps = (1..=step_count).map(build_step).collect();
        SpecSheet {
            id: "task-123".to_string(),
            title: "Spec".to_string(),
            description: "desc".to_string(),
            steps,
            created_by: "agent-1".to_string(),
            created_at: Utc::now(),
            metadata: json!({}),
        }
    }

    #[test]
    fn validates_basic_spec() {
        let spec = build_spec(2);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn spec_sheet_round_trip_with_nested_sub_spec() {
        let child = SpecSheet {
            id: "sub-spec".to_string(),
            title: "Child".to_string(),
            description: "Sub step".to_string(),
            steps: vec![SpecStep {
                index: "1".to_string(),
                title: "Nested".to_string(),
                instructions: "Do nested work".to_string(),
                acceptance_criteria: vec!["All green".to_string()],
                required_tools: vec![],
                constraints: vec![],
                dependencies: vec![],
                is_parallel: false,
                requires_verification: false,
                max_parallelism: None,
                status: StepStatus::InProgress,
                sub_spec: None,
                completed_at: None,
            }],
            created_by: "agent-child".to_string(),
            created_at: Utc::now(),
            metadata: json!({ "nested": true }),
        };

        let root = SpecSheet {
            id: "root".to_string(),
            title: "Root".to_string(),
            description: "Top level spec".to_string(),
            steps: vec![SpecStep {
                index: "1".to_string(),
                title: "Root step".to_string(),
                instructions: "Coordinate".to_string(),
                acceptance_criteria: vec![],
                required_tools: vec!["tool-a".to_string()],
                constraints: vec![],
                dependencies: vec![],
                is_parallel: false,
                requires_verification: false,
                max_parallelism: None,
                status: StepStatus::Pending,
                sub_spec: Some(Box::new(child.clone())),
                completed_at: None,
            }],
            created_by: "agent-root".to_string(),
            created_at: Utc::now(),
            metadata: json!({ "priority": "high" }),
        };

        let value = serde_json::to_value(&root).unwrap();
        let round_trip: SpecSheet = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, root);
    }

    #[test]
    fn task_summary_round_trip() {
        let summary = TaskSummary {
            task_id: "task-42".to_string(),
            step_index: "1".to_string(),
            summary_text: "Completed setup".to_string(),
            artifacts_touched: vec!["README.md".to_string()],
            tests_run: vec![TestRun {
                name: "unit".to_string(),
                result: TestResult::Pass,
                logs_path: Some(PathBuf::from("logs/unit.log")),
                duration_ms: Some(1200),
            }],
            verification: TaskVerification {
                status: VerificationStatus::Passed,
                feedback: vec![FeedbackEntry {
                    author: "QA".to_string(),
                    message: "Looks good".to_string(),
                    timestamp: Utc::now(),
                }],
            },
            worktree: None,
        };

        let value = serde_json::to_value(&summary).unwrap();
        let round_trip: TaskSummary = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, summary);
    }

    #[test]
    fn rejects_empty_steps() {
        let spec = SpecSheet {
            steps: vec![],
            ..build_spec(1)
        };
        assert!(matches!(spec.validate(), Err(SpecValidationError::EmptySteps)));
    }

    #[test]
    fn rejects_missing_step_sequence() {
        let mut spec = build_spec(2);
        spec.steps[1].index = "3".to_string();
        assert!(matches!(
            spec.validate(),
            Err(SpecValidationError::InvalidIndex { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_indexes() {
        let mut spec = build_spec(2);
        spec.steps[1].index = "1".to_string();
        assert!(matches!(
            spec.validate(),
            Err(SpecValidationError::InvalidIndex { .. })
        ));
    }

    #[test]
    fn rejects_bad_dependencies() {
        let mut spec = build_spec(2);
        spec.steps[1].dependencies.push("3".to_string());
        assert!(matches!(
            spec.validate(),
            Err(SpecValidationError::UnknownDependency { .. })
        ));
    }
}
