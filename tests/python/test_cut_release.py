import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "scripts"))

import cut_release  # noqa: E402


class CutReleaseTests(unittest.TestCase):
    def test_next_version_is_plain_semver_after_the_experimental_line(self):
        current = cut_release.latest_version(
            ["harness-e2e/v0.11.19-experimental", "harness-e2e/v0.11.18-experimental", "v9.9.9"]
        )
        self.assertEqual(str(current), "0.11.19-experimental")
        self.assertEqual(str(cut_release.next_version(current, "patch")), "0.11.20")
        self.assertEqual(str(cut_release.next_version(current, "minor")), "0.12.0")
        self.assertEqual(str(cut_release.next_version(current, "major")), "1.0.0")
        self.assertEqual(cut_release.next_version(current, "patch").tag, "harness-e2e/v0.11.20")

    def test_a_stable_release_outranks_its_experimental_twin(self):
        current = cut_release.latest_version(
            ["harness-e2e/v0.11.20", "harness-e2e/v0.11.20-experimental"]
        )
        self.assertEqual(str(current), "0.11.20")
        self.assertEqual(str(cut_release.next_version(None, "minor")), "0.1.0")


if __name__ == "__main__":
    unittest.main()
