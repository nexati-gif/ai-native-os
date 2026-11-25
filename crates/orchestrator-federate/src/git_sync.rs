//! Git synchronization for patches and configurations

use std::path::Path;
use git2::{Repository, Signature, Oid, Commit, Tree, Index, BranchType};
use tokio::process::Command;
use orchestrator_common::{Error, Result, utils::run_command};
use orchestrator_common::types::{Proposal, ValidationResult, DeviceId};

/// Git sync for proposals and configurations
pub struct GitSync {
    repo: Repository,
    remote_name: String,
    branch: String,
}

impl GitSync {
    pub async fn new(remote: Option<&str>, branch: &str) -> Result<Self> {
        let repo_path = "/etc/orchestrator/federation";
        let repo = Repository::open(repo_path).or_else(|_| Repository::init(repo_path))?;

        let remote_name = remote.map(|r| {
            if repo.find_remote("origin").is_err() {
                repo.remote("origin", r).ok();
            }
            "origin".to_string()
        }).unwrap_or("origin".to_string());

        Ok(Self {
            repo,
            remote_name,
            branch: branch.to_string(),
        })
    }

    pub async fn commit_proposal(&mut self, proposal: &Proposal, validation: &ValidationResult) -> Result<()> {
        let mut index = self.repo.index()?;
        
        // Write proposal file
        let proposal_path = format!("proposals/{}.json", proposal.id);
        let proposal_json = serde_json::to_vec_pretty(&(proposal, validation))?;
        tokio::fs::write(&proposal_path, proposal_json).await?;
        index.add_path(Path::new(&proposal_path))?;

        // Write summary
        let summary_path = format!("proposals/{}-summary.md", proposal.id);
        let summary = self.generate_summary(proposal, validation);
        tokio::fs::write(&summary_path, summary).await?;
        index.add_path(Path::new(&summary_path))?;

        index.write()?;

        let tree_id = index.write_tree()?;
        let tree = self.repo.find_tree(tree_id)?;

        let sig = Signature::now("Orchestrator", "orchestrator@localhost")?;
        let parent = self.get_head_commit()?;
        
        self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("proposal: {}", proposal.description),
            &tree,
            &[&parent],
        )?;

        Ok(())
    }

    pub async fn push(&self) -> Result<()> {
        let mut remote = self.repo.find_remote(&self.remote_name)?;
        remote.push(&[format!("refs/heads/{}:refs/heads/{}", self.branch, self.branch)], None)?;
        Ok(())
    }

    pub async fn pull(&mut self) -> Result<()> {
        let mut remote = self.repo.find_remote(&self.remote_name)?;
        remote.fetch(&[&self.branch], None, None)?;
        
        let fetch_head = self.repo.find_reference("FETCH_HEAD")?;
        let fetch_commit = self.repo.reference_to_annotated_commit(&fetch_head)?;
        
        let analysis = self.repo.merge_analysis(&[&fetch_commit])?;
        
        if analysis.0.is_fast_forward() {
            let refname = format!("refs/heads/{}", self.branch);
            let mut ref_ = self.repo.find_reference(&refname)?;
            ref_.set_target(fetch_commit.id(), "Fast-forward")?;
            self.repo.checkout_head(None)?;
        }
        
        Ok(())
    }

    fn get_head_commit(&self) -> Result<Commit> {
        let head = self.repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit)
    }

    fn generate_summary(&self, proposal: &Proposal, validation: &ValidationResult) -> String {
        format!(r#"# Proposal Summary

**ID:** {}
**Type:** {:?}
**Description:** {}
**Status:** {}
**Risk:** {:?}

## Validation Results
- **Passed:** {}
- **Tests:** {} passed / {} total
- **Benchmarks:** {} passed / {} total

## Impact
{}
"#,
            proposal.id,
            proposal.kind,
            proposal.description,
            if validation.passed { "PASSED" } else { "FAILED" },
            proposal.risk_level,
            validation.passed,
            validation.test_results.iter().filter(|t| t.passed).count(),
            validation.test_results.len(),
            validation.benchmark_results.iter().filter(|b| b.passed).count(),
            validation.benchmark_results.len(),
            serde_json::to_string_pretty(&proposal.estimated_impact).unwrap_or_default()
        )
    }
}