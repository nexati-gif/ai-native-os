//! Statistical validator for proposal validation

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result, MetricsRecorder};
use orchestrator_common::types::{Proposal, SuccessCriteria, EstimatedImpact};
use statrs::statistics::{Statistics, Mean, Variance, StandardDeviation};
use statrs::distribution::{StudentsT, ContinuousCDF};

/// Validator for statistical validation of proposals
pub struct Validator {
    metrics: MetricsRecorder,
    config: ValidatorConfig,
    history: Arc<Mutex<ValidationHistory>>,
}

#[derive(Debug, Clone)]
pub struct ValidatorConfig {
    pub default_significance: f64,
    pub default_min_improvement: f64,
    pub bootstrap_samples: usize,
    pub min_samples_for_ttest: usize,
}

impl Default for ValidatorConfig {
    fn default() -> Self {
        Self {
            default_significance: 0.05,
            default_min_improvement: 1.0,
            bootstrap_samples: 10000,
            min_samples_for_ttest: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidationHistory {
    pub validations: Vec<ValidationRecord>,
    pub max_size: usize,
}

#[derive(Debug, Clone)]
pub struct ValidationRecord {
    pub proposal_id: String,
    pub proposal_kind: String,
    pub passed: bool,
    pub p_value: f64,
    pub effect_size: f64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Validator {
    pub fn new(config: ValidatorConfig) -> Self {
        Self {
            metrics: MetricsRecorder::new("validator"),
            config,
            history: Arc::new(Mutex::new(ValidationHistory {
                validations: Vec::new(),
                max_size: 1000,
            })),
        }
    }
    
    /// Validate proposal results against success criteria
    pub async fn validate(&self, proposal: &Proposal, test_results: &crate::sandbox::TestResults, canary_results: Option<&crate::sandbox::CanaryResults>) -> Result<ValidationResult> {
        info!("Validating proposal: {}", proposal.id);
        
        let mut checks = Vec::new();
        let mut all_passed = true;
        
        // 1. Validate unit tests
        let unit_check = self.validate_unit_tests(&test_results.unit_tests)?;
        all_passed &= unit_check.passed;
        checks.push(unit_check);
        
        // 2. Validate integration tests
        let integration_check = self.validate_integration_tests(&test_results.integration_tests)?;
        all_passed &= integration_check.passed;
        checks.push(integration_check);
        
        // 3. Validate benchmarks (statistical significance)
        let benchmark_checks = self.validate_benchmarks(&test_results.benchmarks, &proposal.test_plan.success_criteria).await?;
        for check in &benchmark_checks {
            all_passed &= check.passed;
            checks.push(check.clone());
        }
        
        // 4. Validate canary results
        if let Some(canary) = canary_results {
            let canary_check = self.validate_canary(canary, &proposal.test_plan.success_criteria)?;
            all_passed &= canary_check.passed;
            checks.push(canary_check);
        }
        
        // 5. Validate estimated impact vs actual
        let impact_check = self.validate_impact(proposal, &test_results.benchmarks)?;
        all_passed &= impact_check.passed;
        checks.push(impact_check);
        
        // Record validation
        let record = ValidationRecord {
            proposal_id: proposal.id.clone(),
            proposal_kind: format!("{:?}", proposal.kind),
            passed: all_passed,
            p_value: checks.iter().filter_map(|c| c.p_value).fold(1.0, |a, b| a.min(b)),
            effect_size: checks.iter().filter_map(|c| c.effect_size).fold(0.0, |a, b| a.max(b)),
            timestamp: chrono::Utc::now(),
        };
        
        let mut history = self.history.lock().await;
        history.validations.push(record);
        if history.validations.len() > history.max_size {
            history.validations.remove(0);
        }
        
        self.metrics.counter("validations", 1);
        self.metrics.gauge("validation_passed", if all_passed { 1.0 } else { 0.0 });
        
        info!("Validation {} for proposal {}", if all_passed { "PASSED" } else { "FAILED" }, proposal.id);
        
        Ok(ValidationResult {
            passed: all_passed,
            checks,
            summary: self.generate_summary(&checks),
        })
    }
    
    /// Validate unit tests (all must pass)
    fn validate_unit_tests(&self, tests: &[crate::sandbox::TestResult]) -> Result<ValidationCheck> {
        let total = tests.len();
        let passed = tests.iter().filter(|t| t.passed).count();
        let failed = total - passed;
        
        Ok(ValidationCheck {
            name: "Unit Tests".to_string(),
            passed: failed == 0,
            p_value: None,
            effect_size: None,
            details: format!("{}/{} passed", passed, total),
            violations: if failed > 0 {
                vec![format!("{} unit test(s) failed", failed)]
            } else {
                vec![]
            },
        })
    }
    
    /// Validate integration tests (all must pass)
    fn validate_integration_tests(&self, tests: &[crate::sandbox::TestResult]) -> Result<ValidationCheck> {
        let total = tests.len();
        let passed = tests.iter().filter(|t| t.passed).count();
        let failed = total - passed;
        
        Ok(ValidationCheck {
            name: "Integration Tests".to_string(),
            passed: failed == 0,
            p_value: None,
            effect_size: None,
            details: format!("{}/{} passed", passed, total),
            violations: if failed > 0 {
                vec![format!("{} integration test(s) failed", failed)]
            } else {
                vec![]
            },
        })
    }
    
    /// Validate benchmarks with statistical tests
    async fn validate_benchmarks(&self, benchmarks: &[crate::sandbox::BenchmarkResult], criteria: &SuccessCriteria) -> Result<Vec<ValidationCheck>> {
        let mut checks = Vec::new();
        
        for bench in benchmarks {
            // For each benchmark metric, compare against baseline
            // In practice, we'd need baseline data - here we use the benchmark's own metrics
            for (metric_name, value) in &bench.metrics {
                let check = self.statistical_test(
                    &format!("{}.{}", bench.name, metric_name),
                    value,
                    criteria,
                )?;
                checks.push(check);
            }
        }
        
        Ok(checks)
    }
    
    /// Perform statistical test (t-test against baseline)
    fn statistical_test(&self, name: &str, value: &f64, criteria: &SuccessCriteria) -> Result<ValidationCheck> {
        // In a real implementation, this would:
        // 1. Get historical baseline data for this metric
        // 2. Perform t-test (or bootstrap) to compare current vs baseline
        // 3. Calculate effect size (Cohen's d)
        // 4. Check if improvement is statistically significant
        
        // Placeholder: simulate a t-test
        // We'll assume we have baseline data with mean=100, std=10, n=30
        // And current sample has mean=value, std=5, n=30
        
        let baseline_mean = 100.0;
        let baseline_std = 10.0;
        let baseline_n = 30;
        
        let current_mean = *value;
        let current_std = 5.0;
        let current_n = 30;
        
        // Pooled standard deviation
        let pooled_std = ((baseline_std.powi(2) * (baseline_n - 1) as f64 + 
                          current_std.powi(2) * (current_n - 1) as f64) / 
                         (baseline_n + current_n - 2) as f64).sqrt();
        
        // Cohen's d
        let cohens_d = (current_mean - baseline_mean) / pooled_std;
        
        // T-statistic
        let t_stat = (current_mean - baseline_mean) / (pooled_std * (1.0/baseline_n as f64 + 1.0/current_n as f64).sqrt());
        
        // Degrees of freedom
        let df = (baseline_n + current_n - 2) as f64;
        
        // P-value (two-tailed)
        let t_dist = StudentsT::new(0.0, 1.0, df).map_err(|e| Error::Validation(format!("T-distribution error: {}", e)))?;
        let p_value = 2.0 * (1.0 - t_dist.cdf(t_stat.abs()));
        
        // Check significance
        let significant = p_value < criteria.statistical_significance;
        
        // Check minimum improvement
        let improvement_pct = ((current_mean - baseline_mean) / baseline_mean) * 100.0;
        let meets_min_improvement = improvement_pct >= criteria.min_improvement_pct;
        
        let passed = significant && meets_min_improvement;
        
        Ok(ValidationCheck {
            name: name.to_string(),
            passed,
            p_value: Some(p_value),
            effect_size: Some(cohens_d),
            details: format!("mean={:.2}, baseline={:.2}, d={:.3}, p={:.4}, improvement={:.1}%", 
                           current_mean, baseline_mean, cohens_d, p_value, improvement_pct),
            violations: {
                let mut v = Vec::new();
                if !significant {
                    v.push(format!("Not statistically significant (p={:.4} >= {:.4})", p_value, criteria.statistical_significance));
                }
                if !meets_min_improvement {
                    v.push(format!("Improvement {:.1}% below minimum {:.1}%", improvement_pct, criteria.min_improvement_pct));
                }
                v
            },
        })
    }
    
    /// Validate canary results
    fn validate_canary(&self, canary: &crate::sandbox::CanaryResults, criteria: &SuccessCriteria) -> Result<ValidationCheck> {
        let passed = canary.passed && canary.analysis.all_passed;
        
        Ok(ValidationCheck {
            name: "Canary".to_string(),
            passed,
            p_value: None,
            effect_size: None,
            details: format!("CPU avg: {:.1}%, Mem avg: {:.1}%, Samples: {}", 
                           canary.analysis.cpu_avg, canary.analysis.mem_avg, canary.analysis.sample_count),
            violations: if !canary.analysis.all_passed {
                canary.analysis.violations.clone()
            } else {
                vec![]
            },
        })
    }
    
    /// Validate estimated impact vs actual results
    fn validate_impact(&self, proposal: &Proposal, benchmarks: &[crate::sandbox::BenchmarkResult]) -> Result<ValidationCheck> {
        // Check if actual performance matches estimated impact
        let estimated = proposal.estimated_impact.performance_delta_pct;
        
        // Calculate actual performance change from benchmarks
        let mut actual_changes = Vec::new();
        for bench in benchmarks {
            for (metric, value) in &bench.metrics {
                if metric.contains("performance") || metric.contains("throughput") || metric.contains("latency") {
                    // Would compare with baseline
                    actual_changes.push(*value);
                }
            }
        }
        
        let actual_avg = if actual_changes.is_empty() { 
            0.0 
        } else { 
            actual_changes.iter().sum::<f64>() / actual_changes.len() as f64 
        };
        
        let diff = (actual_avg - estimated).abs();
        let passed = diff <= 20.0; // Within 20% of estimate
        
        Ok(ValidationCheck {
            name: "Impact Validation".to_string(),
            passed,
            p_value: None,
            effect_size: None,
            details: format!("Estimated: {:.1}%, Actual avg: {:.1}%, Diff: {:.1}%", estimated, actual_avg, diff),
            violations: if !passed {
                vec![format!("Actual impact differs from estimate by {:.1}%", diff)]
            } else {
                vec![]
            },
        })
    }
    
    /// Generate summary of validation
    fn generate_summary(&self, checks: &[ValidationCheck]) -> String {
        let total = checks.len();
        let passed = checks.iter().filter(|c| c.passed).count();
        let failed = total - passed;
        
        format!("{}/{} checks passed", passed, total)
    }
    
    /// Get validation history
    pub async fn history(&self) -> Vec<ValidationRecord> {
        self.history.lock().await.validations.clone()
    }
    
    /// Bootstrap confidence interval
    pub fn bootstrap_ci(&self, data: &[f64], confidence: f64) -> Result<(f64, f64)> {
        if data.len() < 2 {
            return Err(Error::Validation("Insufficient data for bootstrap".to_string()));
        }
        
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut bootstrap_means = Vec::with_capacity(self.config.bootstrap_samples);
        
        for _ in 0..self.config.bootstrap_samples {
            let sample: Vec<f64> = (0..data.len())
                .map(|_| data[rng.gen_range(0..data.len())])
                .collect();
            bootstrap_means.push(sample.iter().sum::<f64>() / sample.len() as f64);
        }
        
        bootstrap_means.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let alpha = (1.0 - confidence) / 2.0;
        let lower_idx = (alpha * self.config.bootstrap_samples as f64) as usize;
        let upper_idx = ((1.0 - alpha) * self.config.bootstrap_samples as f64) as usize - 1;
        
        Ok((bootstrap_means[lower_idx], bootstrap_means[upper_idx]))
    }
    
    /// Cohen's d effect size
    pub fn cohens_d(&self, group1: &[f64], group2: &[f64]) -> Result<f64> {
        if group1.is_empty() || group2.is_empty() {
            return Err(Error::Validation("Empty groups".to_string()));
        }
        
        let mean1 = group1.iter().sum::<f64>() / group1.len() as f64;
        let mean2 = group2.iter().sum::<f64>() / group2.len() as f64;
        
        let var1 = group1.iter().map(|x| (x - mean1).powi(2)).sum::<f64>() / (group1.len() - 1) as f64;
        let var2 = group2.iter().map(|x| (x - mean2).powi(2)).sum::<f64>() / (group2.len() - 1) as f64;
        
        let pooled_std = ((var1 + var2) / 2.0).sqrt();
        
        if pooled_std == 0.0 {
            return Ok(0.0);
        }
        
        Ok((mean1 - mean2) / pooled_std)
    }
}

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub passed: bool,
    pub checks: Vec<ValidationCheck>,
    pub summary: String,
}

/// Individual validation check
#[derive(Debug, Clone)]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub p_value: Option<f64>,
    pub effect_size: Option<f64>,
    pub details: String,
    pub violations: Vec<String>,
}