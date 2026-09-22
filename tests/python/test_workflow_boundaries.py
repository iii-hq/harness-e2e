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
                    "EXECUTION_ID": "execution-1",
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
        self.assertIn("ref: ${{ inputs.runner_sha }}", workflow)
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
        """The workflow is read from the default branch; the executor is pinned
        per campaign by runner_sha. Any contract field the workflow reads itself
        would have to change in lockstep with the contract, so it reads none —
        the scripts checked out at runner_sha read them all."""
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        for selector in (".suite", ".plan.definition", ".security.", ".orchestration", ".runner."):
            self.assertNotIn(selector, workflow, f"workflow selects contract field {selector}")
        self.assertIn("exact_stack_campaign.py digest", workflow)
        self.assertIn("exact_stack_campaign.py groups", workflow)
        self.assertIn("exact_stack_campaign.py validate", workflow)

    def test_release_control_dispatches_a_profile_and_resolves_nothing(self):
        """The five inputs of the run ledger. Release Control names an
        execution, a plan with its profile, a stack policy, the executor commit
        and the CLI; the composition and every exact version are resolved here."""
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        block = workflow.split("    inputs:\n", 1)[1].split("\npermissions:", 1)[0]
        self.assertEqual(
            sorted(re.findall(r"^      (\w+):$", block, re.MULTILINE)),
            ["cli_version", "execution_id", "plan", "runner_sha", "stack"],
        )
        # An absent input is a silent default; every one of the five is stated.
        self.assertEqual(block.count("required: true"), 5)
        # The composition belongs to the runner: the profile is materialized
        # from the pinned commit, never read out of the dispatch.
        self.assertIn("test-plan materialize --profile", workflow)
        self.assertIn("scripts/resolve_stack_lock.py", workflow)

    def test_the_dispatched_plan_reaches_disk_as_the_plan(self):
        """The recording step must write the dispatch, not a verdict about it.

        `jq -e 'type == "object"'` validates and then writes its own `true`,
        which every later step reads as the plan. Run the real command."""
        import shutil
        import subprocess

        if not shutil.which("jq"):
            self.skipTest("jq is not installed")
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        block = workflow.split("Record the dispatch", 1)[1].split("- uses:", 1)[0]
        command = next(line.strip() for line in block.splitlines() if "plan.json" in line)
        command = command.replace("target/harness-e2e-contract/plan.json", "/dev/stdout")
        plan = json.dumps({"key": "harness-regression", "profile": {"plan_id": "harness", "id": "regression"}})

        written = subprocess.run(
            ["bash", "-c", command], env={"PLAN": plan, "PATH": os.environ["PATH"]},
            capture_output=True, text=True, check=True,
        ).stdout
        self.assertEqual(json.loads(written), json.loads(plan))

        # And a dispatch that is not an object still fails the step.
        rejected = subprocess.run(
            ["bash", "-c", command], env={"PLAN": "true", "PATH": os.environ["PATH"]},
            capture_output=True, text=True,
        )
        self.assertNotEqual(rejected.returncode, 0)

    def test_every_execution_reports_whatever_it_managed_to_observe(self):
        """No execution is lost: the profile is reported before anything runs,
        each shard reports its runs whatever the group did, and the summary is
        posted whatever the finalizer did."""
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text(encoding="utf-8")
        for kind in ("materialized", "shard", "summary"):
            self.assertIn(f"report_execution.py {kind}", workflow)
        shard = workflow.split("Report this shard's runs", 1)[1]
        self.assertTrue(shard.lstrip().startswith("if: always()"), "the shard report must be unconditional")
        summary = workflow.split("Report the campaign summary", 1)[1]
        self.assertTrue(summary.lstrip().startswith("if: always()"), "the summary report must be unconditional")
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
            finalizer.index("Swatinem/rust-cache"),
            finalizer.index("Restore deterministic group paths"),
        )
        restore = finalizer.split("- name: Restore deterministic group paths", 1)[
            1
        ].split("\n      - uses:", 1)[0]
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
        self.assertNotIn("release.yml", workflows)

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

    def test_endurance_keeps_github_authority_in_the_post_run_publisher(self):
        workflow = (ROOT / ".github/workflows/engineering-endurance.yml").read_text(
            encoding="utf-8"
        )
        scenario = (
            ROOT / "src/scenarios/engineering_endurance_ladder.rs"
        ).read_text(encoding="utf-8")
        publisher = (
            ROOT / "scripts/publish_engineering_endurance.py"
        ).read_text(encoding="utf-8")
        self.assertIn("timeout-minutes: 240", workflow)
        self.assertIn("E2E_FIXTURE_GITHUB_TOKEN", workflow)
        self.assertIn("Publish sanitized GitHub handoff", workflow)
        self.assertIn('"github::*"', scenario)
        self.assertIn('ALLOWED_REPOSITORY = "iii-hq/e2e-fixture"', publisher)
        self.assertNotIn("E2E_FIXTURE_GITHUB_TOKEN", scenario)
        self.assertNotIn("hidden_output", publisher.split("def public_projection", 1)[1].split("def create_blob", 1)[0])


if __name__ == "__main__":
    unittest.main()
