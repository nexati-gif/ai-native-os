//! Statistical functions for validation

use statrs::distribution::{Continuous, StudentsT, Normal};
use statrs::statistics::Statistics;

/// Perform two-sample t-test
pub fn t_test(sample1: &[f64], sample2: &[f64]) -> Option<StatisticalTestResult> {
    if sample1.len() < 2 || sample2.len() < 2 {
        return None;
    }

    let mean1 = sample1.iter().sum::<f64>() / sample1.len() as f64;
    let mean2 = sample2.iter().sum::<f64>() / sample2.len() as f64;

    let var1 = sample1.iter().map(|x| (x - mean1).powi(2)).sum::<f64>() / (sample1.len() - 1) as f64;
    let var2 = sample2.iter().map(|x| (x - mean2).powi(2)).sum::<f64>() / (sample2.len() - 1) as f64;

    let pooled_se = ((var1 / sample1.len() as f64) + (var2 / sample2.len() as f64)).sqrt();
    let t_stat = (mean1 - mean2) / pooled_se;

    let df = sample1.len() + sample2.len() - 2;
    let dist = StudentsT::new(0.0, 1.0, df as f64).ok()?;
    
    let p_value = 2.0 * (1.0 - dist.cdf(t_stat.abs()));

    Some(StatisticalTestResult {
        t_statistic: t_stat,
        p_value,
        degrees_of_freedom: df,
        mean_diff: mean1 - mean2,
    })
}

/// Bootstrap confidence interval
pub fn bootstrap_ci(data: &[f64], confidence: f64, iterations: usize) -> Option<(f64, f64)> {
    if data.is_empty() {
        return None;
    }

    use rand::Rng;
    let mut rng = rand::thread_rng();
    let n = data.len();
    let mut means = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let sample: Vec<f64> = (0..n).map(|_| data[rng.gen_range(0..n)]).collect();
        means.push(sample.iter().sum::<f64>() / n as f64);
    }

    means.sort_by(|a, b| a.partial_cmp(b).unwrap());
    
    let alpha = (1.0 - confidence) / 2.0;
    let lower_idx = (alpha * iterations as f64) as usize;
    let upper_idx = ((1.0 - alpha) * iterations as f64) as usize;

    Some((means[lower_idx], means[upper_idx]))
}

/// Effect size (Cohen's d)
pub fn effect_size(sample1: &[f64], sample2: &[f64]) -> Option<f64> {
    if sample1.len() < 2 || sample2.len() < 2 {
        return None;
    }

    let mean1 = sample1.iter().sum::<f64>() / sample1.len() as f64;
    let mean2 = sample2.iter().sum::<f64>() / sample2.len() as f64;

    let var1 = sample1.iter().map(|x| (x - mean1).powi(2)).sum::<f64>() / (sample1.len() - 1) as f64;
    let var2 = sample2.iter().map(|x| (x - mean2).powi(2)).sum::<f64>() / (sample2.len() - 1) as f64;

    let pooled_sd = ((var1 + var2) / 2.0).sqrt();
    
    if pooled_sd == 0.0 {
        return None;
    }

    Some((mean1 - mean2) / pooled_sd)
}

/// Statistical test result
#[derive(Debug, Clone)]
pub struct StatisticalTestResult {
    pub t_statistic: f64,
    pub p_value: f64,
    pub degrees_of_freedom: usize,
    pub mean_diff: f64,
}

impl StatisticalTestResult {
    pub fn is_significant(&self, alpha: f64) -> bool {
        self.p_value < alpha
    }
}