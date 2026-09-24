"""The executor: one image of tools, the checkout's scripts mounted into it.

`scripts/run_in_image.sh` is the host side every runner shares (GitHub now,
the Console's Docker runner later); `scripts/executor.sh` is what each phase
does inside the image.
"""

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TAG = "ghcr.io/iii-hq/harness-e2e:tools-" + hashlib.sha256((ROOT / "Dockerfile").read_bytes()).hexdigest()[:12]
DIGEST = "ghcr.io/iii-hq/harness-e2e@sha256:" + "d" * 64

# Logs each call, one argument per line and a blank line after it. `pull`
# succeeds when FAKE_PUBLISHED is set; `image inspect` finds the image once it
# was pulled or built, with a digest either way, as the containerd image store
# gives a local build one too.
FAKE_DOCKER = textwrap.dedent("""\
    #!/usr/bin/env bash
    printf '%s\\n' "$@" '' >>"$FAKE_LOG"
    case "$1 ${2:-}" in
      "pull "*) [[ -n "${FAKE_PUBLISHED:-}" ]] && touch "$FAKE_STATE/present" ;;
      "build "*) cat >/dev/null; touch "$FAKE_STATE/present" ;;
      "image inspect") [[ -f "$FAKE_STATE/present" ]] && printf '%s\\n' "$FAKE_DIGEST" ;;
    esac
""")


def calls(log: Path) -> list[list[str]]:
    return [block.split("\n") for block in log.read_text().split("\n\n") if block]


