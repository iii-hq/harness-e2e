#!/usr/bin/env python3
"""Check, before an execution spends anything, that what it needs answers.

    provider  the key of the provider the dispatch's model runs on, asked
              with a call that spends no token (PROVIDER_PROBES).
    kanban    the Kanban fixture's catalog at the commit the groups check
              out, when the suite runs a Kanban group.
    registry  the Registry the stack's workers come from.

What does not answer (no answer, 429, 5xx) is asked again, waiting twice as
long each time, until HARNESS_E2E_PREFLIGHT_SECONDS have gone by (five
minutes unless the workflow says otherwise). A provider refusing the key is
an answer and fails at once. A Kanban catalog other than the one the
bootstrap expects is only a warning: the Kanban groups stop in their
bootstrap, the others still run. Why it failed is the last line,
`[FAIL] ...`, where `prepare_execution.py failure` reads it for Release
Control. The workflow runs this; a Console execution does not.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Callable, Mapping

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_kanban_catalog as kanban  # noqa: E402
from exact_stack_campaign import credential_catalog  # noqa: E402
from prepare_execution import CLI_TARGET, FIXTURES, REGISTRY_URL, RUNNER  # noqa: E402

PREFLIGHT_SECONDS = 300
FIRST_WAIT = 10
ANTHROPIC_VERSION = "2023-06-01"
#: Where each provider says whether a key is good without spending a token,
#: and what is POSTed there: its models list, except where that list answers
#: without a key (OpenRouter: its key endpoint) or there is none (Z.AI: the
#: chat endpoint provider-zai calls, asked with no messages, answers 400 to a
#: good key and 401 to a bad one). A provider not here runs on its own host
#: or signs in with a subscription, whose login reaches the groups alone.
PROVIDER_PROBES: dict[str, tuple[str, Any]] = {
    "anthropic": ("https://api.anthropic.com/v1/models", None),
    "deepseek": ("https://api.deepseek.com/models", None),
    "kimi": ("https://api.moonshot.ai/v1/models", None),
    "openai": ("https://api.openai.com/v1/models", None),
    "openrouter": ("https://openrouter.ai/api/v1/key", None),
    "xai": ("https://api.x.ai/v1/models", None),
    "zai": ("https://api.z.ai/api/coding/paas/v4/chat/completions", {"model": "glm-4.6", "messages": []}),
}


class Unavailable(Exception):
    """No answer yet: worth asking again."""


class Refused(Exception):
    """An answer that waiting will not change."""


def answer(url: str, headers: Mapping[str, str] | None = None, body: Any = None) -> tuple[int, bytes]:
    """The status and body `url` answers; Unavailable when it does not
    answer, or answers 429 or 5xx."""
    request = urllib.request.Request(url, headers={"Accept": "application/json", **(headers or {})})
    if body is not None:
        request.data = json.dumps(body).encode()
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        status, payload = error.code, error.read()
    except OSError as error:
        raise Unavailable(f"no answer ({getattr(error, 'reason', error)})") from error
    if status == 429 or status >= 500:
        raise Unavailable(f"HTTP {status}")
    return status, payload


def check_provider(provider: str, environ: Mapping[str, str]) -> None:
    variable = credential_catalog()[0].get(provider)
    if variable is None or provider not in PROVIDER_PROBES:
        print(f"provider {provider}: no key to check before its stack starts", file=sys.stderr)
        return
    key = environ.get(variable)
    if not key:
        raise Refused(f"{variable} is not set; provider-{provider} would start without a key")
    url, body = PROVIDER_PROBES[provider]
    headers = ({"x-api-key": key, "anthropic-version": ANTHROPIC_VERSION} if provider == "anthropic"
               else {"Authorization": f"Bearer {key}"})
    try:
        status, _ = answer(url, headers, body)
    except Unavailable as error:
        raise Unavailable(f"cannot reach {provider} at {url}: {error}") from error
    # xAI says 400 to a key it does not know; the empty chat Z.AI is asked
    # gets 400 once its key passed.
    if status in (401, 402, 403) or (status == 400 and body is None):
        raise Refused(f"{provider} refused {variable}: HTTP {status} from {url}")
    if 200 <= status < 300 or status == 400:
        print(f"provider {provider}: {variable} accepted", file=sys.stderr)
    else:
        print(f"::warning::{url} answered HTTP {status}: whether {variable} is good is not known", file=sys.stderr)


def check_kanban(revision: str) -> None:
    repository = FIXTURES["kanban-e2e-fixture"][0]
    at = f"{repository}@{revision[:12]}"
    try:
        status, catalog = answer(f"https://raw.githubusercontent.com/{repository}/{revision}/{kanban.FIXTURE_CATALOG}")
    except Unavailable as error:
        raise Unavailable(f"cannot read the Kanban fixture's catalog at {at}: {error}") from error
    if status == 200 and catalog == kanban.EMBEDDED.read_bytes():
        print(f"kanban fixture {at}: catalog matches", file=sys.stderr)
        return
    what = (f"carries a {kanban.FIXTURE_CATALOG} other than {kanban.CATALOG}" if status == 200
            else f"has no {kanban.FIXTURE_CATALOG} (HTTP {status})")
    print(f"::warning title=Kanban catalog drift::{at} {what}: every Kanban group stops in its bootstrap; "
          "the other groups run", file=sys.stderr)


def check_registry() -> None:
    try:
        status, _ = answer(f"{REGISTRY_URL}/resolve", body={"worker": RUNNER, "version": "latest", "target": CLI_TARGET})
    except Unavailable as error:
        # Release Control reads "cannot reach the registry at <host>" as test
        # infrastructure, not the product.
        raise Unavailable(f"cannot reach the registry at {REGISTRY_URL}: {error}") from error
    print(f"registry {REGISTRY_URL}: answered HTTP {status}", file=sys.stderr)


def checks_for(contract_dir: Path, environ: Mapping[str, str]) -> dict[str, Callable[[], None]]:
    """What this execution needs, from the dispatch and the materialized suite."""
    execution = json.loads((contract_dir / "execution.json").read_text())
    suite = json.loads((contract_dir / "suite.json").read_text())
    provider = str(execution.get("model") or "").partition("/")[0]
    checks: dict[str, Callable[[], None]] = {"provider": lambda: check_provider(provider, environ)}
    prefix = FIXTURES["kanban-e2e-fixture"][2]
    revision = (execution.get("fixtures") or {}).get("kanban-e2e-fixture")
    if revision and any(str(group.get("id", "")).startswith(prefix)
                        for campaign in suite.get("campaigns") or [] for group in campaign.get("groups") or []):
        checks["kanban"] = lambda: check_kanban(revision)
    checks["registry"] = check_registry
    return checks


def until_ready(checks: Mapping[str, Callable[[], None]], seconds: float,
                clock: Callable[[], float] = time.monotonic, sleep: Callable[[float], None] = time.sleep) -> None:
    """Run every check, then what was unavailable again, FIRST_WAIT seconds
    later and twice as long each time after, until `seconds` have gone by.
    A refusal ends it at once."""
    deadline, wait, pending = clock() + seconds, float(FIRST_WAIT), list(checks)
    while True:
        unavailable = {}
        for name in pending:
            try:
                checks[name]()
            except Unavailable as error:
                unavailable[name] = str(error)
        causes = "; ".join(unavailable.values())
        left = deadline - clock()
        if not unavailable:
            return
        if left <= 0:
            raise Unavailable(f"{causes}, for {seconds:g}s")
        wait = min(wait, left)
        print(f"::warning::not ready, asking again in {wait:g}s: {causes}", file=sys.stderr)
        sleep(wait)
        wait *= 2
        pending = list(unavailable)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--contract-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    seconds = float(os.environ.get("HARNESS_E2E_PREFLIGHT_SECONDS") or PREFLIGHT_SECONDS)
    try:
        until_ready(checks_for(args.contract_dir, os.environ), seconds)
    except (Unavailable, Refused) as error:
        print(f"[FAIL] {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
