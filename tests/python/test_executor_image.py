"""The executor: one image of tools, the checkout's scripts mounted into it.

`scripts/run_in_image.sh` is the host side every runner shares (GitHub's jobs
and the Console's Docker executions); `scripts/executor.sh` is what each
phase does inside the image.
"""

import hashlib
import json
import os
import shutil
import signal
import subprocess
import tempfile
import textwrap
import time
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TAG = "ghcr.io/iii-hq/harness-e2e:tools-" + hashlib.sha256((ROOT / "Dockerfile").read_bytes()).hexdigest()[:12]
DIGEST = "ghcr.io/iii-hq/harness-e2e@sha256:" + "d" * 64

# Logs each call, one argument per line and a blank line after it. `pull`
# succeeds when FAKE_PUBLISHED is set; `image inspect` finds the image once it
# was pulled or built, with a digest either way, as the containerd image store
# gives a local build one too. `build` fails with FAKE_BUILD_EXIT. `run`
# writes its --cidfile and exits with FAKE_RUN_EXIT; FAKE_RUN_EXIT=125 is a
# container that never started (no cid), and FAKE_RUN_BLOCKS keeps it
# running until `docker stop` names its cid.
FAKE_DOCKER = textwrap.dedent("""\
    #!/usr/bin/env bash
    printf '%s\\n' "$@" '' >>"$FAKE_LOG"
    case "$1 ${2:-}" in
      "pull "*) [[ -n "${FAKE_PUBLISHED:-}" ]] && touch "$FAKE_STATE/present" ;;
      "build "*) cat >/dev/null; [[ -z "${FAKE_BUILD_EXIT:-}" ]] || exit "$FAKE_BUILD_EXIT"; touch "$FAKE_STATE/present" ;;
      "image inspect") [[ -f "$FAKE_STATE/present" ]] && printf '%s\\n' "$FAKE_DIGEST" ;;
      "run "*)
        [[ "${FAKE_RUN_EXIT:-0}" != 125 ]] || exit 125
        while [[ "$1" != --cidfile ]]; do shift; done
        printf 'cid-4f2a' >"$2"
        if [[ -n "${FAKE_RUN_BLOCKS:-}" ]]; then
          printf '%s' "$$" >"$FAKE_STATE/run.pid"
          exec sleep 30
        fi
        exit "${FAKE_RUN_EXIT:-0}"
        ;;
      "stop "*) [[ "${!#}" == cid-4f2a ]] && kill "$(<"$FAKE_STATE/run.pid")" ;;
    esac
""")


def calls(log: Path) -> list[list[str]]:
    return [block.split("\n") for block in log.read_text().split("\n\n") if block]


def pairs(call: list[str]) -> set[tuple[str, str]]:
    return {(call[i], call[i + 1]) for i in range(len(call) - 1)}


