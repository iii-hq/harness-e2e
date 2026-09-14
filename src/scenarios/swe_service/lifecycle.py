"""Company curriculum and objective artifact checks, owned by the evaluator."""
import json
from pathlib import PurePosixPath


THROUGH = (0, 0, 3, 4, 5, 6, 7, 8)
STAGES = (
    ("demand", "Customer demand", 8),
    ("planning", "Technical plan", 8),
    ("implementation", "Build the first release", 20),
    ("review_ci", "Review and continuous integration", 15),
    ("release", "Publish and validate compatibility", 12),
    ("evolution", "Introduce tenant isolation", 10),
    ("operations", "Incident, recovery and rollback", 12),
    ("handoff", "Operational handoff", 5),
)
REQUIREMENTS = {"config", "cache", "replay"}

AUTHORED_PROBE = '''import argparse,contextlib,io,json,sys,unittest
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--workspace');p.add_argument('--through');a=p.parse_args()
sys.dont_write_bytecode=True
sys.path.insert(0,str(Path(a.workspace)/'src'))
stream=io.StringIO()
try:
 with contextlib.redirect_stdout(stream),contextlib.redirect_stderr(stream):
  suite=unittest.defaultTestLoader.discover(str(Path(a.workspace)/'tests/agent'),pattern='test_*.py')
  result=unittest.TextTestRunner(stream=stream).run(suite)
 print(json.dumps({'passed':result.testsRun>0 and result.wasSuccessful(),'checks':[],
  'tests_run':result.testsRun,'failures':len(result.failures),'errors':len(result.errors)}))
except Exception:
 print(json.dumps({'passed':False,'checks':[],'tests_run':0,'failures':0,'errors':1}))
'''


