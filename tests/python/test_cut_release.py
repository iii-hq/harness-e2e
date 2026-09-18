import importlib.util
import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("cut_release", ROOT / "scripts/cut_release.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class VersionTests(unittest.TestCase):
    def test_a_release_version_round_trips_through_its_tag(self):
        for text in ("0.11.11-experimental", "1.0.0", "0.0.1-experimental"):
            version = MODULE.parse_version(text)
            self.assertEqual(str(version), text)
            self.assertEqual(version.tag, f"harness-e2e/v{text}")
            self.assertEqual(version.branch, f"release/harness-e2e-v{text}")

    def test_versions_the_release_workflow_would_reject_are_rejected_here(self):
        for text in ("0.11", "v0.11.1", "0.11.1-rc.1", "01.2.3", "0.11.1-Experimental", ""):
            with self.subTest(text=text), self.assertRaises(ValueError):
                MODULE.parse_version(text)

    def test_tags_outside_this_scheme_are_ignored_rather_than_failing(self):
        self.assertIsNone(MODULE.parse_tag("v1.2.3"))
        self.assertIsNone(MODULE.parse_tag("harness-e2e/v1.2"))
        self.assertIsNone(MODULE.parse_tag("other-worker/v1.2.3"))
        self.assertEqual(str(MODULE.parse_tag("harness-e2e/v1.2.3")), "1.2.3")

    def test_latest_is_chosen_by_semver_and_not_by_string_order(self):
        tags = [
            "harness-e2e/v0.11.9-experimental",
            "harness-e2e/v0.11.10-experimental",
            "harness-e2e/v0.11.11-experimental",
            "harness-e2e/v0.9.5-experimental",
            "some-other-tag",
        ]
        # Lexically "0.11.9" sorts above "0.11.10"; semver does not.
        self.assertEqual(str(MODULE.latest_version(tags)), "0.11.11-experimental")
        self.assertIsNone(MODULE.latest_version(["nothing", "harness-e2e/vbad"]))

    def test_a_stable_release_outranks_the_experimental_of_the_same_core(self):
        tags = ["harness-e2e/v0.11.11-experimental", "harness-e2e/v0.11.11"]
        self.assertEqual(str(MODULE.latest_version(tags)), "0.11.11")

    def test_each_bump_moves_the_part_it_names_and_zeroes_the_rest(self):
        current = MODULE.parse_version("0.11.11-experimental")
        self.assertEqual(str(MODULE.next_version(current, "patch", "experimental")), "0.11.12-experimental")
        self.assertEqual(str(MODULE.next_version(current, "minor", "experimental")), "0.12.0-experimental")
        self.assertEqual(str(MODULE.next_version(current, "major", "experimental")), "1.0.0-experimental")

    def test_the_channel_is_chosen_independently_of_the_bump(self):
        current = MODULE.parse_version("0.11.11-experimental")
        self.assertEqual(str(MODULE.next_version(current, "patch", "stable")), "0.11.12")
        stable = MODULE.parse_version("1.0.0")
        self.assertEqual(str(MODULE.next_version(stable, "patch", "experimental")), "1.0.1-experimental")

    def test_the_first_release_starts_the_line_the_bump_names(self):
        self.assertEqual(str(MODULE.next_version(None, "patch", "experimental")), "0.0.1-experimental")
        self.assertEqual(str(MODULE.next_version(None, "minor", "experimental")), "0.1.0-experimental")
        self.assertEqual(str(MODULE.next_version(None, "major", "stable")), "1.0.0")

    def test_an_unknown_bump_or_channel_is_refused(self):
        current = MODULE.parse_version("0.1.0")
        with self.assertRaises(ValueError):
            MODULE.next_version(current, "rebuild", "experimental")
        with self.assertRaises(ValueError):
            MODULE.next_version(current, "patch", "nightly")


class ManifestStampTests(unittest.TestCase):
    CARGO_TOML = (
        '[package]\n'
        'name = "harness-e2e"\n'
        'version = "0.8.1-experimental"\n'
        'edition = "2021"\n'
        '\n'
        '[dependencies]\n'
        'tokio = { version = "1", features = ["macros"] }\n'
        'serde = { version = "1" }\n'
    )

    def test_only_the_package_table_version_is_rewritten(self):
        updated = MODULE.set_cargo_toml_version(self.CARGO_TOML, "0.11.12-experimental")
        self.assertIn('version = "0.11.12-experimental"\n', updated)
        # A dependency that happens to carry a version is left alone.
        self.assertIn('tokio = { version = "1", features = ["macros"] }', updated)
        self.assertNotIn("0.8.1-experimental", updated)

    def test_a_manifest_without_a_package_version_is_refused(self):
        with self.assertRaises(ValueError):
            MODULE.set_cargo_toml_version('[dependencies]\nserde = "1"\n', "1.0.0")

    def test_only_the_worker_entry_of_the_lock_is_rewritten(self):
        lock = (
            '[[package]]\n'
            'name = "harness-e2e"\n'
            'version = "0.8.1-experimental"\n'
            'dependencies = [\n "quinn",\n]\n'
            '\n'
            '[[package]]\n'
            'name = "quinn"\n'
            'version = "0.11.11"\n'
        )
        updated = MODULE.set_cargo_lock_version(lock, "0.11.12-experimental")
        self.assertIn('name = "harness-e2e"\nversion = "0.11.12-experimental"\n', updated)
        # quinn 0.11.11 shares the shape of a release version; it must not move.
        self.assertIn('name = "quinn"\nversion = "0.11.11"\n', updated)

    def test_a_lock_without_the_worker_is_refused(self):
        with self.assertRaises(ValueError):
            MODULE.set_cargo_lock_version('[[package]]\nname = "quinn"\nversion = "0.11.11"\n', "1.0.0")

    def test_the_real_manifests_take_a_stamp_and_stay_parseable(self):
        import tomllib

        stamped = MODULE.set_cargo_toml_version(
            (ROOT / "Cargo.toml").read_text(encoding="utf-8"), "9.9.9-experimental"
        )
        self.assertEqual(tomllib.loads(stamped)["package"]["version"], "9.9.9-experimental")
        lock = MODULE.set_cargo_lock_version(
            (ROOT / "Cargo.lock").read_text(encoding="utf-8"), "9.9.9-experimental"
        )
        entries = tomllib.loads(lock)["package"]
        stamped_entries = [entry for entry in entries if entry["name"] == "harness-e2e"]
        self.assertEqual([entry["version"] for entry in stamped_entries], ["9.9.9-experimental"])
        # Every other locked package keeps the version it had.
        original = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))["package"]
        moved = [
            entry["name"]
            for entry, before in zip(entries, original)
            if entry["version"] != before["version"]
        ]
        self.assertEqual(moved, ["harness-e2e"])


