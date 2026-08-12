use std::fmt::Write as _;

use super::{sum_cost, E2eReport};

impl E2eReport {
    pub fn summary(&self, verbose: bool) -> String {
        let mut output = String::new();
        let total_cost = sum_cost(
            self.scenarios
                .iter()
                .map(|scenario| scenario.aggregate.cost.total_usd),
        );
        let _ = writeln!(
            output,
            "Harness E2E: {}  subject={}/{}  cost={}",
            if self.passed { "PASS" } else { "FAIL" },
            self.subject.provider,
            self.subject.model,
            format_cost(total_cost),
        );

        for scenario in &self.scenarios {
            let duration_ms = scenario
                .runs
                .iter()
                .map(|run| run.wall_time_ms)
                .sum::<u64>();
            let score = scenario
                .aggregate
                .median_score
                .map(|score| format!("{score:.1}"))
                .unwrap_or_else(|| "n/a".to_string());
            let case = scenario.case.as_ref();
            let tier = case
                .map(|case| format!("{:?}", case.complexity.tier))
                .unwrap_or_else(|| "unclassified".to_string());
            let _ = writeln!(
                output,
                "{} {}  tier={}  score={}  passed={}/{}  time={}  cost={}",
                if scenario.passed { "PASS" } else { "FAIL" },
                scenario.scenario_id,
                tier,
                score,
                scenario.aggregate.passed_runs,
                scenario.aggregate.runs,
                format_duration(duration_ms),
                format_cost(scenario.aggregate.cost.total_usd),
            );

            for (index, run) in scenario.runs.iter().enumerate() {
                let score = run
                    .score
                    .map(|score| score.to_string())
                    .unwrap_or_else(|| "n/a".to_string());
                let retries = if run.retry_attempts.is_empty() {
                    String::new()
                } else {
                    format!("  retries={}", run.retry_attempts.len())
                };
                let _ = writeln!(
                    output,
                    "  run {}: {}  score={}  time={}  cost={}{}",
                    index + 1,
                    run.status.label(),
                    score,
                    format_duration(run.wall_time_ms),
                    format_cost(run.cost.total_usd),
                    retries,
                );

                for attempt in &run.retry_attempts {
                    let reason = attempt
                        .failures
                        .first()
                        .map(|failure| failure.message.as_str())
                        .unwrap_or("no failure detail");
                    let _ = writeln!(
                        output,
                        "    retry after {}: {}",
                        attempt.status.label(),
                        single_line(reason),
                    );
                }
                for failure in &run.failures {
                    let _ = writeln!(
                        output,
                        "    {:?}: {}",
                        failure.phase,
                        single_line(&failure.message),
                    );
                }
                for gate in run.hard_gates.iter().filter(|gate| verbose || !gate.passed) {
                    let _ = writeln!(
                        output,
                        "    gate {}: {} - {}",
                        gate.id,
                        if gate.passed { "pass" } else { "FAIL" },
                        single_line(&gate.reason),
                    );
                }
                for deliverable in run
                    .deliverables
                    .iter()
                    .filter(|deliverable| verbose || !deliverable.passed())
                {
                    let _ = writeln!(
                        output,
                        "    deliverable {}: {} - schema={} provenance={} invariants={}/{}",
                        deliverable.id,
                        if deliverable.passed() { "pass" } else { "FAIL" },
                        deliverable.schema_valid,
                        deliverable.provenance_valid,
                        deliverable
                            .invariants
                            .iter()
                            .filter(|invariant| invariant.passed)
                            .count(),
                        deliverable.invariants.len(),
                    );
                }
                for criterion in run.criteria.iter().filter(|criterion| {
                    verbose
                        || criterion
                            .awarded
                            .is_some_and(|awarded| awarded < criterion.possible)
                }) {
                    let awarded = criterion
                        .awarded
                        .map(|awarded| awarded.to_string())
                        .unwrap_or_else(|| "n/a".to_string());
                    let _ = writeln!(
                        output,
                        "    criterion {}: {}/{} - {}",
                        criterion.id,
                        awarded,
                        criterion.possible,
                        single_line(&criterion.reason),
                    );
                }
            }
        }
        output
    }
}

fn format_cost(cost: Option<f64>) -> String {
    cost.map(|cost| format!("${cost:.4}"))
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m{seconds:02}s")
    }
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
