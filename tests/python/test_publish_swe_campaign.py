import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from publish_swe_service import ALLOWED_REPOSITORY, PublishError
from publish_swe_campaign import main


def native_report(run_id, *, mode="journey"):
    return {
        "schema": "swe-service-report/v1",
        "scenario_id": "swe_service_journey" if mode == "journey" else "swe_config_isolation",
        "mode": mode,
        "run_id": run_id,
    }


class CampaignPublicationTests(unittest.TestCase):
    def invoke(self, reports, output, execution_id="campaign-123"):
        with contextlib.redirect_stdout(io.StringIO()):
            return main(
                [
                    "--reports-dir",
                    str(reports),
                    "--execution-id",
                    execution_id,
                    "--run-url",
                    "https://harness.test/runs/campaign-123",
                    "--output",
                    str(output),
                ]
            )

    def write_report(self, root, relative, value):
        path = root / relative / "swe_service_report.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_empty_reports_directory_writes_skipped_receipt_without_publisher_access(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            reports.mkdir()
            output = root / "publication.json"
            with mock.patch("publish_swe_campaign.publish") as publisher:
                code = self.invoke(reports, output)

            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(code, 0)
            self.assertEqual(receipt["status"], "skipped")
            self.assertEqual(receipt["report_count"], 0)
            self.assertEqual(receipt["reports"], [])
            publisher.assert_not_called()

    def test_missing_directory_is_skipped_but_a_file_path_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            missing = root / "missing-deliverables"
            missing_output = root / "missing.json"
            not_directory = root / "not-a-directory"
            not_directory.write_text("occupied", encoding="utf-8")
            file_output = root / "file.json"
            with mock.patch("publish_swe_campaign.publish") as publisher:
                missing_code = self.invoke(missing, missing_output)
                file_code = self.invoke(not_directory, file_output)

            missing_receipt = json.loads(missing_output.read_text(encoding="utf-8"))
            file_receipt = json.loads(file_output.read_text(encoding="utf-8"))
            self.assertEqual(missing_code, 0)
            self.assertEqual(missing_receipt["status"], "skipped")
            self.assertEqual(missing_receipt["report_count"], 0)
            self.assertEqual(file_code, 1)
            self.assertEqual(file_receipt["status"], "failed")
            publisher.assert_not_called()

    def test_isolated_report_is_recorded_as_ignored_without_publisher_access(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            self.write_report(
                reports, "isolated", native_report("isolated-1", mode="isolated")
            )
            output = root / "publication.json"
            with mock.patch("publish_swe_campaign.publish") as publisher:
                code = self.invoke(reports, output)

            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(code, 0)
            self.assertEqual(receipt["status"], "skipped")
            self.assertEqual(receipt["journey_count"], 0)
            self.assertEqual(
                receipt["reports"],
                [
                    {
                        "path": "isolated/swe_service_report.json",
                        "status": "ignored",
                        "reason": "not a journey report",
                    }
                ],
            )
            publisher.assert_not_called()

    def test_partial_failure_persists_every_receipt_and_never_mutates_sources_or_secret_env(self):
        calls = []

        def fake_publish(**kwargs):
            calls.append(kwargs)
            if kwargs["execution_id"].endswith("run-b-002"):
                raise PublishError("temporary GitHub failure")
            return {
                "repository": ALLOWED_REPOSITORY,
                "branch": f"feat/swe-result-{kwargs['execution_id']}",
                "commit_sha": "c" * 40,
                "pull_request": "https://github.test/iii-hq/e2e-fixture/pull/1",
                "publication_digest": "sha256:" + "d" * 64,
                "accepted_tickets": [1],
                "terminal_status": "capability_failure",
                "reused": False,
            }

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            first = self.write_report(reports, "a", native_report("run-a"))
            second = self.write_report(reports, "b", native_report("run-b"))
            before = {first: first.read_bytes(), second: second.read_bytes()}
            output = root / "publication.json"
            with mock.patch.dict(
                os.environ, {"GH_TOKEN": "TOP-SECRET-TOKEN"}, clear=False
            ):
                with mock.patch("publish_swe_campaign.publish", side_effect=fake_publish):
                    code = self.invoke(reports, output)
                self.assertEqual(os.environ["GH_TOKEN"], "TOP-SECRET-TOKEN")

            receipt_bytes = output.read_bytes()
            receipt = json.loads(receipt_bytes)
            self.assertEqual(code, 1)
            self.assertEqual(receipt["status"], "partial_failure")
            self.assertEqual(
                [record["status"] for record in receipt["reports"]],
                ["published", "error"],
            )
            self.assertEqual(
                [call["execution_id"] for call in calls],
                ["campaign-123-run-a-001", "campaign-123-run-b-002"],
            )
            self.assertNotIn(b"TOP-SECRET-TOKEN", receipt_bytes)
            self.assertEqual(first.read_bytes(), before[first])
            self.assertEqual(second.read_bytes(), before[second])

    def test_one_journey_keeps_supplied_id_and_repeated_run_is_deterministic(self):
        calls = []

        def fake_publish(**kwargs):
            calls.append(kwargs)
            return {
                "repository": ALLOWED_REPOSITORY,
                "branch": f"feat/swe-result-{kwargs['execution_id']}",
                "commit_sha": "e" * 40,
                "pull_request": "https://github.test/iii-hq/e2e-fixture/pull/9",
                "publication_digest": "sha256:" + "f" * 64,
                "accepted_tickets": [],
                "terminal_status": "cancelled",
                "reused": len(calls) > 1,
            }

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            source = self.write_report(reports, "only", native_report("native-run"))
            source_before = source.read_bytes()
            first_output = root / "first.json"
            second_output = root / "second.json"
            with mock.patch("publish_swe_campaign.publish", side_effect=fake_publish):
                first_code = self.invoke(reports, first_output, "exact-execution")
                second_code = self.invoke(reports, second_output, "exact-execution")

            first_receipt = json.loads(first_output.read_text(encoding="utf-8"))
            second_receipt = json.loads(second_output.read_text(encoding="utf-8"))
            self.assertEqual((first_code, second_code), (0, 0))
            self.assertEqual(
                [call["execution_id"] for call in calls],
                ["exact-execution", "exact-execution"],
            )
            self.assertEqual(first_receipt["status"], "completed")
            self.assertEqual(second_receipt["status"], "completed")
            self.assertEqual(
                first_receipt["reports"][0]["execution_id"], "exact-execution"
            )
            self.assertEqual(
                second_receipt["reports"][0]["execution_id"], "exact-execution"
            )
            self.assertEqual(source.read_bytes(), source_before)

    def test_large_native_envelope_reaches_the_sanitizing_publisher(self):
        value = native_report("large-run")
        value["transcript"] = "x" * (5 * 1024 * 1024)
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            self.write_report(reports, "large", value)
            output = root / "publication.json"
            with mock.patch(
                "publish_swe_campaign.publish",
                return_value={"pull_request": "https://github.test/pull/1"},
            ) as publisher:
                code = self.invoke(reports, output)

            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(code, 0)
            self.assertEqual(receipt["status"], "completed")
            publisher.assert_called_once()

    def test_identical_duplicate_run_is_published_once_with_one_dedup_receipt(self):
        calls = []

        def fake_publish(**kwargs):
            calls.append(kwargs)
            return {"pull_request": "https://github.test/pull/1"}

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            value = native_report("copied-run")
            self.write_report(reports, "a", value)
            self.write_report(reports, "b", value)
            output = root / "publication.json"
            with mock.patch("publish_swe_campaign.publish", side_effect=fake_publish):
                code = self.invoke(reports, output)

            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(code, 0)
            self.assertEqual(receipt["status"], "completed")
            self.assertEqual(receipt["duplicate_count"], 1)
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0]["execution_id"], "campaign-123")
            self.assertEqual(
                [record["status"] for record in receipt["reports"]],
                ["published", "duplicate"],
            )
            self.assertEqual(
                receipt["reports"][1]["duplicate_of"],
                "a/swe_service_report.json",
            )

    def test_conflicting_duplicate_run_fails_before_any_publisher_access(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            reports = root / "reports"
            first = native_report("conflicting-run")
            second = native_report("conflicting-run")
            first["copy"] = "first"
            second["copy"] = "second"
            self.write_report(reports, "a", first)
            self.write_report(reports, "b", second)
            output = root / "publication.json"
            with mock.patch("publish_swe_campaign.publish") as publisher:
                code = self.invoke(reports, output)

            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(code, 1)
            self.assertEqual(receipt["status"], "failed")
            self.assertEqual(receipt["error_count"], 2)
            self.assertTrue(
                all(record["status"] == "error" for record in receipt["reports"])
            )
            publisher.assert_not_called()


if __name__ == "__main__":
    unittest.main()