def curriculum(tickets):
    prompts = [
        "You lead the engineering team of a company operating this profile service. "
        "Customer support reports configuration leaking between deployments, stale profiles, "
        "and dropped replay events. Inspect the repository and commit docs/request.json with "
        "goal (nonempty string), stakeholders (nonempty array of nonempty strings), and acceptance "
        "(exactly one {id, behavior} for each requirement id config, cache, replay; behavior is a "
        "nonempty string). State observable "
        "acceptance behavior and business impact. This checkpoint records the demand before implementation. "
        "The company's complete initial technical acceptance follows:\n\n" +
        "\n\n".join(ticket["prompt"] for ticket in tickets[:3]),
        "Plan the work before implementation. Commit docs/plan.json with work_items: a nonempty array "
        "of {id, owner, requirements, source_paths, test_paths, depends_on}. Use unique nonempty ids "
        "and nonempty owner strings. requirements, source_paths and test_paths are nonempty arrays. "
        "requirements refers to config/cache/replay; source_paths names existing repository-relative "
        "files under src/; test_paths names planned "
        "tests/agent/test_*.py files; depends_on refers to other work item ids, without cycles. "
        "Cover all acceptance ids. Include interfaces and risks arrays of nonempty strings. "
        "Choose the architecture, sequence and team organization yourself; delegation is optional. "
        "Only traceability is scored from this document, not similarity to a reference plan.",
        "Implement the customer demand and author executable regression tests under tests/agent. "
        "Deliver one integrated commit retaining the demand and plan.\n\n" +
        "\n\n".join(ticket["prompt"] for ticket in tickets[:3]),
        "Review the accepted implementation and prepare it for CI. Commit docs/review.json with "
        "reviewed_head set to the preceding accepted implementation SHA, decision set to approve "
        "after resolving all findings, and findings (array of {file, line, risk, resolution, test}). "
        "Each finding names an existing source file, a positive integer line within that file, "
        "nonempty risk and resolution strings, and test set to an existing tests/agent/test_*.py "
        "file path (without class or method identifiers). An empty findings "
        "array is allowed. Resolve findings and include meaningful unittest regression tests: "
        "the controller runs them on the submitted code and on the original defective code, "
        "where at least one assertion must fail without test/import errors. CI executes the "
        "public suite and cumulative private checks on an immutable Git export. "
        "Address the reliability review as part of this change:\n\n" + tickets[3]["prompt"],
        "Prepare a versioned release of the service. Commit docs/release.json with version "
        "(nonempty string), entrypoint (an existing source path), and rollback_head equal to the "
        "Preceding accepted HEAD shown below (the accepted stage-4 candidate, not the workflow or "
        "merge SHA). The controller exports the submitted SHA, starts its actual "
        "HTTP service and tests compatibility and restart behavior; successful publication is "
        "recorded with that immutable SHA. Production behavior runs in the isolated environment; "
        "the PR, CI, merge and release are real GitHub resources. Submit the first release checkpoint "
        "before merging to receive the compatibility canary. After satisfying that revision, refresh "
        "GitHub CI and review for the final SHA, merge and publish before resubmitting.\n\n" + tickets[4]["prompt"],
        "Product has approved a new customer isolation requirement after release. Update the "
        "implementation and regression tests while retaining the published API contracts.\n\n" + tickets[5]["prompt"],
        "Operations has reported replay degradation and requires an incident repair and recovery "
        "exercise. Commit docs/incident.json with release_head equal to the Published release HEAD "
        "shown below (the accepted stage-5 candidate, not the GitHub tag or merge SHA), "
        "symptom, cause, mitigation (nonempty strings), and regression_test naming an existing "
        "tests/agent/test_*.py file path without class or method identifiers. "
        "The controller measures the published and repaired versions and exercises upgrade, "
        "SIGKILL, restart, rollback and re-upgrade against the same durable database. "
        "Do not discard data or replay acknowledgements.\n\n" + tickets[6]["prompt"],
        "Complete the operational handoff. Commit docs/handoff.json with release_head equal to "
        "the Published release HEAD shown below (the accepted stage-5 candidate, not the GitHub tag "
        "or merge SHA), owner (nonempty string), regression_tests (nonempty array of existing "
        "tests/agent/test_*.py file paths without class or method identifiers), and runbook naming "
        "docs/delivery.md. The controller rechecks startup, API "
        "compatibility, restart, rollback, persisted data and configuration restoration, in "
        "addition to the accumulated acceptance checks.\n\n" + tickets[7]["prompt"],
    ]
    github = (
        "Create the customer issue with operation=issue (title/body), then push this committed HEAD. "
        "Source code must remain unchanged until demand and planning are accepted.",
        "Push this planning HEAD and open its PR with operation=pr. The same PR will track the first release.",
        "Push the integrated HEAD and refresh the existing PR with operation=pr.",
        "Push this HEAD, refresh the PR, request CI with operation=ci and repeat ci until the run is terminal. "
        "Fix failures and repeat on the new SHA. Submit a technical COMMENT review with operation=review "
        "and a substantive body. This records self-review, not independent approval.",
        "Push this HEAD, refresh the PR, obtain successful CI and review at this exact SHA. "
        "After the compatibility revision is revealed, use operation=merge then operation=release with "
        "a unique version of 1-41 ASCII characters: start with a letter or digit, then use only letters, "
        "digits, dots, underscores or hyphens. The release tag will be scoped to this attempt; "
        "published versions are immutable.",
        "Deliver through a new PR: push, pr, successful ci, review and merge at this exact HEAD.",
        "Deliver the incident repair through a new PR: push, pr, successful ci, review and merge at this exact HEAD.",
        "Deliver the handoff through a new PR: push, pr, successful ci, review, merge and a new versioned release. "
        "Use a unique version of 1-41 ASCII characters: start with a letter or digit, then use only "
        "letters, digits, dots, underscores or hyphens. Then close the customer issue with operation=close_issue.",
    )
    return [{"number": number, "id": stage[0], "title": stage[1],
             "prompt": prompt + "\n\nGitHub evidence required: " + remote +
             " Use the attempt-specific GitHub tool; supply head for push/pr/ci/review/merge/release. "
             "Every operation returns an operation_id and verified remote observations; inspect shows the "
             "current records. Missing or failed operations must be resolved before checkpoint acceptance.",
             **({"canary_prompt": tickets[4]["canary_prompt"]} if number == 5 else {})}
            for number, (stage, prompt, remote) in enumerate(zip(STAGES, prompts, github), 1)]


