import copy
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


sys.path.insert(0, str(ROOT / "scripts"))
renderer = load("render_e2e_report")


def run_report(**overrides):
    report = {
        "run_id": "run-1",
        "attempt_id": "attempt-2",
        "attempt_number": 2,
        "session_id": "session-2",
        "wall_time_ms": 200,
        "status": "passed",
        "technical": "valid",
        "completion": "completed",
        "score": 70,
        "cost": {"total_usd": None},
        "criteria": [{"id": "quality", "possible": 30, "awarded": 0, "reason": "missing"}],
        "failures": [],
        "retry_attempts": [],
        "evidence": [],
    }
    report.update(overrides)
    return report


def results(run, scenario_id="minimal_path"):
    return {
        "execution": {"execution_id": "execution-1"},
        "system_under_test": {"harness_version": "1.2.3"},
        "subject": {"model": "model-1", "provider": "provider-1"},
        "result_contract_sha256": "sha256:result",
        "scenarios": [{
            "scenario_id": scenario_id,
            "case_id": "case-1",
            "behavior_sha256": "sha256:behavior",
            "case": {"seed": 1, "inputs_sha256": "sha256:definition"},
            "runs": [run],
        }]
    }


def discovery_transcript(functions, *, is_error=False, raw=None, origin=True,
                         origin_functions=None, notice="between"):
    content = raw if raw is not None else json.dumps({"functions": functions})
    source = origin_functions
    if source is None:
        source = [
            {"function_id": item["function_id"], "request_schema": {}}
            for item in functions
            if isinstance(item, dict) and isinstance(item.get("function_id"), str)
        ]
    notice_entry = {
        "entry_id": "notice-entry",
        "custom": {"custom_type": "model_notice", "data": {"kind": "registry-changed"}},
    }
    entries = [notice_entry] if notice == "before" else []
    if origin:
        entries.append({"entry_id": "source-call-entry", "message": {
            "role": "assistant",
            "content": [{
                "type": "function_call", "id": "call-1",
                "function_id": "engine::functions::info", "arguments": {},
            }],
        }})
        entries.append({"entry_id": "source-entry", "message": {
            "role": "function_result",
            "function_id": "engine::functions::info",
            "function_call_id": "call-1",
            "is_error": False,
            "content": [{"type": "text", "text": json.dumps({"functions": source})}],
        }})
    if notice == "between":
        entries.append(notice_entry)
    entries.append({"entry_id": "assistant-entry", "message": {
        "role": "assistant",
        "content": [{"type": "function_call", "id": "call-2",
                     "function_id": "agent_trigger", "arguments": {
                         "function": "engine::functions::info", "payload": {}
                     }}],
    }})
    entries.append({"entry_id": "repeat-entry", "message": {
        "role": "function_result",
        "function_id": "engine::functions::info",
        "function_call_id": "call-2",
        "is_error": is_error,
        "content": [{"type": "text", "text": content}],
    }})
    return {"messages": entries}


def tool_error_transcript(calls):
    messages = []
    for index, call in enumerate(calls):
        call_id = call.get("call_id", f"tool-{index}")
        function_id = call.get("function_id", "fixture::lookup")
        arguments = call.get("arguments", {"key": "same"})
        direct = call.get("direct", False)
        messages.append({"entry_id": f"call-entry-{index}", "message": {
            "role": "assistant",
            "content": [{
                "type": "function_call",
                **({"id": call_id} if call_id is not None else {}),
                "function_id": function_id if direct else "agent_trigger",
                "arguments": arguments if direct else {
                    "function": function_id, "payload": arguments,
                },
            }],
        }})
        if call.get("result", True):
            result = {
                "role": "function_result",
                "function_call_id": call_id,
                "function_id": call.get("result_function_id", function_id),
                "is_error": call.get("is_error", True),
            }
            code = call.get("error_code", "FAILED")
            if code is not None:
                result[call.get("code_field", "error_code")] = code
            messages.append({"entry_id": f"result-entry-{index}", "message": result})
    return {"messages": messages}


