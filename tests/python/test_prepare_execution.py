"""What `iii: latest` resolves to when the newest candidate is not out yet."""
import contextlib
import importlib.util
import io
import sys
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("prepare_execution", ROOT / "scripts" / "prepare_execution.py")
prepare_execution = importlib.util.module_from_spec(SPEC)
sys.modules["prepare_execution"] = prepare_execution
SPEC.loader.exec_module(prepare_execution)

DIGEST = "sha256:" + "c" * 64
ARCHIVE = {"name": prepare_execution.CLI_ASSET, "digest": DIGEST}


class LatestCliTests(unittest.TestCase):
    def resolve(self, releases, selector="latest"):
        """`releases` maps a tag to its assets, or None for a tag whose
        release GitHub does not have (404)."""
        asked = []

        def get(url, token=None):
            if url.endswith("/git/matching-refs/tags/iii/v"):
                return [{"ref": f"refs/tags/iii/v{tag}"} for tag in ["0.24.1", *releases]]
            tag = url.rsplit("/iii/v", 1)[1]
            asked.append(tag)
            if releases.get(tag) is None:
                raise prepare_execution.ResolutionError(f"{url} answered HTTP 404") from urllib.error.HTTPError(
                    url, 404, "Not Found", None, None)
            return {"assets": releases[tag]}

        said = io.StringIO()
        with patch.object(prepare_execution, "get_json", side_effect=get), contextlib.redirect_stdout(said):
            cli = prepare_execution.resolve_cli(selector, None)
        return cli, asked, said.getvalue()

    def test_the_newest_candidate_runs_when_it_has_the_archive(self):
        cli, asked, said = self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": [ARCHIVE]})
        self.assertEqual((cli["version"], cli["sha256"]), ("0.24.4-rc.2", DIGEST))
        self.assertEqual(asked, ["0.24.4-rc.2"])
        self.assertEqual(said, "")

    def test_a_candidate_still_building_or_unreleased_gives_way_to_the_one_before_it_said_out_loud(self):
        windows = {"name": "iii-x86_64-pc-windows-msvc.zip", "digest": DIGEST}
        for newest in ([windows], [], None):
            with self.subTest(newest=newest):
                cli, asked, said = self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": newest})
                self.assertEqual(cli["version"], "0.24.4-rc.1")
                self.assertEqual(asked, ["0.24.4-rc.2", "0.24.4-rc.1"])
                self.assertEqual(said, f"::warning::iii/v0.24.4-rc.2 publishes no {prepare_execution.CLI_ASSET} yet; "
                                       "iii: latest runs iii/v0.24.4-rc.1, the newest release candidate that does\n")

    def test_integrity_and_a_named_version_still_fail(self):
        with self.assertRaisesRegex(prepare_execution.ResolutionError, "no iii-hq/iii release candidate publishes"):
            self.resolve({"0.24.4-rc.1": [], "0.24.4-rc.2": []})
        # A digest is what the groups check the download against.
        with self.assertRaisesRegex(prepare_execution.ResolutionError, "digest"):
            self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": [{"name": prepare_execution.CLI_ASSET}]})
        # A version the stack names is that version or nothing.
        with self.assertRaisesRegex(prepare_execution.ResolutionError, "iii/v0.24.4-rc.2 publishes no"):
            self.resolve({"0.24.4-rc.1": [ARCHIVE], "0.24.4-rc.2": []}, selector="0.24.4-rc.2")


if __name__ == "__main__":
    unittest.main()