class WrapperTests(unittest.TestCase):
    def run_wrapper(self, *args, published=True, env=None):
        directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory)
        root = directory / "checkout"
        (root / "scripts").mkdir(parents=True)
        shutil.copy(ROOT / "scripts/run_in_image.sh", root / "scripts")
        shutil.copy(ROOT / "Dockerfile", root)
        (directory / "bin").mkdir()
        docker = directory / "bin/docker"
        docker.write_text(FAKE_DOCKER)
        docker.chmod(0o755)
        (directory / "state").mkdir()
        (directory / "tmp").mkdir()
        socket = directory / "docker.sock"
        socket.touch()
        environment = {
            "PATH": f"{directory / 'bin'}:{os.environ['PATH']}", "HOME": str(directory),
            "FAKE_LOG": str(directory / "docker.log"), "FAKE_STATE": str(directory / "state"),
            "FAKE_DIGEST": DIGEST, "DOCKER_HOST": f"unix://{socket}", "TMPDIR": str(directory / "tmp"),
            **({"FAKE_PUBLISHED": "1"} if published else {}), **(env or {}),
        }
        result = subprocess.run(["bash", str(root / "scripts/run_in_image.sh"), *args], env=environment,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        log = directory / "docker.log"
        return root.resolve(), directory, result, calls(log) if log.exists() else []

    def test_a_phase_runs_in_the_published_image_with_the_checkout_mounted_where_it_is(self):
        root, directory, result, invoked = self.run_wrapper("group", env={
            "HARNESS_E2E_CONTRACT": "target/contract.json", "DEEPSEEK_API_KEY": "secret-value",
            "EXECUTION_KEY": "42", "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "url.x.insteadOf",
            "GIT_CONFIG_VALUE_0": "y", "GIT_CONFIG_GLOBAL": "/host/gitconfig", "UNRELATED": "1",
            "HARNESS_E2E_EXECUTOR_IMAGE": "stale",
        })
        self.assertEqual(invoked[0], ["pull", "--quiet", TAG])
        run = next(call for call in invoked if call[0] == "run")
        self.assertEqual(run[-4:], [TAG, "bash", "scripts/executor.sh", "group"])
        options = run[:-4]
        pairs = {(options[i], options[i + 1]) for i in range(len(options) - 1)}
        socket = directory / "docker.sock"
        tmp = next(value for flag, value in pairs if flag == "--env" and value.startswith("TMPDIR="))[7:]
        self.assertTrue(tmp.startswith(f"{directory / 'tmp'}/harness-e2e-executor."))
        self.assertFalse(Path(tmp).exists(), "the phase's TMPDIR is removed afterwards")
        for pair in [
            ("--user", f"{os.getuid()}:{os.getgid()}"),
            ("--group-add", str(socket.stat().st_gid)),
            ("--volume", f"{socket}:/var/run/docker.sock"),
            ("--volume", f"{root}:{root}"),
            ("--workdir", str(root)),
            ("--volume", f"{tmp}:{tmp}"),
            ("--env", f"TMPDIR={tmp}"),
            ("--env", f"HARNESS_E2E_EXECUTOR_IMAGE={DIGEST}"),
        ]:
            self.assertIn(pair, pairs)
        passed = sorted(value for flag, value in pairs if flag == "--env" and "=" not in value)
        # By name only: the values never reach the command line.
        self.assertEqual(passed, ["DEEPSEEK_API_KEY", "EXECUTION_KEY", "GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0",
                                  "GIT_CONFIG_VALUE_0", "HARNESS_E2E_CONTRACT"])
        self.assertNotIn("secret-value", "\n".join(run))
        self.assertNotIn("--network", run)
        self.assertNotIn("--env-file", run)
        self.assertNotIn("not published", result.stderr)

    def test_an_unpublished_image_is_built_here_and_said_out_loud(self):
        root, _, result, invoked = self.run_wrapper("--env-file", "/secrets/providers.env", "prepare", "assemble",
                                                    published=False)
        self.assertEqual([call[:2] for call in invoked[:3]],
                         [["pull", "--quiet"], ["image", "inspect"], ["build", "--tag"]])
        self.assertIn(f"::warning::{TAG} is not published; building it from {root}/Dockerfile", result.stderr)
        run = next(call for call in invoked if call[0] == "run")
        # Built here, it has no registry digest: the execution records the tag.
        self.assertIn(f"HARNESS_E2E_EXECUTOR_IMAGE={TAG}", run)
        self.assertEqual(run[run.index("--env-file") + 1], "/secrets/providers.env")
        self.assertEqual(run[-5:], [TAG, "bash", "scripts/executor.sh", "prepare", "assemble"])

    def test_a_host_that_runs_one_phase_at_a_time_may_put_it_on_its_network(self):
        _, _, _, invoked = self.run_wrapper("group", env={"HARNESS_E2E_DOCKER_NETWORK": "host"})
        run = next(call for call in invoked if call[0] == "run")
        self.assertEqual(run[run.index("--network") + 1], "host")
        # The wrapper's own setting, not the phase's.
        self.assertNotIn("HARNESS_E2E_DOCKER_NETWORK", run)

    def test_the_image_is_named_by_the_dockerfile(self):
        _, _, result, invoked = self.run_wrapper("image")
        self.assertEqual(result.stdout, TAG + "\n")
        self.assertEqual(invoked, [])


# Stand-ins for what the phases call, logging each call to $FAKE_LOG.
FAKE_SCRIPTS = {
    "prepare_execution.py": """\
        import json, os, pathlib, sys
        open(os.environ["FAKE_LOG"], "a").write("prepare " + " ".join(sys.argv[1:]) + "\\n")
        command = sys.argv[1]
        contract_dir = pathlib.Path("target/harness-e2e-contract")
        if command == "dispatch":
            contract_dir.mkdir(parents=True, exist_ok=True)
            (contract_dir / "execution.json").write_text(json.dumps({"suite": os.environ["DISPATCH_SUITE"]}))
        elif command == "runner":
            print(os.environ["FAKE_RUNNER"])
        elif command == "contracts":
            (contract_dir / "contracts").mkdir(parents=True, exist_ok=True)
            (contract_dir / "contracts/pr-r01.json").write_text("{}")
            (contract_dir / "contracts/resolution.json").write_text(json.dumps({
                "campaign_ids": ["pr-r01"], "matrix": {"include": [{"group_id": "case-minimal-path"}]}}))
        elif command == "runner-binary":
            print(os.environ["FAKE_RUNNER"])
    """,
    "exact_stack_campaign.py": """\
        import os, sys
        open(os.environ["FAKE_LOG"], "a").write("campaign " + " ".join(sys.argv[1:]) + "\\n")
        if sys.argv[1] == "manifest":
            open(sys.argv[sys.argv.index("--output") + 1], "w").write("{}")
    """,
    "run_e2e_campaign.py": """\
        import json, os, sys
        open(os.environ["FAKE_LOG"], "a").write("aggregate " + " ".join(sys.argv[1:]) + "\\n")
        summary = sys.argv[sys.argv.index("--summary") + 1]
        open(summary, "w").write(json.dumps({"campaign": os.path.basename(os.path.dirname(os.path.normpath(summary)))}))
    """,
    "run_exact_stack_group.sh": """\
        echo "group $HARNESS_E2E_CONTRACT $HARNESS_E2E_CAMPAIGN_GROUP_ID ${HARNESS_E2E_ASSEMBLE_ONLY:-} $HARNESS_E2E_ARTIFACTS_DIR" >>"$FAKE_LOG"
        [[ "$HARNESS_E2E_ARTIFACTS_DIR" != */attempt-1 ]]
    """,
}


class ExecutorTests(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.directory)
        self.root = self.directory / "checkout"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy(ROOT / "scripts/executor.sh", self.root / "scripts")
        for name, body in FAKE_SCRIPTS.items():
            (self.root / "scripts" / name).write_text(textwrap.dedent(body))
        self.runner = self.directory / "harness-e2e"
        self.runner.write_text('#!/usr/bin/env bash\necho "runner $*" >>"$FAKE_LOG"\n'
                               'echo \'{"campaigns":[{"campaign_id":"pr-r01"}]}\'\n')
        self.runner.chmod(0o755)
        self.log = self.directory / "log"
        self.log.touch()

    def executor(self, *args):
        environment = {**os.environ, "FAKE_LOG": str(self.log), "FAKE_RUNNER": str(self.runner),
                       "EXECUTION_KEY": "42", "DISPATCH_SUITE": "pr", "TMPDIR": str(self.directory)}
        # Run from elsewhere: the phases work from the checkout's root.
        return subprocess.run(["bash", str(self.root / "scripts/executor.sh"), *args], cwd=self.directory,
                              env=environment, capture_output=True, text=True)

    def test_preparation_materializes_with_the_stacks_runner_then_assembles_with_one_more_try(self):
        result = self.executor("prepare")
        self.assertEqual(result.returncode, 0, result.stderr)
        contract_dir = "--contract-dir target/harness-e2e-contract"
        assembly = f"{self.root}/target/harness-e2e-assembly"
        self.assertEqual([line.split(" --work-dir")[0] for line in self.log.read_text().splitlines()], [
            f"prepare dispatch {contract_dir}",
            f"prepare runtime {contract_dir}",
            f"prepare runner {contract_dir}",
            "runner test-plan materialize --profile pr",
            f"prepare contracts {contract_dir} --execution-key 42 --oidc-audience release-control-harness-e2e",
            f"group target/harness-e2e-contract/contracts/pr-r01.json case-minimal-path 1 {assembly}/attempt-1",
            f"group target/harness-e2e-contract/contracts/pr-r01.json case-minimal-path 1 {assembly}/attempt-2",
            f"prepare lock {contract_dir} --assembled {assembly}/attempt-2/stack",
            "campaign validate --contract target/harness-e2e-contract/contracts/pr-r01.json",
        ])
        self.assertIn("::warning::stack assembly attempt 1 failed", result.stdout)
        suite = (self.root / "target/harness-e2e-contract/suite.json").read_text()
        self.assertEqual((self.root / "target/harness-e2e-contract/profile.json").read_text(), suite)

    def test_finalize_aggregates_every_campaign_into_one_summary(self):
        # Nothing to aggregate is a failed finalizer, not an empty summary.
        self.assertNotEqual(self.executor("finalize").returncode, 0)
        for campaign in ("pr-r01", "smoke-r01"):
            (self.root / "target/harness-e2e-campaign" / campaign / "groups").mkdir(parents=True)
        result = self.executor("finalize")
        self.assertEqual(result.returncode, 0, result.stderr)
        aggregated = [line for line in self.log.read_text().splitlines() if line.startswith("aggregate")]
        self.assertEqual(len(aggregated), 2)
        self.assertIn(f"--e2e-bin {self.runner}", aggregated[0])
        self.assertIn("--execution-id 42", aggregated[0])
        summary = json.loads((self.root / "target/harness-e2e-campaign/execution-summary.json").read_text())
        self.assertEqual(summary, {"campaigns": [{"campaign": "pr-r01"}, {"campaign": "smoke-r01"}]})

    def test_an_unknown_phase_says_how_to_call_it(self):
        result = self.executor("assemble")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: executor.sh", result.stderr)


if __name__ == "__main__":
    unittest.main()