class MarkdownReportTests(unittest.TestCase):
    def invoke(self, artifacts, output):
        return subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "render_e2e_report.py"),
             "--artifacts", str(artifacts), "--output", str(output)],
            capture_output=True, text=True,
        )

    def test_cli_renders_separate_statuses_missing_metrics_and_canonical_details(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            payload = results(run_report(failures=[{
                "phase": "evaluate", "domain": "subject", "code": "objective_gate_failed",
                "message": "Expected state was absent",
            }], evidence=[{
                "id": "result", "kind": "probe", "path": "evidence/result.json",
                "sha256": "sha256:" + "a" * 64,
            }]))
            original = copy.deepcopy(payload)
            (artifacts / "results.json").write_text(json.dumps(payload))

            completed = self.invoke(artifacts, output)

            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(json.loads((artifacts / "results.json").read_text()), original)
            summary = (output / "summary.md").read_text()
            self.assertIn("| minimal\\_path | valid | completed | 70 |", summary)
            self.assertNotIn("](failures/", summary)
            self.assertRegex(summary, r"`0001-[0-9a-f]{12}\.md`")
            page = next((output / "failures").iterdir()).read_text()
            self.assertIn("| 2 / attempt\\-2 | valid | completed | 70 | unknown | passed |", page)
            self.assertIn("phase=evaluate; domain=subject; code=objective\\_gate\\_failed", page)
            self.assertIn("quality: 0/30", page)
            self.assertIn("path=evidence/result\\.json", page)
            self.assertNotIn("transcript", page.lower())

    def test_failed_retry_is_preserved_when_selected_attempt_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            retry = run_report(
                attempt_id="attempt-1", attempt_number=1, session_id="session-1", wall_time_ms=100,
                cost={"total_usd": 0.25}, status="infrastructure_error",
                technical="technical_invalid", completion="undetermined", score=None,
                transcript=discovery_transcript([{
                    "contract_status": "unchanged_in_context",
                    "function_id": "fixture::cached",
                    "source_function_call_id": "call-1",
                }]),
                failures=[{"phase": "execute", "domain": "e2e_infrastructure",
                           "code": "infrastructure_failed", "message": "timed out"}],
                criteria=[],
            )
            (artifacts / "results.json").write_text(json.dumps(results(run_report(
                score=100, criteria=[{"id": "quality", "possible": 30, "awarded": 30, "reason": "ok"}],
                cost={"total_usd": 0.75}, retry_attempts=[retry],
            ))))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            page = next((output / "failures").iterdir()).read_text()
            self.assertLess(page.index("Failed retry attempt"), page.index("Run outcome (including retries)"))
            self.assertIn("technical\\_invalid", page)
            self.assertIn("timed out", page)
            self.assertIn("Session: session\\-1", page)
            self.assertIn("Wall time ms: 100", page)
            self.assertIn("| 1 / attempt\\-1 | technical\\_invalid | undetermined | unknown | 0\\.25 |", page)
            self.assertIn("| 2 / attempt\\-2 | valid | completed | 100 | 0\\.75 |", page)
            self.assertIn("Repeated contract discovery", page)
            self.assertIn("function_id=fixture::cached", page)
            self.assertIn("source_function_call_id=call\\-1", page)
            diagnostics = json.loads((output / "diagnostics.json").read_text())
            self.assertEqual(diagnostics["affected_runs"], 1)
            self.assertEqual(diagnostics["runs"][0]["attempts"][0]["kind"], "retry")
            self.assertEqual(diagnostics["occurrences"], 1)

    def test_green_run_with_repeated_contract_discovery_gets_a_detail_page(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            run = run_report(
                score=100,
                criteria=[{"id": "quality", "possible": 30, "awarded": 30, "reason": "ok"}],
                failures=[],
                transcript=discovery_transcript([
                    {"contract_status": "unchanged_in_context", "function_id": "fixture::one",
                     "source_function_call_id": "call-1"},
                    {"contract_status": "unchanged_in_context", "function_id": "fixture::two",
                     "source_function_call_id": "call-1"},
                ]),
                efficiency={"function_calls": 5, "total_tokens": 6561},
            )
            (artifacts / "results.json").write_text(json.dumps(results(run)))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            summary = (output / "summary.md").read_text()
            self.assertIn("Runs requiring attention: 1", summary)
            page = next((output / "failures").iterdir()).read_text()
            self.assertIn("Function calls: 5", page)
            self.assertIn("Total tokens: 6561", page)
            self.assertIn("contracts already present in context", page)
            self.assertIn("result_entry_id=repeat\\-entry", page)
            self.assertIn("function_id=fixture::one", page)
            self.assertIn("source_function_call_id=call\\-1", page)
            self.assertIn("registry_changed_notice=notice\\-entry", page)
            self.assertIn("Diagnostic recurrence", summary)
            self.assertIn("| repeated\\_contract\\_discovery | minimal\\_path | case\\-1 | 1 |", summary)

    def test_only_all_unchanged_successful_contract_results_are_reported(self):
        unchanged = {"contract_status": "unchanged_in_context", "function_id": "fixture::one",
                     "source_function_call_id": "call-1"}
        cases = {
            "new schema": discovery_transcript([unchanged, {"function_id": "fixture::new", "request_schema": {}}]),
            "function error": discovery_transcript([unchanged], is_error=True),
            "malformed json": discovery_transcript([], raw="{not-json"),
            "empty response": discovery_transcript([]),
            "top-level function without marker": discovery_transcript([], raw=json.dumps({
                "function_id": "fixture::one", "request_schema": {}
            })),
        }
        for name, transcript in cases.items():
            with self.subTest(name=name):
                self.assertEqual(renderer.repeated_contract_discovery({"transcript": transcript}), [])

    def test_top_level_unchanged_contract_marker_is_reported(self):
        transcript = discovery_transcript([], raw=json.dumps({
            "contract_status": "unchanged_in_context",
            "function_id": "fixture::one",
            "source_function_call_id": "call-1",
        }), origin_functions=[{"function_id": "fixture::one", "request_schema": {}}])

        finding = renderer.repeated_contract_discovery({"transcript": transcript})[0]
        self.assertEqual(finding["repeated_result"], {
            "transcript_index": 4, "entry_id": "repeat-entry", "function_call_id": "call-2",
        })
        self.assertEqual(finding["repeated_call"]["transcript_index"], 3)
        self.assertEqual(finding["functions"][0]["source_result"], {
            "transcript_index": 1, "entry_id": "source-entry",
        })
        self.assertEqual(finding["functions"][0]["registry_changed_notice"]["entry_id"], "notice-entry")

    def test_origin_is_required_and_notice_outside_interval_is_unknown(self):
        unchanged = [{
            "contract_status": "unchanged_in_context", "function_id": "fixture::one",
            "source_function_call_id": "call-1",
        }]
        self.assertEqual(renderer.repeated_contract_discovery({
            "transcript": discovery_transcript(unchanged, origin=False)
        }), [])
        isolated_result = discovery_transcript(unchanged)
        isolated_result["messages"].pop(0)
        self.assertEqual(renderer.repeated_contract_discovery({"transcript": isolated_result}), [])

        duplicate_source_call = discovery_transcript(unchanged)
        duplicate_source_call["messages"].insert(1, copy.deepcopy(duplicate_source_call["messages"][0]))
        self.assertEqual(renderer.repeated_contract_discovery({"transcript": duplicate_source_call}), [])

        reused_by_other_target = discovery_transcript(unchanged)
        reused_by_other_target["messages"].insert(1, {
            "entry_id": "ambiguous-call-entry",
            "message": {"role": "assistant", "content": [{
                "type": "function_call", "id": "call-1",
                "function_id": "fixture::other", "arguments": {},
            }]},
        })
        self.assertEqual(renderer.repeated_contract_discovery({
            "transcript": reused_by_other_target
        }), [])

        finding = renderer.repeated_contract_discovery({
            "transcript": discovery_transcript(unchanged, notice="before")
        })[0]
        self.assertIsNone(finding["functions"][0]["registry_changed_notice"])
        self.assertEqual(finding["cause"], "unknown")

        correlated = renderer.repeated_contract_discovery({
            "transcript": discovery_transcript(unchanged)
        })[0]
        self.assertEqual(correlated["cause"], "harness_notice_correlated")

    def test_contract_discovery_requires_unique_function_results(self):
        unchanged = [{
            "contract_status": "unchanged_in_context", "function_id": "fixture::one",
            "source_function_call_id": "call-1",
        }]
        duplicate_origin = discovery_transcript(unchanged)
        duplicate_origin["messages"].insert(2, copy.deepcopy(duplicate_origin["messages"][1]))
        self.assertEqual(renderer.repeated_contract_discovery({"transcript": duplicate_origin}), [])

        duplicate_repeated = discovery_transcript(unchanged)
        duplicate_repeated["messages"].append(copy.deepcopy(duplicate_repeated["messages"][-1]))
        self.assertEqual(renderer.repeated_contract_discovery({"transcript": duplicate_repeated}), [])

    def test_repeated_tool_error_reports_adjacent_pairs_without_payload(self):
        transcript = tool_error_transcript([
            {"call_id": "tool-1", "arguments": {"secret": "do-not-copy"}, "direct": True},
            {"call_id": "tool-2", "arguments": {"secret": "do-not-copy"}, "direct": True,
             "code_field": "details", "error_code": {"code": "FAILED"}},
            {"call_id": "tool-3", "arguments": {"secret": "do-not-copy"}, "direct": True,
             "code_field": "details", "error_code": {"error": {"code": "FAILED"}}},
        ])
        findings = renderer.repeated_tool_error({"transcript": transcript})

        self.assertEqual(len(findings), 2)
        self.assertEqual(findings[0]["rule_id"], "repeated_tool_error")
        self.assertEqual(findings[0]["cause"], "unknown")
        self.assertEqual(findings[0]["function_id"], "fixture::lookup")
        self.assertEqual(findings[0]["error_code"], "FAILED")
        self.assertEqual(findings[0]["calls"][0]["function_call_id"], "tool-1")
        self.assertEqual(findings[1]["calls"][1]["function_call_id"], "tool-3")
        self.assertNotIn("do-not-copy", json.dumps(findings))

    def test_repeated_tool_error_requires_two_identified_equivalent_adjacent_failures(self):
        cases = {
            "second succeeds": [{}, {"is_error": False}],
            "changed payload": [{}, {"arguments": {"key": "changed"}}],
            "changed code": [{}, {"error_code": "OTHER"}],
            "changed target": [{}, {"function_id": "fixture::other"}],
            "intervening call": [{}, {"function_id": "fixture::other"}, {}],
            "intervening call without id": [{}, {"call_id": None}, {}],
            "unknown result": [{}, {"is_error": None}],
            "missing result": [{}, {"result": False}],
            "missing error code": [{}, {"error_code": None}],
            "same call id": [{"call_id": "duplicate"}, {"call_id": "duplicate"}],
            "result target mismatch": [{}, {"result_function_id": "fixture::other"}],
        }
        for name, calls in cases.items():
            with self.subTest(name=name):
                self.assertEqual(renderer.repeated_tool_error({
                    "transcript": tool_error_transcript(calls)
                }), [])

    def test_repeated_tool_error_excludes_discovery_calls(self):
        for function_id, arguments in (
            ("engine::functions::list", {}),
            ("directory::engine::functions::info", {}),
            ("engine::triggers::info", {}),
            ("engine::triggers::info", {"namespace": "default"}),
        ):
            with self.subTest(function_id=function_id, arguments=arguments):
                self.assertEqual(renderer.repeated_tool_error({
                    "transcript": tool_error_transcript([
                        {"function_id": function_id, "arguments": arguments, "error_code": "NOT_FOUND"},
                        {"function_id": function_id, "arguments": arguments, "error_code": "NOT_FOUND"},
                    ])
                }), [])

    def test_repeated_tool_error_requires_sequential_results(self):
        base = tool_error_transcript([{}, {}])["messages"]
        batched = copy.deepcopy(base)
        batched[0]["message"]["content"].extend(batched[2]["message"]["content"])
        batched = [batched[0], batched[1], batched[3]]
        out_of_order = [base[0], base[2], base[1], base[3]]
        other_error = {
            "entry_id": "other-result",
            "message": {
                "role": "function_result", "function_call_id": "other-call",
                "function_id": "fixture::other", "is_error": True, "error_code": "FAILED",
            },
        }
        for name, messages in {
            "batched calls": batched,
            "out of order results": out_of_order,
            "other failed result between": [base[0], other_error, *base[1:]],
            "other successful result between": [
                base[0],
                {**other_error, "message": {**other_error["message"], "is_error": False}},
                *base[1:],
            ],
        }.items():
            with self.subTest(name=name):
                self.assertEqual(renderer.repeated_tool_error({
                    "transcript": {"messages": messages}
                }), [])

    def test_repeated_tool_errors_do_not_pair_across_attempts(self):
        one_call = tool_error_transcript([{}])
        retry = run_report(attempt_id="retry", transcript=one_call)
        run = run_report(
            attempt_id="terminal", transcript=one_call, retry_attempts=[retry],
            criteria=[{"id": "quality", "possible": 30, "awarded": 30, "reason": "ok"}],
        )
        diagnostics = renderer.diagnostics_document(results(run), [{
            "scenario_id": "minimal_path", "case_id": "case-1", "seed": "1",
            "behavior_sha256": "sha256:behavior", "definition_sha256": "sha256:definition",
            "repetition": 0, "run": run,
        }], "results")

        self.assertEqual(diagnostics["occurrences"], 0)
        self.assertEqual(diagnostics["affected_runs"], 0)

    def test_green_repeated_tool_error_is_rendered_and_grouped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            run = run_report(
                score=100, failures=[],
                criteria=[{"id": "quality", "possible": 30, "awarded": 30, "reason": "ok"}],
                transcript=tool_error_transcript([{"call_id": "tool-1"}, {"call_id": "tool-2"}]),
            )
            (artifacts / "results.json").write_text(json.dumps(results(run)))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            diagnostics = json.loads((output / "diagnostics.json").read_text())
            self.assertEqual(diagnostics["occurrences"], 1)
            self.assertEqual(diagnostics["cohorts"][0]["rule_id"], "repeated_tool_error")
            page = next((output / "failures").iterdir()).read_text()
            self.assertIn("Repeated tool error", page)
            self.assertIn("cause is unknown", page)
            self.assertIn(r"first_call_id=tool\-1", page)

    def test_transcript_coverage_controls_diagnostic_sufficiency(self):
        complete_criteria = [{"id": "quality", "possible": 30, "awarded": 30, "reason": "ok"}]
        without_transcript = run_report(score=100, criteria=complete_criteria)
        no_evidence = renderer.diagnostics_document(results(without_transcript), [{
            "scenario_id": "minimal_path", "run": without_transcript,
        }], "results")
        self.assertEqual(no_evidence["evidence_sufficiency"], "insufficient")
        self.assertEqual(no_evidence["attempts_analyzed"], 0)
        self.assertEqual(no_evidence["attempts_missing_transcript"], 1)
        self.assertIsNone(no_evidence["affected_runs"])
        self.assertIsNone(no_evidence["occurrences"])

        captured_empty = run_report(score=100, criteria=complete_criteria, transcript={"messages": []})
        observed = renderer.diagnostics_document(results(captured_empty), [{
            "scenario_id": "minimal_path", "run": captured_empty,
        }], "results")
        self.assertEqual(observed["evidence_sufficiency"], "observed_runs")
        self.assertEqual(observed["attempts_analyzed"], 1)
        self.assertEqual(observed["occurrences"], 0)

        retry_without_transcript = run_report(attempt_id="retry", criteria=[])
        terminal = run_report(
            score=100, criteria=complete_criteria, retry_attempts=[retry_without_transcript],
            transcript=tool_error_transcript([{}, {}]),
        )
        partial = renderer.diagnostics_document(results(terminal), [{
            "scenario_id": "minimal_path", "run": terminal,
        }], "results")
        self.assertEqual(partial["evidence_sufficiency"], "partial")
        self.assertEqual(partial["attempts_analyzed"], 1)
        self.assertEqual(partial["attempts_missing_transcript"], 1)
        self.assertEqual(partial["occurrences"], 1)

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            (artifacts / "results.json").write_text(json.dumps(results(terminal)))
            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            self.assertIn(
                "Evidence sufficiency: partial (1 analyzed, 1 missing transcript)",
                (output / "summary.md").read_text(),
            )
            self.assertIn(
                "Diagnostic counts include only analyzed attempts.",
                (output / "summary.md").read_text(),
            )

    def test_diagnostics_group_two_occurrences_in_one_run_and_one_in_another(self):
        unchanged = [{
            "contract_status": "unchanged_in_context", "function_id": "fixture::one",
            "source_function_call_id": "call-1",
        }]
        twice = discovery_transcript(unchanged)
        twice["messages"].extend([
            {"entry_id": "assistant-entry-2", "message": {
                "role": "assistant", "content": [{
                    "type": "function_call", "id": "call-3",
                    "function_id": "engine::functions::info", "arguments": {},
                }],
            }},
            {"entry_id": "repeat-entry-2", "message": {
                "role": "function_result", "function_id": "engine::functions::info",
                "function_call_id": "call-3", "is_error": False,
                "content": [{"type": "text", "text": json.dumps({"functions": unchanged})}],
            }},
        ])
        first = run_report(run_id="run-1", attempt_id="attempt-1", transcript=twice)
        second = run_report(
            run_id="run-2", attempt_id="attempt-2", transcript=discovery_transcript(unchanged)
        )
        payload = results(first)
        payload["scenarios"][0]["runs"].append(second)

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            (artifacts / "results.json").write_text(json.dumps(payload))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            diagnostics = json.loads((output / "diagnostics.json").read_text())
            self.assertEqual(diagnostics["identity"]["execution"]["execution_id"], "execution-1")
            self.assertEqual(diagnostics["affected_runs"], 2)
            self.assertEqual(diagnostics["occurrences"], 3)
            self.assertEqual(diagnostics["cohorts"], [{
                "affected_runs": 2,
                "behavior_sha256": "sha256:behavior",
                "case_id": "case-1",
                "definition_sha256": "sha256:definition",
                "occurrences": 3,
                "rule_id": "repeated_contract_discovery",
                "scenario_id": "minimal_path",
                "seed": "1",
            }])

    def test_zero_runs_reports_root_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            (artifacts / "failure.json").write_text(json.dumps({
                "phase": "bootstrap", "outcome": "infra_failed", "message": "engine unavailable"
            }))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            summary = (output / "summary.md").read_text()
            self.assertIn("Infrastructure failure", summary)
            self.assertIn("Canonical source: `failure.json`", summary)
            self.assertIn("engine unavailable", summary)
            self.assertIn("Evidence sufficiency: insufficient (zero analyzable attempts)", summary)
            self.assertIn("Diagnostic affected runs: unknown", summary)
            self.assertIn("Diagnostic occurrences: unknown", summary)
            diagnostics = json.loads((output / "diagnostics.json").read_text())
            self.assertEqual(diagnostics["evidence_sufficiency"], "insufficient")
            self.assertEqual(diagnostics["runs_collected"], 0)
            self.assertEqual(list((output / "failures").iterdir()), [])

    def test_journal_run_and_root_failure_are_both_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            journal = artifacts / "native" / "executions" / "run-1" / "journal"
            events = journal / "events"
            checkpoint = journal / "runs" / "safe-slot"
            events.mkdir(parents=True)
            checkpoint.mkdir(parents=True)
            (events / "0001-slot-inventory-committed.json").write_text(json.dumps({
                "slots": [{
                    "slot_id": "safe-slot", "scenario_id": "partial-scenario",
                    "case_id": "case-1", "seed": "1", "repetition": 0,
                }]
            }))
            (checkpoint / "run-1.json").write_text(json.dumps({
                "slot_id": "safe-slot", "run": run_report(failures=[{
                    "phase": "evaluate", "domain": "subject", "code": "failed", "message": "failed"
                }])
            }))
            (artifacts / "failure.json").write_text(json.dumps({
                "phase": "finalize", "outcome": "infra_failed", "message": "packaging stopped"
            }))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            summary = (output / "summary.md").read_text()
            self.assertIn("Canonical run source: journal", summary)
            self.assertIn("Infrastructure failure", summary)
            self.assertIn("packaging stopped", summary)
            self.assertIn("partial\\-scenario", summary)
            self.assertEqual(len(list((output / "failures").iterdir())), 1)

    def test_hostile_root_failure_cannot_overflow_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            hostile = "'<script>'" * 20_000
            (artifacts / "failure.json").write_text(json.dumps({
                key: hostile
                for key in ("phase", "outcome", "domain", "code", "message", "error", "exit_code")
            }))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            summary = (output / "summary.md").read_bytes()
            self.assertLessEqual(len(summary), renderer.SUMMARY_LIMIT)
            self.assertIn("…".encode(), summary)

    def test_untrusted_markdown_paths_html_and_mentions_are_inert(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            attack = "../../@team <script>alert(1)</script> [click](javascript:alert(1))"
            (artifacts / "results.json").write_text(json.dumps(results(
                run_report(run_id=attack, attempt_id=attack, failures=[{
                    "phase": "execute", "domain": "subject", "code": attack, "message": attack,
                }]),
                scenario_id=attack,
            )))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            files = list((output / "failures").iterdir())
            self.assertEqual(len(files), 1)
            self.assertRegex(files[0].name, r"^0001-[0-9a-f]{12}\.md$")
            rendered = (output / "summary.md").read_text() + files[0].read_text()
            self.assertNotIn("<script>", rendered)
            self.assertNotIn("@team", rendered)
            self.assertNotIn("[click](javascript:", rendered)
            self.assertIn("@\u200bteam", rendered)

    def test_summary_stays_under_github_limit_while_pages_remain_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            artifacts, output = root / "artifacts", root / "report"
            artifacts.mkdir()
            scenarios = []
            repeated = discovery_transcript([{
                "contract_status": "unchanged_in_context", "function_id": "fixture::one",
                "source_function_call_id": "call-1",
            }])
            for index in range(900):
                scenarios.append({
                    "scenario_id": f"scenario-{index}-" + "x" * 100,
                    "case": {"seed": index},
                    "runs": [run_report(
                        run_id=f"run-{index}", attempt_id=f"attempt-{index}", transcript=repeated,
                    )],
                })
            (artifacts / "results.json").write_text(json.dumps({"scenarios": scenarios}))

            self.assertEqual(self.invoke(artifacts, output).returncode, 0)
            summary = (output / "summary.md").read_bytes()
            self.assertLessEqual(len(summary), renderer.SUMMARY_LIMIT)
            self.assertIn(b"additional run report(s) omitted", summary)
            self.assertIn(b"diagnostics.json", summary)
            self.assertEqual(len(list((output / "failures").iterdir())), 900)


if __name__ == "__main__":
    unittest.main()