def document_checks(number, files, state):
    """Check provenance and traceability; do not grade the prose's semantic quality."""
    def text(value):
        return isinstance(value, str) and bool(value.strip())

    def strings(value):
        return isinstance(value, list) and bool(value) and all(text(item) for item in value)

    def authored(path):
        return (isinstance(path, str) and path.startswith("tests/agent/")
                and PurePosixPath(path).name.startswith("test_") and path.endswith(".py"))

    def exists(path):
        return isinstance(path, str) and path in files

    filename = {1: "request", 2: "plan", 4: "review", 5: "release", 7: "incident", 8: "handoff"}.get(number)
    if filename is None:
        return []
    try:
        value = json.loads(files.get(f"docs/{filename}.json", b"null"))
        if not isinstance(value, dict):
            raise ValueError("expected a JSON object")
        if number == 1:
            acceptance = value.get("acceptance", [])
            passed = (text(value.get("goal")) and strings(value.get("stakeholders"))
                      and isinstance(acceptance, list) and len(acceptance) == len(REQUIREMENTS)
                      and all(isinstance(item, dict) and text(item.get("behavior")) for item in acceptance)
                      and {item.get("id") for item in acceptance} == REQUIREMENTS)
        elif number == 2:
            items = value.get("work_items", [])
            passed = isinstance(items, list) and bool(items) and all(isinstance(item, dict) for item in items)
            if passed:
                ids = [item.get("id") for item in items]
                passed = (all(text(item) for item in ids) and len(set(ids)) == len(ids)
                          and strings(value.get("interfaces")) and strings(value.get("risks")))
                covered = set()
                graph = {}
                for item in items:
                    requirements = item.get("requirements", [])
                    sources, tests = item.get("source_paths", []), item.get("test_paths", [])
                    dependencies = item.get("depends_on", [])
                    passed = passed and (text(item.get("owner")) and strings(requirements)
                        and set(requirements) <= REQUIREMENTS and strings(sources)
                        and all(exists(path) and path.startswith("src/") for path in sources)
                        and strings(tests) and all(authored(path) for path in tests)
                        and isinstance(dependencies, list) and all(dep in ids for dep in dependencies))
                    if not passed:
                        break
                    covered.update(requirements)
                    graph[item["id"]] = set(dependencies)
                ready = set()
                while graph:
                    newly_ready = {key for key, deps in graph.items() if deps <= ready}
                    if not newly_ready:
                        passed = False
                        break
                    ready.update(newly_ready)
                    graph = {key: deps for key, deps in graph.items() if key not in newly_ready}
                passed = passed and covered == REQUIREMENTS
        elif number == 4:
            findings = value.get("findings")
            passed = (value.get("reviewed_head") == state["accepted_head"]
                      and value.get("decision") == "approve" and isinstance(findings, list))
            if passed:
                for item in findings:
                    passed = passed and (isinstance(item, dict) and exists(item.get("file"))
                        and type(item.get("line")) is int and 0 < item["line"] <= len(files[item["file"]].splitlines())
                        and text(item.get("risk")) and text(item.get("resolution"))
                        and authored(item.get("test")) and exists(item.get("test")))
        elif number == 5:
            passed = (text(value.get("version")) and exists(value.get("entrypoint"))
                      and value.get("rollback_head") == state["accepted_head"])
        elif number == 7:
            passed = (value.get("release_head") == state.get("release_head")
                      and all(text(value.get(key)) for key in ("symptom", "cause", "mitigation"))
                      and authored(value.get("regression_test")) and exists(value.get("regression_test")))
        else:
            tests = value.get("regression_tests")
            passed = (value.get("release_head") == state.get("release_head")
                      and text(value.get("owner")) and strings(tests)
                      and all(authored(path) and exists(path) for path in tests)
                      and value.get("runbook") == "docs/delivery.md"
                      and bool(files.get("docs/delivery.md", b"").strip()))
        return [{"id": f"{filename}_traceability", "passed": bool(passed),
                 "reason": "Artifact references and structure verified" if passed else
                 f"docs/{filename}.json does not satisfy the published traceability contract"}]
    except (ValueError, TypeError, KeyError, UnicodeError) as error:
        return [{"id": f"{filename}_traceability", "passed": False,
                 "reason": f"Invalid docs/{filename}.json: {error}"}]


def assessments(checkpoints):
    results = []
    for number, (name, _, weight) in enumerate(STAGES, 1):
        accepted = next((item for item in reversed(checkpoints)
                         if item["ticket"] == number and item["accepted"]), None)
        checks = accepted.get("lifecycle_checks", []) if accepted else []
        fraction = sum(check.get("passed") is True for check in checks) / len(checks) if checks else 0
        results.append({"id": name, "weight": weight, "score": fraction,
                        "checks": checks, "head_sha": accepted["head_sha"] if accepted else None})
    return results
