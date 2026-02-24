use crate::commands::{ReviewOptions, ReviewType};
use anyhow::{Result, anyhow};
use std::ffi::{OsStr, OsString};
use tokio::process::Command;

async fn run_git<I, S>(args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect();

    let output = Command::new("git").args(&args).output().await?;

    if !output.status.success() {
        let rendered_args = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        let code = output
            .status
            .code()
            .map_or_else(|| "unknown".to_string(), |value| value.to_string());

        return Err(anyhow!(
            "git {} failed with exit code {}: {}",
            rendered_args,
            code,
            detail
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) async fn build_review_git_context(options: &ReviewOptions) -> Result<String> {
    let mut context = String::new();

    let current_branch = run_git(["branch", "--show-current"]).await?;
    context.push_str(&format!("Current branch: {}\n\n", current_branch.trim()));

    let status = run_git(["status", "--short"]).await?;
    if !status.is_empty() {
        context.push_str(&format!("Git status:\n```\n{}```\n\n", status));
    }

    match options.review_type {
        ReviewType::All => {
            if let Some(ref base) = options.base_branch {
                let diff = run_git(vec!["diff".to_string(), format!("{}...HEAD", base)]).await?;
                context.push_str(&format!(
                    "Changes compared to '{}':\n```diff\n{}```\n\n",
                    base, diff
                ));

                let log = run_git(vec![
                    "log".to_string(),
                    "--oneline".to_string(),
                    format!("{}..HEAD", base),
                ])
                .await?;
                if !log.is_empty() {
                    context.push_str(&format!("Commits since '{}':\n```\n{}```\n\n", base, log));
                }
            } else if let Some(ref commit) = options.base_commit {
                let diff = run_git(vec!["diff".to_string(), format!("{}..HEAD", commit)]).await?;
                context.push_str(&format!(
                    "Changes since commit '{}':\n```diff\n{}```\n\n",
                    commit, diff
                ));

                let log = run_git(vec![
                    "log".to_string(),
                    "--oneline".to_string(),
                    format!("{}..HEAD", commit),
                ])
                .await?;
                if !log.is_empty() {
                    context.push_str(&format!("Commits since '{}':\n```\n{}```\n\n", commit, log));
                }
            } else {
                let diff = run_git(["diff", "HEAD"]).await?;
                if !diff.is_empty() {
                    context.push_str(&format!("Uncommitted changes:\n```diff\n{}```\n\n", diff));
                } else {
                    context.push_str("No uncommitted changes.\n\n");
                }
            }
        }
        ReviewType::Committed => {
            if let Some(ref base) = options.base_branch {
                let diff = run_git(vec!["diff".to_string(), format!("{}...HEAD", base)]).await?;
                context.push_str(&format!(
                    "Committed changes compared to '{}':\n```diff\n{}```\n\n",
                    base, diff
                ));

                let log = run_git(vec![
                    "log".to_string(),
                    "--oneline".to_string(),
                    format!("{}..HEAD", base),
                ])
                .await?;
                if !log.is_empty() {
                    context.push_str(&format!("Commits:\n```\n{}```\n\n", log));
                }
            } else if let Some(ref commit) = options.base_commit {
                let diff = run_git(vec!["diff".to_string(), format!("{}..HEAD", commit)]).await?;
                context.push_str(&format!(
                    "Committed changes since '{}':\n```diff\n{}```\n\n",
                    commit, diff
                ));

                let log = run_git(vec![
                    "log".to_string(),
                    "--oneline".to_string(),
                    format!("{}..HEAD", commit),
                ])
                .await?;
                if !log.is_empty() {
                    context.push_str(&format!("Commits:\n```\n{}```\n\n", log));
                }
            } else {
                let log = run_git(["log", "--oneline", "-10"]).await?;
                context.push_str(&format!("Recent commits:\n```\n{}```\n\n", log));

                let diff = run_git(["diff", "HEAD~1..HEAD"]).await?;
                if !diff.is_empty() {
                    context.push_str(&format!("Last commit diff:\n```diff\n{}```\n\n", diff));
                }
            }
        }
        ReviewType::Uncommitted => {
            let diff = run_git(["diff", "HEAD"]).await?;
            if !diff.is_empty() {
                context.push_str(&format!("Uncommitted changes:\n```diff\n{}```\n\n", diff));
            } else {
                context.push_str("No uncommitted changes.\n\n");
            }

            let staged = run_git(["diff", "--cached"]).await?;
            if !staged.is_empty() {
                context.push_str(&format!("Staged changes:\n```diff\n{}```\n\n", staged));
            }
        }
    }

    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::build_review_git_context;
    use crate::commands::{ReviewOptions, ReviewType};

    #[tokio::test]
    async fn review_context_contains_current_branch() {
        let context = build_review_git_context(&ReviewOptions::default())
            .await
            .expect("default review context should build");
        assert!(context.contains("Current branch:"));
    }

    #[tokio::test]
    async fn review_context_reports_git_failures_with_status_details() {
        let options = ReviewOptions {
            review_type: ReviewType::Committed,
            base_branch: Some("__branch_that_should_not_exist__".to_string()),
            base_commit: None,
            no_tool: false,
        };

        let err = build_review_git_context(&options)
            .await
            .expect_err("invalid base branch should fail");
        let message = err.to_string();

        assert!(message.contains("git diff __branch_that_should_not_exist__...HEAD failed"));
        assert!(message.contains("exit code"));
    }
}
