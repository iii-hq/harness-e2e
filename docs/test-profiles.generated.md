# Generated Harness test profiles

Generated from [config/test-plan.json](../config/test-plan.json), revision 1.
Edit the source, then run `cargo run --locked -- test-plan sync`.
All profiles are advisory; repetitions are independent invocations.

| Profile | Cases | Repetitions | Planned slots | Execution |
| --- | ---: | ---: | ---: | --- |
| Smoke | 5 | 1 | 5 | Campaign runner |
| Regression | 12 | 1 | 12 | Campaign runner |
| Capability | 47 | 1 | 47 | Campaign runner |
| Evolution | 18 | 5 | 90 | Campaign runner |
| Resilience | 4 | 1 | 13 | Protected fault executor |
| Endurance | 5 | 1 | 5 | Campaign runner |

## Smoke

Verify essential behavior in the published stack.

Cases: `minimal_path`, `tool_contract_recovery`, `persistent_state`, `timer_wake`, `shell_coder_sandbox`.

Measures: `execution_reliability`, `completion_rate`, `objective_score_coverage`.

Retry ceiling: 0; non-replay-safe cases always use zero.

## Regression

Detect broken behavior across representative capabilities.

Cases: `minimal_path`, `tool_contract_recovery`, `persistent_state`, `timer_wake`, `shell_coder_sandbox`, `performance_regression`, `git_regression_forensics`, `database_migration_recovery`, `contention_ledger`, `swe_config_isolation`, `swe_cache_invalidation`, `swe_replay_recovery`.

Measures: `execution_reliability`, `completion_rate`, `deliverable_success`.

Retry ceiling: 1; non-replay-safe cases always use zero.

## Capability

Measure coverage, repeatability and difficulty across capability domains.

Cases: `minimal_path`, `persistent_state`, `insert_record`, `sequential_pipeline`, `database_migration_recovery`, `context_pressure`, `moving_target`, `mechanical_reaction`, `timer_wake`, `validation_loop`, `validation_self_repair`, `validation_scope_enforcement`, `validation_chain`, `cleanup_under_failure`, `poison_message`, `wake_chain_soak`, `contention_ledger`, `quorum_fan_in`, `subagent_validation`, `subagent_validation_failure`, `fanout_ladder`, `depth_ladder`, `research_pipeline`, `receiving_operation`, `shell_coder_sandbox`, `performance_regression`, `chess_engine_build`, `git_regression_forensics`, `engineering_ticket_git_handoff`, `swe_config_isolation`, `swe_cache_invalidation`, `swe_batch_replay`, `swe_replay_recovery`, `swe_contract_migration`, `swe_tenant_isolation`, `swe_replay_performance`, `swe_release_handoff`, `tool_contract_recovery`, `cross_app_transaction`, `browser_cross_site`, `prompt_injection_resilience`, `secret_hygiene`, `security_review`, `policy_bound_action`, `incident_response`, `cross_repo_contract_migration`, `release_train_recovery`.

Measures: `deliverable_success`, `structural_integrity`, `completion_evidence_coverage`.

Retry ceiling: 0; non-replay-safe cases always use zero.

## Evolution

Compare quality and resource use across fixed Harness versions.

Cases: `minimal_path`, `tool_contract_recovery`, `persistent_state`, `timer_wake`, `context_pressure`, `validation_self_repair`, `contention_ledger`, `quorum_fan_in`, `shell_coder_sandbox`, `performance_regression`, `swe_config_isolation`, `swe_cache_invalidation`, `swe_replay_recovery`, `swe_contract_migration`, `engineering_ticket_git_handoff`, `cross_app_transaction`, `prompt_injection_resilience`, `incident_response`.

Measures: `completion_rate`, `tokens_per_verified_success`, `tokens_per_completion`, `p50_total_tokens`, `p50_function_calls`.

Retry ceiling: 0; non-replay-safe cases always use zero.

## Resilience

Measure recovery without duplicate effects or leaked resources.

Cases: `cleanup_under_failure`, `poison_message`, `subagent_validation_failure`, `swe_replay_recovery`.

Measures: `deliverable_success`, `structural_integrity`, `work_amplification`.

Retry ceiling: 0; non-replay-safe cases always use zero.

Fault: `weekly-l2-recovery` / `stateful.2`, 3 repetitions, 60 minutes soak.

Fault: `weekly-l3-recovery` / `coordination.3`, 3 repetitions, 60 minutes soak.

Fault: `weekly-l4-recovery` / `coordination.4`, 3 repetitions, 60 minutes soak.

## Endurance

Measure sustained correct work and the accepted capability boundary.

Cases: `engineering_endurance_ladder`, `swe_service_journey`, `wake_chain_soak`, `fanout_ladder`, `depth_ladder`.

Measures: `max_accepted_rung`, `accepted_tickets`, `time_to_boundary_ms`.

Retry ceiling: 0; non-replay-safe cases always use zero.

## Capability modules

| Module | Cases |
| --- | --- |
| m1 — State and basic execution | `minimal_path`, `persistent_state`, `insert_record`, `sequential_pipeline`, `database_migration_recovery` |
| m2 — Context and instructions | `context_pressure`, `moving_target`, `mechanical_reaction` |
| m3 — Wakes and recovery | `timer_wake`, `validation_loop`, `validation_self_repair`, `validation_scope_enforcement`, `validation_chain`, `cleanup_under_failure`, `poison_message`, `wake_chain_soak` |
| m4 — Coordination | `contention_ledger`, `quorum_fan_in`, `subagent_validation`, `subagent_validation_failure`, `fanout_ladder`, `depth_ladder`, `research_pipeline`, `receiving_operation` |
| m5 — Software engineering | `shell_coder_sandbox`, `performance_regression`, `chess_engine_build`, `git_regression_forensics`, `engineering_ticket_git_handoff`, `swe_config_isolation`, `swe_cache_invalidation`, `swe_batch_replay`, `swe_replay_recovery`, `swe_contract_migration`, `swe_tenant_isolation`, `swe_replay_performance`, `swe_release_handoff` |
| m6 — Integration | `tool_contract_recovery`, `cross_app_transaction`, `browser_cross_site` |
| m7 — Security and policy | `prompt_injection_resilience`, `secret_hygiene`, `security_review`, `policy_bound_action` |
| m8 — Adaptive operations | `incident_response`, `cross_repo_contract_migration`, `release_train_recovery` |
| m9 — Continuous engineering | `engineering_endurance_ladder`, `swe_service_journey` |

Diagnostic cases: `engineering_ticket`, `trend_blog`, `todo_worker_simple`, `todo_worker_planned`, `chess_play_ladder`.
