import json
import os
import re
import pathlib
import subprocess
import tempfile
import unittest

import yaml


ROOT = pathlib.Path(__file__).resolve().parents[2]


class WorkflowBoundaryTests(unittest.TestCase):
    def test_partial_reruns_download_the_contract_uploaded_by_preparation(self):
        jobs = yaml.safe_load((ROOT / ".github/workflows/exact-stack-e2e.yml").read_text())["jobs"]
        upload = next(step for step in jobs["prepare"]["steps"]
                      if step.get("uses", "").startswith("actions/upload-artifact@"))
        self.assertIsNotNone(upload.get("id"), "preparation must expose the uploaded contract")
        self.assertEqual(jobs["prepare"]["outputs"].get("contract_artifact_id"),
                         "${{ steps." + upload["id"] + ".outputs.artifact-id }}")
        self.assertEqual(jobs["prepare"]["outputs"].get("contract_attempt"), "${{ github.run_attempt }}")
        self.assertEqual(upload["with"]["retention-days"], 90)
        # On attempt 2, preparation's retained output is still artifact 71.
        retained_outputs = {"needs.prepare.outputs.contract_artifact_id": "71"}
        for name in ("groups", "finalize"):
            download = next(step for step in jobs[name]["steps"]
                            if step.get("with", {}).get("path") == "target/harness-e2e-contract")
            expression = download["with"].get("artifact-ids", "").removeprefix("${{ ").removesuffix(" }}")
            self.assertEqual(retained_outputs.get(expression), "71")
            self.assertEqual(download["with"].get("github-token"), "${{ github.token }}")
            self.assertEqual(download["with"].get("run-id"), "${{ github.run_id }}")

    def test_rerun_artifacts_belong_to_the_jobs_actual_execution(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/exact-stack-e2e.yml").read_text())
        steps = workflow["jobs"]["finalize"]["steps"]
        selection = next((step for step in steps if step.get("id") == "group_artifacts"), None)
        self.assertIsNotNone(selection, "reruns must select evidence from each job's actual execution")
        download = next(step for step in steps if step.get("id") == "group_download")
        self.assertEqual(download["with"].get("artifact-ids"), "${{ steps.group_artifacts.outputs.artifact_ids }}")
        self.assertEqual(download["with"].get("path"), "${{ steps.group_artifacts.outputs.download_path }}")
        self.assertEqual(download["with"].get("github-token"), "${{ github.token }}")
        self.assertEqual(download["with"].get("run-id"), "${{ github.run_id }}")

        def job(group, start, end, conclusion="success"):
            return {"name": f"smoke-r01 · {group}", "run_attempt": 3, "status": "completed",
                    "started_at": f"2026-09-21T{start}Z", "completed_at": f"2026-09-21T{end}Z",
                    "conclusion": conclusion}

        def artifact(group, attempt, identifier, created, expired=False):
            return {"id": identifier, "name": f"e2e-observation-execution-1-smoke-r01-{group}-gh-{attempt}",
                    "created_at": f"2026-09-21T{created}Z", "expired": expired}

        old_job = job("case-old", "05:00:00", "05:01:00")
        new_job = job("case-new", "11:00:00", "11:01:00", "failure")
        old = artifact("case-old", 1, 10, "05:00:50")
        stale = artifact("case-new", 1, 11, "05:00:50")
        diagnostic = artifact("case-new", 3, 30, "11:00:50")
        cases = [
            ("copied_success_and_new_failure", [old_job, new_job], [old, stale, diagnostic], 1, "10,30"),
            ("missing_new_artifact", [new_job], [stale], 1, ""),
            ("expired_new_artifact", [new_job], [stale, dict(diagnostic, expired=True)], 1, ""),
            ("different_contract", [old_job], [old], 2, ""),
            ("ambiguous_artifacts", [new_job], [diagnostic, dict(diagnostic, id=31)], 1, ""),
            ("ambiguous_jobs", [new_job, new_job], [diagnostic], 1, ""),
            ("malformed_job", [dict(new_job, completed_at=None)], [diagnostic], 1, ""),
            ("single_artifact", [old_job], [old, stale], 1, "10"),
            ("single_new_failure", [new_job], [diagnostic], 1, "30"),
            ("future_attempt", [new_job], [artifact("case-new", 4, 40, "11:00:50")], 1, ""),
        ]
        for name, jobs, artifacts, contract_attempt, expected_ids in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                (root / "bin").mkdir()
                (root / "jobs.json").write_text(json.dumps([{"jobs": jobs}]))
                (root / "artifacts.json").write_text(json.dumps([{"artifacts": artifacts}]))
                gh = root / "bin/gh"
                gh.write_text('#!/bin/sh\ncase "$*" in\n'
                              '  *"/attempts/3/jobs?per_page=100"*) cat "$FIXTURE_DIR/jobs.json" ;;\n'
                              '  *"/artifacts?per_page=100"*) cat "$FIXTURE_DIR/artifacts.json" ;;\n'
                              '  *) exit 2 ;;\nesac\n')
                gh.chmod(0o755)
                output = root / "github-output"
                result = subprocess.run(["bash", "-c", selection["run"]], cwd=root, env={
                    **os.environ, "PATH": f"{root / 'bin'}:{os.environ['PATH']}",
                    "FIXTURE_DIR": str(root), "GITHUB_OUTPUT": str(output),
                    "GITHUB_REPOSITORY": "iii-hq/harness-e2e", "GITHUB_RUN_ID": "77",
                    "GITHUB_RUN_ATTEMPT": "3", "CONTRACT_ATTEMPT": str(contract_attempt),
                    "EXECUTION_KEY": "execution-1",
                }, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                values = dict(line.split("=", 1) for line in output.read_text().splitlines())
                self.assertEqual(values["artifact_ids"], expected_ids)
                selected = json.loads((root / "target/selected-group-artifacts.json").read_text())
                self.assertEqual(
                    ",".join(str(item["id"]) for item in selected.values()), expected_ids,
                )
                if name == "single_artifact":
                    # download-artifact v7 flattens a singleton even with merge-multiple:false.
                    self.assertEqual(values["download_path"], "target/downloaded-groups/" + old["name"])
                    self.assertEqual(selected["smoke-r01 · case-old"]["run_attempt"], 1)
                elif name == "single_new_failure":
                    self.assertEqual(values["download_path"], "target/downloaded-groups/" + diagnostic["name"])
                else:
                    self.assertEqual(values["download_path"], "target/downloaded-groups")

    def test_external_actions_are_pinned_to_immutable_commits(self):
        action_ref = re.compile(r"^\s*uses:\s*([^\s#]+)", re.MULTILINE)
        immutable = re.compile(r"^[^\s]+@[0-9a-f]{40}$")
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            for action in action_ref.findall(path.read_text(encoding="utf-8")):
                self.assertRegex(action, immutable, f"{path.name}: {action}")

    def test_migrated_operational_paths_are_compose_only_and_unversioned(self):
        paths = [
            ROOT / "scripts/exact_stack_campaign.py",
            ROOT / "scripts/run_exact_stack_group.sh",
            ROOT / ".github/workflows/exact-stack-e2e.yml",
            ROOT / "src/worker.rs",
            ROOT / "src/main.rs",
        ]
        forbidden = [
            "iii " + "worker",
            "iii-" + "worker",
            "iii." + "lock",
            "HARNESS_E2E_" + "DATA_DIR",
            "contract_" + "schema_version",
            "schema" + "_version",
        ]
        for path in paths:
            content = path.read_text()
            for token in forbidden:
                self.assertNotIn(token, content, f"{path.relative_to(ROOT)} contains {token}")

    def test_exact_stack_campaign_execution_is_owned_here(self):
        workflow = (
            ROOT / ".github/workflows/exact-stack-e2e.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("strategy:\n      fail-fast: false", workflow)
        self.assertIn("scripts/run_exact_stack_group.sh", workflow)
        self.assertIn("scripts/exact_stack_campaign.py", workflow)
        self.assertIn("runs-on: ${{ matrix.runs_on }}", workflow)
        self.assertIn("environment: harness-e2e-trusted", workflow)
        self.assertNotIn("ref: ${{ inputs.runner_sha }}", workflow)
        self.assertNotIn("matrix.requires_", workflow)
        launcher = (ROOT / "scripts/run_exact_stack_group.sh").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("engineering-ticket.bundle", launcher)
        self.assertNotIn("HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH", launcher)
        self.assertNotIn("HARNESS_E2E_FIXTURE_PATH", launcher)
        self.assertNotIn("cleanup --lease-id", launcher)
        self.assertNotIn("iii-hq/workers", workflow)

    def test_the_campaign_workflow_knows_nothing_about_the_contract(self):
        """Contract fields are read by the scripts, never by the workflow: a
        field the workflow read itself would have to change in lockstep."""
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        for selector in (".suite.", ".plan.definition", ".security.", ".orchestration", ".runner.", ".runtime."):
            self.assertNotIn(selector, workflow, f"workflow selects contract field {selector}")
        self.assertIn("exact_stack_campaign.py digest", workflow)
        self.assertIn("exact_stack_campaign.py groups", workflow)
        self.assertIn("exact_stack_campaign.py validate", workflow)

    def test_a_dispatch_names_suite_stack_model_and_profile(self):
        """What to test, where, with whom, and optionally for which Release
        Control execution; the older five inputs are still accepted. Nothing
        is required by the form: preparation says what is missing."""
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        inputs = yaml.safe_load(workflow)[True]["workflow_dispatch"]["inputs"]
        self.assertEqual(
            list(inputs),
            ["suite", "stack", "model", "profile", "execution_id", "plan", "runner_sha", "cli_version"],
        )
        self.assertFalse(any(spec["required"] for spec in inputs.values()))
        # Every input reaches a shell through the environment, never
        # interpolated into the script.
        for job in yaml.safe_load(workflow)["jobs"].values():
            for step in job["steps"]:
                for expression in ("${{ inputs.", "${{ env."):
                    self.assertNotIn(expression, step.get("run", ""), step.get("name"))
        # A suite stated whole is named "custom suite" in the run title.
        self.assertIn("startsWith(inputs.suite, '{') && 'custom suite' || inputs.suite", workflow)
        # The runner the stack runs materializes the suite, with the flag every
        # release of it knows.
        self.assertIn('"$RUNNER_BINARY" test-plan materialize --profile "$suite"', workflow)
        self.assertNotIn("cargo build", workflow)
        self.assertIn("prepare_execution.py dispatch", workflow)
        self.assertNotIn("resolve_stack_lock", workflow)
        # Scripts come from the dispatched ref, and anyone who may dispatch may
        # run it: the identity that matters is the OIDC token of the reports.
        self.assertNotIn("ref: ${{ inputs.runner_sha }}", workflow)
        # Identity stays a gate: a dispatch that reports to Release Control
        # comes from its bot, with the ids it issues.
        gate = next(step for step in yaml.safe_load(workflow)["jobs"]["prepare"]["steps"]
                    if step.get("name") == "Validate a Release Control dispatch")
        self.assertEqual(gate["if"], "inputs.execution_id != ''")
        for check in ('test "$GITHUB_ACTOR" = "$RELEASE_CONTROL_BOT_LOGIN"',
                      '[[ "$EXECUTION_ID" =~ ^[0-9a-f-]{36}$ ]]',
                      '[[ -z "$RUNNER_SHA" || "$RUNNER_SHA" =~ ^[0-9a-f]{40}$ ]]'):
            self.assertIn(check, gate["run"])

    def test_the_dispatched_plan_reaches_disk_as_the_plan(self):
        """An older dispatch keeps its plan verbatim for the readers of
        plan.json, and becomes the suite, model and stack it stands for."""
        workflow = yaml.safe_load((ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8"))
        step = next(step for step in workflow["jobs"]["prepare"]["steps"] if step.get("name") == "Read the dispatch")
        plan = {"key": "harness-regression", "profile": {"plan_id": "harness", "id": "regression"},
                "subject": {"provider": "deepseek", "model": "deepseek-v4-flash"},
                "runner": {"revision": "a" * 40, "version": "0.12.2"}}
        with tempfile.TemporaryDirectory() as directory:
            env = {name: "" for name in step["env"]}
            env.update(DISPATCH_PLAN=json.dumps(plan), DISPATCH_STACK='{"versions":{"harness":"1.9.3"}}',
                       DISPATCH_EXECUTION_ID="b0607faa-096a-4efe-a4a2-a2a9bc06de83", DISPATCH_CLI_VERSION="0.24.2")
            subprocess.run(["bash", "-c", step["run"].replace("target/", f"{directory}/")], cwd=ROOT,
                           env={**os.environ, **env}, check=True, capture_output=True, text=True)
            written = pathlib.Path(directory) / "harness-e2e-contract"
            self.assertEqual(json.loads((written / "plan.json").read_text()), plan)
            execution = json.loads((written / "execution.json").read_text())
            stack = yaml.safe_load((written / "stack.yaml").read_text())
        self.assertEqual(execution["suite"], "regression")
        self.assertEqual(execution["model"], "deepseek/deepseek-v4-flash")
        self.assertEqual(stack["iii"], "0.24.2")
        self.assertEqual(stack["containers"]["harness"]["version"], "1.9.3")
        self.assertEqual(stack["containers"]["harness-e2e"]["version"], "0.12.2")

    def test_groups_start_the_stack_preparation_assembled_and_locked(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8"))
        prepare = [step.get("name") for step in workflow["jobs"]["prepare"]["steps"]]
        order = ["Resolve the iii release and the template", "Fetch the stack's runner",
                 "Materialize the requested suite", "Report the materialized suite",
                 "Write one contract per campaign", "Assemble and lock the stack",
                 "Lock every contract to the assembled stack"]
        self.assertEqual([name for name in prepare if name in order], order)
        steps = {step.get("name"): step for step in workflow["jobs"]["prepare"]["steps"]}
        # Assembled with the credentials the groups get, and tried twice.
        assemble = steps["Assemble and lock the stack"]
        for secret in ("ZAI_API_KEY", "DEEPSEEK_API_KEY", "TYPESAFE_API_KEY"):
            self.assertEqual(assemble["env"][secret], "${{ secrets." + secret + " }}")
        self.assertIn("for attempt in 1 2; do", assemble["run"])
        # Its evidence passes the group packaging checks before upload, and
        # never fails a prepared execution.
        self.assertIn("exact_stack_campaign.py package", steps["Package the stack assembly evidence"]["run"])
        self.assertTrue(steps["Package the stack assembly evidence"]["continue-on-error"])
        # A preparation that fails after the materialized report still closes
        # the execution in Release Control.
        failed = workflow["jobs"]["report_preparation_failure"]
        self.assertEqual(failed["needs"], "prepare")
        self.assertEqual(failed["if"], "always() && needs.prepare.result != 'success' && needs.prepare.outputs.materialized == 'true'")
        self.assertIn("report_execution.py summary", failed["steps"][-1]["run"])
        self.assertEqual(workflow["jobs"]["prepare"]["outputs"]["materialized"], "${{ steps.materialized.outputs.posted }}")
        self.assertIn('echo "posted=true"', steps["Report the materialized suite"]["run"])
        report = steps["Report the materialized suite"]["run"]
        self.assertIn("--cli-version", report)
        self.assertIn('--runner-sha "$RUNNER_REVISION"', report)
        # The runner's identity: an older dispatch's runner_sha, otherwise the
        # revision of the runner the stack ran, never this workflow's commit.
        self.assertIn("jq -r '.runner_revision'", steps["Fetch the stack's runner"]["run"])
        self.assertNotIn("RUNNER_SHA", steps["Fetch the stack's runner"].get("env", {}))
        self.assertEqual(workflow["jobs"]["prepare"]["outputs"]["runner_revision"], "${{ steps.stack_runner.outputs.revision }}")
        for job, step in (("groups", "Report this shard's runs"), ("finalize", "Report the campaign summary")):
            env = next(s for s in workflow["jobs"][job]["steps"] if s.get("name") == step)["env"]
            self.assertEqual(env["RUNNER_REVISION"], "${{ needs.prepare.outputs.runner_revision }}")
        self.assertNotIn("github.sha }}'", yaml.safe_dump(workflow))
        launcher = (ROOT / "scripts/run_exact_stack_group.sh").read_text()
        self.assertIn('{file:$file,frozen:$frozen}', launcher)
        self.assertIn("HARNESS_E2E_ASSEMBLE_ONLY", launcher)

    def test_every_execution_reports_whatever_it_managed_to_observe(self):
        """No execution is lost: the profile is reported before anything runs,
        each shard reports its runs whatever the group did, and the summary is
        posted whatever the finalizer did."""
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        for kind in ("materialized", "shard", "summary"):
            self.assertIn(f"report_execution.py {kind}", workflow)
        # Whatever the group or finalizer did — and only when there is a
        # Release Control execution to report to.
        for step in ("Report this shard's runs", "Report the campaign summary"):
            condition = workflow.split(step, 1)[1].lstrip().split("\n", 1)[0]
            self.assertEqual(condition, "if: always() && inputs.execution_id != ''")
        materialized = next(step for step in yaml.safe_load(workflow)["jobs"]["prepare"]["steps"]
                            if step.get("name") == "Report the materialized suite")
        self.assertEqual(materialized["if"], "inputs.execution_id != ''")
        # Admission is gone with the campaign it admitted; the reports carry
        # the OIDC identity now, and the first one binds the run.
        self.assertNotIn("/admit", workflow)
        self.assertNotIn("exact_stack_campaign.py admit", workflow)

    def test_finalizer_aggregates_only_campaigns_from_the_current_execution(self):
        import subprocess
        import tempfile
        import textwrap

        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        finalizer = workflow.split("\n  finalize:", 1)[1]
        self.assertLess(
            finalizer.index("Fetch the stack's runner"),
            finalizer.index("Restore deterministic group paths"),
        )
        restore = finalizer.split("- name: Restore deterministic group paths", 1)[
            1
        ].split("\n      - ", 1)[0]
        command = textwrap.dedent(restore.split("run: |\n", 1)[1])
        command = command.replace(
            "${{ inputs.execution_id }}", "execution-1"
        ).replace("${{ github.run_attempt }}", "1")

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            campaign_root = root / "target/harness-e2e-campaign"
            stale_regression = campaign_root / "regression-r01"
            stale_regression.mkdir(parents=True)
            (stale_regression / "stack-lock.json").write_text('{"campaign":"stale"}')
            (stale_regression / "campaign-summary.json").write_text(
                '{"campaign":"stale"}'
            )
            (campaign_root / "obsolete-r01").mkdir()
            contracts = root / "target/harness-e2e-contract/contracts"
            contracts.mkdir(parents=True)
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "exact_stack_campaign.py").write_text(
                "import sys\nassert sys.argv[1] == 'groups'\nprint('case-minimal-path\\ncase-missing')\n"
            )
            campaigns = ("smoke-r01", "capability-r01", "regression-r01")
            selected = {}
            for campaign in campaigns:
                (contracts / f"{campaign}.json").write_text(f'{{"campaign":"{campaign}"}}')
                group = root / (
                    "target/downloaded-groups/"
                    f"e2e-observation-execution-1-{campaign}-case-minimal-path-gh-1"
                )
                group.mkdir(parents=True)
                (group / "result.json").write_text("{}")
                selected[f"{campaign} · case-minimal-path"] = {"name": group.name}
                stale_group = root / f"target/downloaded-groups/e2e-observation-execution-1-{campaign}-case-missing-gh-1"
                stale_group.mkdir()
                (stale_group / "result.json").write_text('{"status":"old_success"}')
            (root / "target/selected-group-artifacts.json").write_text(json.dumps(selected))

            subprocess.run(["bash", "-c", command], cwd=root, check=True)

            self.assertEqual(
                sorted(path.name for path in campaign_root.iterdir()),
                ["capability-r01", "regression-r01", "smoke-r01"],
            )
            for campaign in campaigns:
                contract = contracts / f"{campaign}.json"
                self.assertTrue(contract.is_file())
                self.assertEqual(
                    (campaign_root / campaign / "stack-lock.json").read_text(),
                    contract.read_text(),
                )
                self.assertTrue(
                    (
                        campaign_root
                        / campaign
                        / "groups/case-minimal-path/result.json"
                    ).is_file()
                )
                missing = campaign_root / campaign / "groups/case-missing"
                self.assertEqual(json.loads((missing / "failure.json").read_text())["outcome"], "infra_failed")
                self.assertFalse((missing / "result.json").exists())
            self.assertFalse(
                (campaign_root / "regression-r01/campaign-summary.json").exists()
            )

    def test_exact_stack_is_the_only_release_control_executor(self):
        workflows = {path.name for path in (ROOT / ".github/workflows").glob("*.yml")}
        self.assertIn("exact-stack-e2e.yml", workflows)
        self.assertNotIn("shadow.yml", workflows)
        # Releasing is one workflow on main: the tag cut-release.yml pushes
        # builds, publishes next and promotes latest in release.yml itself.
        self.assertIn("release.yml", workflows)
        self.assertNotIn("promote-latest.yml", workflows)
        release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertIn("scripts/release_worker.py build-payload", release)
        self.assertIn("scripts/promote_registry.py promote", release)
        self.assertNotIn("inputs.expected_latest_version", release)
        cut = (ROOT / ".github/workflows/cut-release.yml").read_text(encoding="utf-8")
        self.assertNotIn("cherry-pick", cut)
        self.assertIn("git push origin HEAD:main", cut)

    def test_release_control_is_the_only_operational_campaign_dispatch(self):
        for name in (
            "daily.yml",
            "post-deploy.yml",
            "weekly.yml",
            "weekly-stress.yml",
            "run-campaign.yml",
        ):
            self.assertFalse((ROOT / ".github/workflows" / name).exists())
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text()
        self.assertIn("exact-stack", workflow)
        contract_tool = (ROOT / "scripts/exact_stack_campaign.py").read_text()
        # Release Control owns the campaign; this repository states the policy it
        # materializes for the aggregator and reads none of it back from config.
        self.assertIn('"failure_policy": "advisory"', contract_tool)
        self.assertNotIn("config/campaigns", contract_tool)


    def test_canonical_gate_pins_and_authorizes_the_e2e_revision(self):
        workflow = (ROOT / ".github/workflows/canonical-gate.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("repository: iii-hq/harness-e2e", workflow)
        self.assertIn("ref: ${{ inputs.e2e_revision }}", workflow)
        self.assertIn("compare/$E2E_REVISION...$default_sha", workflow)
        self.assertIn("/opt/iii-harness-e2e/resolve-cutover-evidence", workflow)

    def test_compose_campaigns_do_not_prepare_an_engineering_ticket_fixture(self):
        launcher = (ROOT / "scripts/run_exact_stack_group.sh").read_text()
        self.assertNotIn("HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH", launcher)
        self.assertNotIn("HARNESS_E2E_FIXTURE_PATH", launcher)
        self.assertNotIn("engineering_fixture_revision=", launcher)
        self.assertNotIn("prepare --execution-id", launcher)
        self.assertNotIn("cleanup --lease-id", launcher)
        self.assertNotIn("git commit", launcher)

    def test_registry_groups_prepare_private_sources_without_persisting_credentials(self):
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text()
        source_block = workflow.split("Mint private Registry source token", 1)[1].split(
            "Mint private trending topics fixture token", 1
        )[0]
        self.assertEqual(source_block.count("startsWith(matrix.group_id, 'case-registry-')"), 4)
        self.assertIn("repository: iii-hq/registry", source_block)
        self.assertIn("ref: 662eb87c1bdbb395f36264d5d26bf823e2ace783", source_block)
        self.assertIn("repository: iii-hq/e2e-fixture", source_block)
        fixture_checkout = source_block.split("Checkout latest E2E fixture", 1)[1].split(
            "Route Registry fixture clones", 1
        )[0]
        self.assertNotIn("ref:", fixture_checkout)
        self.assertEqual(source_block.count("persist-credentials: false"), 2)
        self.assertIn("actions/create-github-app-token@fee1f7d63c2ff003460e3d139729b119787bc349", source_block)
        self.assertIn("permission-contents: read", source_block)
        self.assertNotIn("E2E_FIXTURE_GITHUB_TOKEN", source_block)
        self.assertNotIn("token@github.com", source_block)

    def test_trending_topics_group_fetches_only_the_pinned_fixture_without_credentials(self):
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text()
        source_block = workflow.split("Mint private trending topics fixture token", 1)[1].split(
            "- uses: actions/download-artifact", 1
        )[0]
        self.assertEqual(source_block.count("matrix.group_id == 'case-trending-topics-build'"), 3)
        self.assertIn("repositories: e2e-fixture", source_block)
        self.assertIn("permission-contents: read", source_block)
        self.assertIn("persist-credentials: false", source_block)
        lifecycle = (ROOT / "tests/fixtures/trending-topics-build/lifecycle.py").read_text()
        revision = lifecycle.split('FIXTURE_SHA = "', 1)[1].split('"', 1)[0]
        self.assertIn(f"ref: {revision}", source_block)
        self.assertIn(".insteadOf git@github.com:iii-hq/e2e-fixture.git", source_block)
        self.assertNotIn("token@github.com", source_block)

if __name__ == "__main__":
    unittest.main()
