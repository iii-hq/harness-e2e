//! Suites this Console keeps. A suite is only what to test: its scenarios,
//! how many times each runs and how many technical retries a crash gets.
//! The master plan's suites are read-only; a local one starts as a copy of
//! another suite and is edited here.
pub(crate) mod stacks;
pub(crate) mod store;

use std::collections::BTreeSet;

use anyhow::{ensure, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::scenarios::ScenarioId;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub(crate) struct LocalSuite {
    pub id: String,
    pub label: String,
    pub scenarios: Vec<String>,
    pub repetitions: u32,
    pub technical_retries: u8,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct SuiteCreateRequest {
    /// The suite it starts as a copy of: one of the master plan or of this Console.
    pub from: String,
    /// Empty or absent names it after that suite.
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct SuiteUpdateRequest {
    pub suite_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub scenarios: Option<Vec<String>>,
    #[serde(default)]
    pub repetitions: Option<u32>,
    #[serde(default)]
    pub technical_retries: Option<u8>,
}

impl LocalSuite {
    pub(crate) fn apply(&mut self, update: &SuiteUpdateRequest) {
        if let Some(label) = &update.label {
            self.label = label.trim().to_owned();
        }
        if let Some(scenarios) = &update.scenarios {
            self.scenarios = scenarios.clone();
        }
        if let Some(repetitions) = update.repetitions {
            self.repetitions = repetitions;
        }
        if let Some(retries) = update.technical_retries {
            self.technical_retries = retries;
        }
    }

    /// What a suite may hold: a name, known scenarios once each, 1 to 20
    /// runs and at most 3 technical retries.
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.label.is_empty()
                && self.label.chars().count() <= 160
                && !self.label.chars().any(char::is_control),
            "Name the suite (up to 160 characters, without control characters)."
        );
        ensure!(
            !self.scenarios.is_empty(),
            "Select at least one test for the suite."
        );
        ensure!(
            self.scenarios.iter().collect::<BTreeSet<_>>().len() == self.scenarios.len(),
            "A suite lists each scenario once."
        );
        for id in &self.scenarios {
            id.parse::<ScenarioId>()?;
        }
        ensure!(
            (1..=20).contains(&self.repetitions),
            "runs must be between 1 and 20"
        );
        ensure!(
            self.technical_retries <= 3,
            "technical_retries must be between 0 and 3"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite() -> LocalSuite {
        LocalSuite {
            id: "local-0123456789ab".into(),
            label: "Mine".into(),
            scenarios: vec!["minimal_path".into()],
            repetitions: 1,
            technical_retries: 0,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_suite_holds_a_name_known_scenarios_runs_and_retries() {
        suite().validate().unwrap();
        for (change, reason) in [
            (
                SuiteUpdateRequest {
                    label: Some("  ".into()),
                    ..SuiteUpdateRequest::default()
                },
                "Name the suite",
            ),
            (
                SuiteUpdateRequest {
                    scenarios: Some(Vec::new()),
                    ..SuiteUpdateRequest::default()
                },
                "at least one test",
            ),
            (
                SuiteUpdateRequest {
                    scenarios: Some(vec!["minimal_path".into(), "minimal_path".into()]),
                    ..SuiteUpdateRequest::default()
                },
                "each scenario once",
            ),
            (
                SuiteUpdateRequest {
                    scenarios: Some(vec!["retired_scenario".into()]),
                    ..SuiteUpdateRequest::default()
                },
                "retired_scenario",
            ),
            (
                SuiteUpdateRequest {
                    repetitions: Some(21),
                    ..SuiteUpdateRequest::default()
                },
                "between 1 and 20",
            ),
            (
                SuiteUpdateRequest {
                    technical_retries: Some(4),
                    ..SuiteUpdateRequest::default()
                },
                "between 0 and 3",
            ),
        ] {
            let mut edited = suite();
            edited.apply(&change);
            let error = edited.validate().unwrap_err().to_string();
            assert!(error.contains(reason), "{error}");
        }
    }
}
