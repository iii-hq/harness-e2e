import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ASSETS = Path(__file__).resolve().parents[2] / "repository-tasks/registry-version-comparison"
spec = importlib.util.spec_from_file_location("registry_score", ASSETS / "score.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ScoringTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "evidence.txt").write_text("Independent observations")
        self.catalog = {"schema_version": 1, "tests": [
            {"id": n, "metrics": [{"id": f"test{n}", "question": "Did this check pass?",
             "expected": "The specified response", "measurement": "binary", "weight": 100,
             "evidence": ["Captured response"]}]} for n in range(1, 5)
        ]}
        self.path = self.root / "catalog.json"
        self.write_catalog()

    def write_catalog(self):
        self.path.write_text(json.dumps(self.catalog))
        self.catalog, self.identity = module.load_catalog(self.path)

    def measured(self):
        observations = module.template(self.catalog, self.identity)
        for item in observations["observations"]:
            item.update(status="measured", value=1, evidence=["evidence.txt"])
            del item["reason"]
        return observations

    def test_complete_scores_and_mean(self):
        observations = self.measured()
        observations["observations"][0]["value"] = 0
        result = module.score(self.catalog, self.identity, observations, self.root)
        self.assertEqual([t["score"] for t in result["tests"]], [0, 100, 100, 100])
        self.assertEqual(result["mean_score"], 75)

    def test_missing_is_unavailable_and_never_renormalized(self):
        self.catalog["tests"][0]["metrics"][0]["weight"] = 60
        metric = copy.deepcopy(self.catalog["tests"][0]["metrics"][0])
        metric.update(id="missing", weight=40)
        self.catalog["tests"][0]["metrics"].append(metric)
        self.write_catalog()
        observations = self.measured()
        observations["observations"] = [i for i in observations["observations"] if i["id"] != "missing"]
        result = module.score(self.catalog, self.identity, observations, self.root)
        self.assertIsNone(result["tests"][0]["score"])
        self.assertEqual(result["tests"][0]["earned_points"], 60)
        self.assertEqual(result["tests"][0]["measured_weight"], 60)
        self.assertIsNone(result["mean_score"])

    def test_ratio_and_zero_denominator(self):
        self.catalog["tests"][0]["metrics"][0].update(measurement="ratio", numerator="confirmed findings", denominator="reported findings")
        self.write_catalog()
        observations = self.measured()
        item = observations["observations"][0]
        del item["value"]
        item.update(numerator=1, denominator=4)
        result = module.score(self.catalog, self.identity, observations, self.root)
        self.assertEqual(result["tests"][0]["score"], 25)
        for numerator, denominator in ((2, 1), (-1, 1), (True, 2), (1, float("inf"))):
            invalid = copy.deepcopy(observations)
            invalid["observations"][0].update(numerator=numerator, denominator=denominator)
            with self.assertRaises(ValueError):
                module.score(self.catalog, self.identity, invalid, self.root)
        item.update(numerator=0, denominator=0, status="not_applicable", reason="No reported findings")
        result = module.score(self.catalog, self.identity, observations, self.root)
        self.assertIsNone(result["tests"][0]["score"])
        self.assertEqual(result["tests"][0]["metrics"][0]["status"], "not_applicable")
        item["status"] = "measured"
        with self.assertRaises(ValueError):
            module.score(self.catalog, self.identity, observations, self.root)

    def test_rejects_drift_duplicates_missing_evidence_and_invalid_values(self):
        for mutate in (
            lambda o: o.update(catalog_sha256="changed"),
            lambda o: o["observations"].append(o["observations"][0]),
            lambda o: o["observations"][0].update(value=0.5),
            lambda o: o["observations"][0].update(value=True),
            lambda o: o["observations"][0].update(evidence=["missing.txt"]),
            lambda o: o["observations"][0].update(id="unknown"),
            lambda o: o["observations"][0].update(evidence=[str(self.root / "evidence.txt")]),
        ):
            observations = self.measured()
            mutate(observations)
            with self.assertRaises(ValueError):
                module.score(self.catalog, self.identity, observations, self.root)

    def test_evidence_cannot_escape_the_bundle(self):
        bundle = self.root / "bundle"
        bundle.mkdir()
        (bundle / "link").symlink_to(self.root / "evidence.txt")
        for reference in ("../evidence.txt", "link"):
            observations = self.measured()
            observations["observations"][0]["evidence"] = [reference]
            with self.assertRaises(ValueError):
                module.score(self.catalog, self.identity, observations, bundle)

    def test_invalid_weights_and_shipped_catalog(self):
        self.catalog["tests"][0]["metrics"][0]["weight"] = 99
        with self.assertRaises(ValueError):
            self.write_catalog()
        catalog, identity = module.load_catalog(ASSETS / "metrics.json")
        result = module.score(catalog, identity, module.template(catalog, identity), self.root)
        self.assertTrue(all(t["score"] is None for t in result["tests"]))
        self.assertIsNone(result["mean_score"])


if __name__ == "__main__":
    unittest.main()