class ResolveAgainstGitTests(unittest.TestCase):
    """`resolve` reads the tags of a real repository, so exercise it on one."""

    def _repository(self, directory: str, tags: list[str]) -> pathlib.Path:
        root = pathlib.Path(directory)
        run = lambda *args: subprocess.run(
            ["git", "-C", str(root), *args], check=True, capture_output=True, text=True
        )
        run("init", "--quiet", "--initial-branch", "main")
        run("config", "user.email", "test@example.com")
        run("config", "user.name", "Test")
        (root / "README.md").write_text("fixture\n", encoding="utf-8")
        run("add", "README.md")
        run("commit", "--quiet", "-m", "initial")
        for tag in tags:
            run("tag", tag)
        return root

    def _resolve(self, root: pathlib.Path, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys_executable(), str(ROOT / "scripts/cut_release.py"), "--root", str(root), "resolve", *args],
            capture_output=True,
            text=True,
        )

    def test_resolve_names_the_next_tag_branch_and_source(self):
        import json

        with tempfile.TemporaryDirectory() as directory:
            root = self._repository(directory, ["harness-e2e/v0.11.10-experimental", "harness-e2e/v0.11.11-experimental"])
            result = self._resolve(root, "--bump", "patch")
            self.assertEqual(result.returncode, 0, result.stderr)
            payload = json.loads(result.stdout)
            self.assertEqual(payload["current_version"], "0.11.11-experimental")
            self.assertEqual(payload["version"], "0.11.12-experimental")
            self.assertEqual(payload["tag"], "harness-e2e/v0.11.12-experimental")
            self.assertEqual(payload["branch"], "release/harness-e2e-v0.11.12-experimental")
            self.assertEqual(payload["source_tag"], "harness-e2e/v0.11.11-experimental")
            self.assertRegex(payload["source_commit"], r"^[0-9a-f]{40}$")

    def test_resolve_counts_a_release_cut_in_the_meantime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._repository(
                directory,
                ["harness-e2e/v0.11.11-experimental", "harness-e2e/v0.11.12-experimental"],
            )
            # The operator dispatched believing 0.11.11 was newest; someone cut
            # 0.11.12 first. Without the guard this would silently become
            # 0.11.13 on top of a release they have not seen.
            result = self._resolve(root, "--bump", "patch", "--expected-current", "0.11.11-experimental")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("the tags say 0.11.12-experimental", result.stderr)
            # Left blank, the same dispatch proceeds from the real newest tag.
            allowed = self._resolve(root, "--bump", "patch")
            self.assertEqual(allowed.returncode, 0, allowed.stderr)
            self.assertIn("0.11.13-experimental", allowed.stdout)

    def test_resolve_refuses_when_the_operator_expected_another_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._repository(directory, ["harness-e2e/v0.11.11-experimental"])
            result = self._resolve(root, "--bump", "patch", "--expected-current", "0.11.9-experimental")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("expected current release", result.stderr)

    def test_resolve_writes_the_github_outputs_the_workflow_reads(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self._repository(directory, ["harness-e2e/v0.11.11-experimental"])
            output = pathlib.Path(directory) / "out.txt"
            result = self._resolve(root, "--bump", "minor", "--github-output", str(output))
            self.assertEqual(result.returncode, 0, result.stderr)
            written = dict(
                line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines()
            )
            self.assertEqual(written["version"], "0.12.0-experimental")
            self.assertEqual(written["tag"], "harness-e2e/v0.12.0-experimental")


def sys_executable() -> str:
    import sys

    return sys.executable


if __name__ == "__main__":
    unittest.main()
