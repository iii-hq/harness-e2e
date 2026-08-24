"""Small append-only durable queue used by the engineering endurance benchmark.

The implementation intentionally contains only the behavior needed by the public
baseline. Endurance tickets add production behavior cumulatively.
"""

from __future__ import annotations

import json
import uuid
from pathlib import Path
from typing import Any


class JobNotFound(KeyError):
    pass


class DurableQueue:
    def __init__(self, journal_path: str | Path):
        self.journal_path = Path(journal_path)
        self.journal_path.parent.mkdir(parents=True, exist_ok=True)
        self.journal_path.touch(exist_ok=True)
        self._jobs: dict[str, dict[str, Any]] = {}
        self._load()

    def _load(self) -> None:
        for line in self.journal_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            event = json.loads(line)
            if event["type"] == "submitted":
                self._jobs[event["job_id"]] = {
                    "id": event["job_id"],
                    "payload": event["payload"],
                    "status": "pending",
                }
            elif event["type"] == "completed":
                self._jobs[event["job_id"]]["status"] = "completed"

    def _append(self, event: dict[str, Any]) -> None:
        with self.journal_path.open("a", encoding="utf-8") as journal:
            journal.write(json.dumps(event, sort_keys=True, separators=(",", ":")) + "\n")
            journal.flush()

    def submit(self, payload: dict[str, Any], idempotency_key: str | None = None) -> str:
        job_id = uuid.uuid4().hex
        self._append({"type": "submitted", "job_id": job_id, "payload": payload})
        self._jobs[job_id] = {"id": job_id, "payload": payload, "status": "pending"}
        return job_id

    def get(self, job_id: str) -> dict[str, Any]:
        try:
            return dict(self._jobs[job_id])
        except KeyError as error:
            raise JobNotFound(job_id) from error

    def list_jobs(self) -> list[dict[str, Any]]:
        return [dict(job) for job in self._jobs.values()]

    def complete(self, job_id: str) -> None:
        if job_id not in self._jobs:
            raise JobNotFound(job_id)
        self._append({"type": "completed", "job_id": job_id})
        self._jobs[job_id]["status"] = "completed"