class WrapperTests(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.directory)
        self.root = self.directory / "checkout"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy(ROOT / "scripts/run_in_image.sh", self.root / "scripts")
        shutil.copy(ROOT / "Dockerfile", self.root)
        (self.directory / "bin").mkdir()
        docker = self.directory / "bin/docker"
        docker.write_text(FAKE_DOCKER)
        docker.chmod(0o755)
        (self.directory / "state").mkdir()
        (self.directory / "tmp").mkdir()
        self.socket = self.directory / "docker.sock"
        self.socket.touch()
        self.log = self.directory / "docker.log"
        self.artifacts = self.root / "target/harness-e2e-exact-stack"

    def environment(self, published=True, env=None):
        return {
            "PATH": f"{self.directory / 'bin'}:{os.environ['PATH']}", "HOME": str(self.directory),
            "FAKE_LOG": str(self.log), "FAKE_STATE": str(self.directory / "state"),
            "FAKE_DIGEST": DIGEST, "DOCKER_HOST": f"unix://{self.socket}", "TMPDIR": str(self.directory / "tmp"),
            **({"FAKE_PUBLISHED": "1"} if published else {}), **(env or {}),
        }

    def run_wrapper(self, *args, published=True, env=None, status=0):
        result = subprocess.run(["bash", str(self.root / "scripts/run_in_image.sh"), *args],
                                env=self.environment(published, env), capture_output=True, text=True)
        self.assertEqual(result.returncode, status, result.stderr)
        return result, calls(self.log) if self.log.exists() else []

    def test_a_group_runs_in_the_published_image_with_the_checkout_mounted_where_it_is(self):
        root = self.root.resolve()
        result, invoked = self.run_wrapper("group", env={
            "HARNESS_E2E_CONTRACT": "target/contract.json", "HARNESS_E2E_CAMPAIGN_GROUP_ID": "case-minimal-path",
            "DEEPSEEK_API_KEY": "secret-value", "EXECUTION_KEY": "42", "CI": "true", "GITHUB_TOKEN": "ghs_token",
            "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "url.x.insteadOf", "GIT_CONFIG_VALUE_0": "y",
            "GIT_CONFIG_GLOBAL": "/host/gitconfig", "UNRELATED": "1", "HARNESS_E2E_EXECUTOR_IMAGE": "stale",
        })
        self.assertEqual(invoked[0], ["pull", "--quiet", TAG])
        run = next(call for call in invoked if call[0] == "run")
        self.assertEqual(run[-4:], [TAG, "bash", "scripts/executor.sh", "group"])
        options = pairs(run[:-4])
        tmp = next(value for flag, value in options if flag == "--env" and value.startswith("TMPDIR="))[7:]
        self.assertTrue(tmp.startswith(f"{self.directory / 'tmp'}/harness-e2e-executor."))
        self.assertFalse(Path(tmp).exists(), "the phase's TMPDIR is removed afterwards")
        for pair in [
            ("--user", f"{os.getuid()}:{os.getgid()}"),
            ("--security-opt", "no-new-privileges"),
            ("--group-add", str(self.socket.stat().st_gid)),
            ("--volume", f"{self.socket}:/var/run/docker.sock"),
            ("--volume", f"{root}:{root}"),
            ("--workdir", str(root)),
            ("--volume", f"{tmp}:{tmp}"),
            ("--env", f"HARNESS_E2E_EXECUTOR_IMAGE={DIGEST}"),
            ("--cidfile", f"{tmp}.cid"),
            ("--label", "harness-e2e.execution=42"),
            ("--label", "harness-e2e.phase=group"),
            ("--label", "harness-e2e.group=case-minimal-path"),
        ]:
            self.assertIn(pair, options)
        passed = sorted(value for flag, value in options if flag == "--env" and "=" not in value)
        # By name only: the values never reach the command line. The subject
        # has a shell in a group, so no GitHub token enters it.
        self.assertEqual(passed, ["CI", "DEEPSEEK_API_KEY", "EXECUTION_KEY", "GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0",
                                  "GIT_CONFIG_VALUE_0", "HARNESS_E2E_CAMPAIGN_GROUP_ID", "HARNESS_E2E_CONTRACT"])
        self.assertNotIn("secret-value", "\n".join(run))
        self.assertNotIn("--network", run)
        self.assertNotIn("--env-file", run)
        self.assertNotIn("not published", result.stderr)
        self.assertFalse(Path(f"{tmp}.cid").exists())

    def test_only_prepare_gets_the_github_token_and_only_a_group_the_docker_socket(self):
        _, invoked = self.run_wrapper("prepare", "materialize", env={"GITHUB_TOKEN": "ghs_token"})
        run = next(call for call in invoked if call[0] == "run")
        self.assertIn(("--env", "GITHUB_TOKEN"), pairs(run))
        self.assertNotIn("--group-add", run)
        self.assertFalse(any(value.endswith(":/var/run/docker.sock") for value in run))
        self.log.unlink()
        _, invoked = self.run_wrapper("finalize", env={"GITHUB_TOKEN": "ghs_token"})
        run = next(call for call in invoked if call[0] == "run")
        self.assertNotIn(("--env", "GITHUB_TOKEN"), pairs(run))
        self.assertFalse(any(value.endswith(":/var/run/docker.sock") for value in run))

    def test_an_unpublished_image_is_built_here_and_said_out_loud(self):
        result, invoked = self.run_wrapper("--env-file", "/secrets/providers.env", "prepare", "assemble",
                                           published=False)
        self.assertEqual([call[:2] for call in invoked[:3]],
                         [["pull", "--quiet"], ["image", "inspect"], ["build", "--tag"]])
        self.assertIn(f"::warning::{TAG} is not published; building it from {self.root.resolve()}/Dockerfile",
                      result.stderr)
        run = next(call for call in invoked if call[0] == "run")
        # Built here, it has no registry digest: the execution records the tag.
        self.assertIn(f"HARNESS_E2E_EXECUTOR_IMAGE={TAG}", run)
        self.assertEqual(run[run.index("--env-file") + 1], "/secrets/providers.env")
        self.assertEqual(run[-5:], [TAG, "bash", "scripts/executor.sh", "prepare", "assemble"])

    def test_an_image_that_cannot_be_had_fails_the_group_as_infrastructure(self):
        _, invoked = self.run_wrapper("group", published=False, status=7, env={
            "FAKE_BUILD_EXIT": "7", "HARNESS_E2E_ARTIFACTS_DIR": str(self.artifacts)})
        self.assertFalse(any(call[0] == "run" for call in invoked))
        failure = json.loads((self.artifacts / "failure.json").read_text())
        self.assertEqual(failure, {"phase": "executor_image", "outcome": "infra_failed",
                                   "error": f"could not pull or build {TAG}", "exit_code": 7})

    def test_a_container_that_never_started_fails_the_group_as_infrastructure(self):
        self.run_wrapper("group", status=125, env={
            "FAKE_RUN_EXIT": "125", "HARNESS_E2E_ARTIFACTS_DIR": str(self.artifacts)})
        failure = json.loads((self.artifacts / "failure.json").read_text())
        self.assertEqual((failure["phase"], failure["outcome"], failure["exit_code"]),
                         ("executor_start", "infra_failed", 125))

    def test_a_phase_that_fails_in_its_container_fails_the_wrapper_and_keeps_its_own_account(self):
        self.artifacts.mkdir(parents=True)
        (self.artifacts / "failure.json").write_text('{"phase":"execution"}')
        self.run_wrapper("group", status=3, env={
            "FAKE_RUN_EXIT": "3", "HARNESS_E2E_ARTIFACTS_DIR": str(self.artifacts)})
        self.assertEqual(json.loads((self.artifacts / "failure.json").read_text()), {"phase": "execution"})
        # A phase with no artifact directory writes none.
        self.run_wrapper("finalize", status=3, env={"FAKE_RUN_EXIT": "3"})

    def test_an_interrupted_wrapper_stops_its_container(self):
        wrapper = subprocess.Popen(["bash", str(self.root / "scripts/run_in_image.sh"), "group"],
                                   env=self.environment(env={"FAKE_RUN_BLOCKS": "1"}),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        started = self.directory / "state/run.pid"
        deadline = time.monotonic() + 10
        while not started.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertTrue(started.exists(), wrapper.stderr.read() if wrapper.poll() is not None else "not started")
        wrapper.send_signal(signal.SIGTERM)
        self.assertEqual(wrapper.wait(timeout=10), 143)
        wrapper.stdout.close()
        wrapper.stderr.close()
        self.assertIn(["stop", "--time", "30", "cid-4f2a"], calls(self.log))

    def test_a_host_that_runs_one_phase_at_a_time_may_put_it_on_its_network(self):
        _, invoked = self.run_wrapper("group", env={"HARNESS_E2E_DOCKER_NETWORK": "host"})
        run = next(call for call in invoked if call[0] == "run")
        self.assertEqual(run[run.index("--network") + 1], "host")
        # The wrapper's own setting, not the phase's.
        self.assertNotIn("HARNESS_E2E_DOCKER_NETWORK", run)

    def test_the_image_is_named_by_the_dockerfile(self):
        result, invoked = self.run_wrapper("image")
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

    def executor(self, *args, env=None):
        environment = {**os.environ, "FAKE_LOG": str(self.log), "FAKE_RUNNER": str(self.runner),
                       "EXECUTION_KEY": "42", "DISPATCH_SUITE": "pr", "TMPDIR": str(self.directory), **(env or {})}
        environment.pop("GIT_CONFIG_COUNT", None)
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
        self.assertNotEqual(self.executor("finalize", "aggregate").returncode, 0)
        for campaign in ("pr-r01", "smoke-r01"):
            (self.root / "target/harness-e2e-campaign" / campaign / "groups").mkdir(parents=True)
        result = self.executor("finalize", "aggregate")
        self.assertEqual(result.returncode, 0, result.stderr)
        aggregated = [line for line in self.log.read_text().splitlines() if line.startswith("aggregate")]
        self.assertEqual(len(aggregated), 2)
        self.assertIn(f"--e2e-bin {self.runner}", aggregated[0])
        self.assertIn("--execution-id 42", aggregated[0])
        summary = json.loads((self.root / "target/harness-e2e-campaign/execution-summary.json").read_text())
        self.assertEqual(summary, {"campaigns": [{"campaign": "pr-r01"}, {"campaign": "smoke-r01"}]})

    def test_fixtures_check_out_what_the_groups_start_from_with_a_token_only_for_private_sources(self):
        contracts = self.root / "target/harness-e2e-contract/contracts"
        contracts.mkdir(parents=True)
        # Each group once, in order, whatever the matrix repeats.
        groups = ["case-minimal-path", "case-registry-implementation", "case-trending-topics-build",
                  "case-linkly-tutorial", "case-kanban-board", "case-registry-implementation"]
        (contracts / "resolution.json").write_text(json.dumps({
            "matrix": {"include": [{"group_id": group} for group in groups]},
            "template": {"id": "harness", "revision": "a" * 40}}))
        # Logs each call and whether it carried credentials; a fetch leaves
        # the commit it fetched as the checkout's HEAD.
        git = self.directory / "bin/git"
        git.parent.mkdir()
        git.write_text(textwrap.dedent("""\
            #!/usr/bin/env bash
            printf 'git %s|%s\\n' "$*" "${GIT_CONFIG_VALUE_0:+$GIT_CONFIG_KEY_0}" >>"$FAKE_LOG"
            case "$1" in
              init) mkdir -p "$3/.git" ;;
              clone) mkdir -p "${@: -1}/.git" ;;
              -C)
                case "$3" in
                  fetch) printf '%s' "${@: -1}" >"$2/.git/fetched" ;;
                  rev-parse) cat "$2/.git/fetched" ;;
                esac
                ;;
            esac
        """))
        git.chmod(0o755)
        environment = {"PATH": f"{git.parent}:{os.environ['PATH']}", "GITHUB_TOKEN": "ghs_fixture"}
        result = self.executor("prepare", "fixtures", env=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        header = "http.https://github.com/.extraheader"
        registry = "662eb87c1bdbb395f36264d5d26bf823e2ace783"
        trending = "3ee24f7ace3c014db35423f14939ad3f6ce0c3d2"
        linkly = "ba1dfd95d4f4120705c8b0cc95d9a2ef86a0290d"
        self.assertEqual(self.log.read_text().splitlines(), [
            "git clone -q --branch main https://github.com/iii-hq/kanban-e2e-fixture.git target/kanban-fixture|",
            "git init -q target/linkly-templates|",
            f"git -C target/linkly-templates fetch -q --depth 1 https://github.com/iii-hq/templates.git {linkly}|",
            "git -C target/linkly-templates checkout -q --detach FETCH_HEAD|",
            "git init -q target/registry-sources/registry|" + header,
            f"git -C target/registry-sources/registry fetch -q --depth 1 https://github.com/iii-hq/registry.git {registry}|{header}",
            "git -C target/registry-sources/registry checkout -q --detach FETCH_HEAD|" + header,
            "git clone -q --depth 1 https://github.com/iii-hq/e2e-fixture.git target/registry-sources/e2e-fixture|" + header,
            "git init -q target/trending-topics-fixture|" + header,
            f"git -C target/trending-topics-fixture fetch -q --depth 1 https://github.com/iii-hq/e2e-fixture.git {trending}|{header}",
            "git -C target/trending-topics-fixture checkout -q --detach FETCH_HEAD|" + header,
            "git init -q target/execution-template|",
            f"git -C target/execution-template fetch -q --depth 1 https://github.com/iii-hq/templates.git {'a' * 40}|",
            "git -C target/execution-template checkout -q --detach FETCH_HEAD|",
        ])
        self.assertNotIn("ghs_fixture", self.log.read_text())
        # Again, for one group: a commit already checked out stays, a branch
        # is checked out anew.
        self.log.write_text("")
        result = self.executor("prepare", "fixtures", env={
            **environment, "HARNESS_E2E_CAMPAIGN_GROUP_ID": "case-registry-implementation"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([line for line in self.log.read_text().splitlines() if "rev-parse" not in line], [
            "git clone -q --depth 1 https://github.com/iii-hq/e2e-fixture.git target/registry-sources/e2e-fixture|" + header,
        ])

    def test_a_checkout_that_fails_is_tried_again(self):
        contracts = self.root / "target/harness-e2e-contract/contracts"
        contracts.mkdir(parents=True)
        (contracts / "resolution.json").write_text(json.dumps({
            "matrix": {"include": [{"group_id": "case-linkly-tutorial"}]}}))
        # The first fetch answers a 5xx; the next one works.
        git = self.directory / "bin/git"
        git.parent.mkdir()
        git.write_text(textwrap.dedent("""\
            #!/usr/bin/env bash
            printf 'git %s\\n' "$*" >>"$FAKE_LOG"
            case "$1" in
              init) mkdir -p "$3/.git" ;;
              -C)
                if [[ "$3" == fetch && ! -f "$FAKE_STATE_FAILED" ]]; then
                  touch "$FAKE_STATE_FAILED"
                  echo "error: RPC failed; HTTP 502" >&2
                  exit 128
                fi
                ;;
            esac
        """))
        git.chmod(0o755)
        result = self.executor("prepare", "fixtures", env={
            "PATH": f"{git.parent}:{os.environ['PATH']}",
            "FAKE_STATE_FAILED": str(self.directory / "failed-once")})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("checking out iii-hq/templates failed (try 1 of 3)", result.stderr)
        fetches = [line for line in self.log.read_text().splitlines() if " fetch " in line]
        self.assertEqual(len(fetches), 2)

    def test_a_group_clones_its_fixture_repositories_from_the_checkouts(self):
        (self.root / "scripts/run_exact_stack_group.sh").write_text(
            'env | grep ^GIT_CONFIG_ | sort >>"$FAKE_LOG"\n')
        for group, expected in (
            ("case-registry-implementation", [
                "GIT_CONFIG_COUNT=2",
                f"GIT_CONFIG_KEY_0=url.file://{self.root}/target/registry-sources/registry.insteadOf",
                f"GIT_CONFIG_KEY_1=url.file://{self.root}/target/registry-sources/e2e-fixture.insteadOf",
                "GIT_CONFIG_VALUE_0=https://github.com/iii-hq/registry.git",
                "GIT_CONFIG_VALUE_1=https://github.com/iii-hq/e2e-fixture.git",
            ]),
            ("case-trending-topics-build", [
                "GIT_CONFIG_COUNT=1",
                f"GIT_CONFIG_KEY_0=url.file://{self.root}/target/trending-topics-fixture.insteadOf",
                "GIT_CONFIG_VALUE_0=git@github.com:iii-hq/e2e-fixture.git",
            ]),
            ("case-minimal-path", []),
        ):
            with self.subTest(group=group):
                self.log.write_text("")
                result = self.executor("group", env={"HARNESS_E2E_CAMPAIGN_GROUP_ID": group})
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.log.read_text().splitlines(), expected)

    def test_package_hashes_each_root_beside_its_contract(self):
        result = self.executor("package", '{"job":"group"}', "target/a", "target/b")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), [
            f"campaign package --root target/{root} --contract target/{root}/stack-lock.json "
            f'--workflow {{"job":"group"}} --output target/{root}/bundle-manifest.json'
            for root in ("a", "b")
        ])
        self.assertEqual(self.executor("package", '{"job":"group"}').returncode, 2)

    def test_an_unknown_phase_says_how_to_call_it(self):
        result = self.executor("assemble")
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: executor.sh", result.stderr)


if __name__ == "__main__":
    unittest.main()
