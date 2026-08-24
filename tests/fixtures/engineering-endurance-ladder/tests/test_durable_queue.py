import tempfile
import unittest
from pathlib import Path

from src.durable_queue import DurableQueue, JobNotFound


class DurableQueueBaselineTests(unittest.TestCase):
    def test_submit_and_reload(self):
        with tempfile.TemporaryDirectory() as temporary:
            journal = Path(temporary) / "queue.jsonl"
            queue = DurableQueue(journal)
            job_id = queue.submit({"release": "2026.8"})
            self.assertEqual(queue.get(job_id)["status"], "pending")
            self.assertEqual(DurableQueue(journal).get(job_id)["payload"], {"release": "2026.8"})

    def test_complete_survives_reload(self):
        with tempfile.TemporaryDirectory() as temporary:
            journal = Path(temporary) / "queue.jsonl"
            queue = DurableQueue(journal)
            job_id = queue.submit({"task": "publish"})
            queue.complete(job_id)
            self.assertEqual(DurableQueue(journal).get(job_id)["status"], "completed")

    def test_unknown_job_raises(self):
        with tempfile.TemporaryDirectory() as temporary:
            queue = DurableQueue(Path(temporary) / "queue.jsonl")
            with self.assertRaises(JobNotFound):
                queue.get("missing")


if __name__ == "__main__":
    unittest.main()
