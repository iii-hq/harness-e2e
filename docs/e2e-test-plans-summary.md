# E2E Test Plans Summary

The canonical plans are `daily`, `weekly`, and `post-deploy`. All plans are
advisory while longitudinal history is being calibrated. Objective failures,
infrastructure failures, coverage gaps, budget violations, and quality scores
remain separate signals; a scenario failure does not automatically become a
deployment or promotion gate.

## plan: daily

Purpose: detect regressions quickly with one execution per scenario. A single
technical retry is allowed only for infrastructure failures.

1. **`minimal_path`** — Validates the minimum Harness execution path by writing
   an exact value and returning a concise answer. Its purpose is to measure
   baseline correctness, call count, turn count, and execution overhead.

2. **`tool_contract_recovery`** — Validates recovery from an outdated tool
   runbook by requiring discovery of the current contract, profile and
   timezone lookup, migration to the v2 function, and safe event creation. Its
   purpose is to verify tool-contract discovery, backward-compatible recovery,
   decoy avoidance, auditability, and correct final state.

3. **`shell_coder_sandbox`** — Validates a complete, reproducible code-fix
   workflow: reproduce the failure, diagnose it, modify only production code,
   pass public and hidden tests, and reproduce the result in an offline Python
   sandbox. Its purpose is to measure real coding correctness, evidence quality,
   scope discipline, and host-to-sandbox parity.

4. **`performance_regression`** — Validates that an algorithm can be optimized
   without changing its output or ordering, using deterministic hash and
   equality counters. Its purpose is to detect computational regressions while
   keeping wall-clock time as an advisory signal.

5. **`database_migration_recovery`** — Validates safe completion of an
   interrupted migration, including backfill completion, invalid-record
   quarantine, compatibility-view creation, idempotent replay, and sentinel
   preservation. Its purpose is to verify migration recovery without duplicate
   data or collateral writes.

6. **`engineering_ticket_git_handoff`** — Validates coordinated engineering
   work in which separate sessions plan and implement a ticket, then produce
   linear Git checkpoints. Its purpose is to verify focused and hidden tests,
   ancestry, scope, absence of merge commits, and clean session handoff.

7. **`chess_engine_build`** — Validates implementation of legal chess moves and
   `perft`, including castling, en passant, promotions, pins, and check
   evasion. Its purpose is to measure correctness on a complex algorithmic
   implementation against an independent oracle.

8. **`git_regression_forensics`** — Validates investigation of a real offline
   Git history by reproducing good and bad behavior, locating the first faulty
   commit efficiently, and producing a structured report. Its purpose is to
   measure regression diagnosis, evidence precision, and probe efficiency.

## plan: weekly

Purpose: measure repeatability, concurrency, coordination, adaptive reasoning,
fault recovery, and long-running capability. The main suite runs weekly; fault
recovery and endurance are specialized weekly tracks.

1. **`minimal_path` and `timer_wake`** — Validate the baseline execution path
   and the complete timer lifecycle: registration before the source write,
   correct wake timing, and resource cleanup. Their purpose is to provide a
   weekly canary for basic execution and scheduled wake behavior.

2. **`shell_coder_sandbox`** — Validates that investigation, patching, public
   and hidden tests, and offline sandbox parity converge across three runs. Its
   purpose is to measure repeatable coding performance rather than a one-off
   success.

3. **`performance_regression`** — Validates optimization correctness across
   five runs using deterministic work measurements. Its purpose is to provide
   the primary statistical signal for code-performance regressions.

4. **`chess_engine_build`** — Validates complex algorithmic implementation
   across three runs. Its purpose is to measure repeatability of correctness
   under a demanding coding task.

5. **`git_regression_forensics`** — Validates historical regression diagnosis
   across three runs. Its purpose is to measure repeatable identification of
   the first bad commit and accurate supporting evidence.

