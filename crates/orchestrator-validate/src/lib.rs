//! Validator: statistical validation of proposals

pub mod validator;
pub mod statistics;

pub use validator::{Validator, ValidationConfig};
pub use statistics::{StatisticalTest, t_test, bootstrap_ci, effect_size};