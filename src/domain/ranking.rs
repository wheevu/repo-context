#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

/// Configurable weights for file ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingWeights {
    #[serde(default = "w_readme")]
    pub readme: f64,
    #[serde(default = "w_contribution_doc")]
    pub contribution_doc: f64,
    #[serde(default = "w_main_doc")]
    pub main_doc: f64,
    #[serde(default = "w_config")]
    pub config: f64,
    #[serde(default = "w_entrypoint")]
    pub entrypoint: f64,
    #[serde(default = "w_api_definition")]
    pub api_definition: f64,
    #[serde(default = "w_core_source")]
    pub core_source: f64,
    #[serde(default = "w_example")]
    pub example: f64,
    #[serde(default = "w_test")]
    pub test: f64,
    #[serde(default = "w_default")]
    pub default: f64,
    #[serde(default = "w_generated")]
    pub generated: f64,
    #[serde(default = "w_lock_file")]
    pub lock_file: f64,
    #[serde(default = "w_vendored")]
    pub vendored: f64,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            readme: w_readme(),
            contribution_doc: w_contribution_doc(),
            main_doc: w_main_doc(),
            config: w_config(),
            entrypoint: w_entrypoint(),
            api_definition: w_api_definition(),
            core_source: w_core_source(),
            example: w_example(),
            test: w_test(),
            default: w_default(),
            generated: w_generated(),
            lock_file: w_lock_file(),
            vendored: w_vendored(),
        }
    }
}

fn w_readme() -> f64 {
    1.00
}
fn w_contribution_doc() -> f64 {
    0.98
}
fn w_main_doc() -> f64 {
    0.95
}
fn w_config() -> f64 {
    0.90
}
fn w_entrypoint() -> f64 {
    0.85
}
fn w_api_definition() -> f64 {
    0.80
}
fn w_core_source() -> f64 {
    0.75
}
fn w_example() -> f64 {
    0.60
}
fn w_test() -> f64 {
    0.50
}
fn w_default() -> f64 {
    0.50
}
fn w_generated() -> f64 {
    0.20
}
fn w_lock_file() -> f64 {
    0.15
}
fn w_vendored() -> f64 {
    0.10
}