6. **`engineering_ticket_git_handoff`** — Validates multi-session planning,
   implementation, testing, and linear Git checkpointing across three runs. Its
   purpose is to measure reliable engineering coordination.

7. **`contention_ledger`** — Validates atomic concurrent updates by running
   three writers that perform five increments each on one shared accumulator.
   Its purpose is to detect lost updates, missing audit records, and incorrect
   wake-before-fan-out ordering; the expected total is 15.

8. **`security_review`** — Validates two exact reviews of a local repository,
   scan deduplication, security-capability detection, actionable suggestions
   without mutation, and GitHub-cycle reconciliation. Its purpose is to test
   bounded, repeatable security-review coordination.

9. **`incident_response`** — Validates adaptive incident handling when
   deterministic evidence disproves the initial duplicate-provider hypothesis.
   Its purpose is to verify re-planning, recognition of ACK-timeout redelivery,
   and selection of either remediation or rollback without executing both.

10. **`cross_repo_contract_migration`** — Validates migration of a versioned
    producer and visible consumer when a later canary reveals a second
    incompatible consumer. Its purpose is to measure adaptive planning,
    preservation of accepted work, multi-repository consistency, and backward
    and forward contract compatibility.

11. **`fault_recovery_matrix`** — Validates recovery from stateful and
    coordinated faults across calibrated L2-L4 phases, including delays,
    failed first writes, duplicate delivery, child failures, timeouts,
    malformed or out-of-order results, bounded amplification, and cleanup. Its
    purpose is to measure whether work can recover without corrupting results,
    duplicating effects, or leaking resources.

12. **`engineering_endurance_ladder`** — Validates one uninterrupted session
    evolving a durable queue through up to ten cumulative tickets, with public
    and hidden tests and immutable Git checkpoints at every rung. Its purpose is
    to measure the maximum sustained engineering capability before the session
    reaches its first capability boundary.

## plan: post-deploy

Purpose: validate the exact published version through a path close to real
consumption and recovery. Every scenario runs once with no technical retry, so
release regressions are not masked.

1. **`minimal_path` and `tool_contract_recovery`** — Validate basic
   availability, execution efficiency, current tool-contract discovery, safe
   recovery, and final-state correctness in the published version. Their
   purpose is to act as release canaries.

2. **`shell_coder_sandbox`** — Validates that the published version can execute
   a complete code-fix workflow and reproduce the result in an offline sandbox.
   Its purpose is to verify release-level coding correctness and evidence
   parity.

3. **`performance_regression`** — Validates optimization without functional
   regression in the published version. Its purpose is to detect release-level
   algorithmic or computational regressions.

4. **`chess_engine_build`** — Validates complex move-generation and search
   implementation against an independent oracle. Its purpose is to exercise
   demanding algorithmic correctness after publication.

5. **`engineering_ticket_git_handoff`** — Validates coordinated planning,
   implementation, testing, and linear Git delivery. Its purpose is to confirm
   that the published version preserves reliable engineering handoff behavior.

6. **`database_migration_recovery`** — Validates idempotent migration recovery
   and data preservation from a partially migrated state. Its purpose is to
   protect release upgrades against duplication and destructive side effects.

7. **`cross_app_transaction`** — Validates convergence of one account across
   three versioned services and recovery from a deterministic compare-and-swap
   conflict. Its purpose is to verify cross-application consistency using
   snapshots and audit logs rather than relying only on the final response.

8. **`browser_cross_site`** — Validates browser behavior across three isolated
   local origins and compares the outcome with runner-controlled backend state.
   Its purpose is to detect navigation, session, and cross-site isolation
   regressions.

9. **`release_train_recovery`** — Validates recovery of an immutable cancelled
   release after partial assets, reuse of the same run ID on a new attempt, and
   safe handling of an incompatible `latest` pointer. Its purpose is to verify
evidence-gated release recovery without retagging, version bumps, or direct
channel mutation.
