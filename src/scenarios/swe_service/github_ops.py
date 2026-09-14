"""Trusted, run-scoped GitHub operations for the company lifecycle fixture."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from urllib.parse import quote


REPO = "iii-hq/software-company-lifecycle-e2e"
WORKFLOW = "github-ci.yml"
REMOTE = f"https://github.com/{REPO}.git"
SHA = re.compile(r"[0-9a-f]{40}\Z")
TOKEN = re.compile(r"[0-9a-f]{32}\Z")
VERSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,40}\Z")
OPERATIONS = {"issue", "push", "pr", "ci", "review", "merge", "release", "inspect", "close_issue"}
SOURCE_OPERATIONS = {"push", "pr", "ci", "review", "merge", "release"}


class BridgeError(RuntimeError):
    pass


def _call(args, *, payload=None, timeout=60, allowed=(0,)):
    result = subprocess.run(args, input=payload, text=True, capture_output=True,
                            timeout=timeout, env=os.environ.copy(), check=False)
    if result.returncode not in allowed:
        raise BridgeError(f"Trusted GitHub command failed ({result.returncode}): {result.stderr[:600]}")
    if len(result.stdout) > 4_000_000:
        raise BridgeError("Trusted GitHub response exceeded its size limit")
    return result.stdout


def _api(path, method="GET", body=None):
    endpoint = f"repos/{REPO}" + (f"/{path}" if path else "")
    args = ["gh", "api", endpoint, "-X", method]
    if body is not None:
        args += ["--input", "-"]
    raw = _call(args, payload=json.dumps(body) if body is not None else None)
    return json.loads(raw) if raw.strip() else {}


def _optional_api(path):
    try:
        return _api(path)
    except BridgeError as error:
        if "HTTP 404" in str(error):
            return None
        raise


def _pages(path, key=None):
    for page in range(1, 21):
        separator = "&" if "?" in path else "?"
        data = _api(f"{path}{separator}per_page=100&page={page}")
        rows = data[key] if key else data
        if not isinstance(rows, list):
            raise BridgeError("GitHub list response has an unexpected shape")
        yield from rows
        if len(rows) < 100:
            return
    raise BridgeError("GitHub list exceeded the bounded search window")


def _git(objects, *args):
    return _call(["git", "-c", "core.hooksPath=/dev/null", "-c",
                  "credential.helper=!gh auth git-credential", "-c",
                  "credential.useHttpPath=true", "-C", str(objects), *args], timeout=120)


def _remote_head(ref):
    lines = _git(".", "ls-remote", "--refs", REMOTE, ref).splitlines()
    if len(lines) > 1:
        raise BridgeError("Remote ref lookup is ambiguous")
    if not lines:
        return None
    head, found = lines[0].split("\t", 1)
    if found != ref or not SHA.fullmatch(head):
        raise BridgeError("Remote ref lookup returned an invalid identity")
    return head


def _scope(state):
    namespace = state["ownership_token"]
    if not TOKEN.fullmatch(namespace):
        raise BridgeError("Run namespace is not a trusted ownership token")
    if state.get("mode") != "lifecycle":
        raise BridgeError("GitHub bridge requires the company lifecycle mode")
    return namespace, f"run/{namespace}/base"


def _local(state, save_callback):
    namespace, base = _scope(state)
    if "github" not in state:
        state["github"] = {"repo": REPO, "namespace": namespace, "base_ref": base,
                           "actor": None, "journal": [], "heads": {}, "prs": {},
                           "ci": {}, "reviews": {}, "merges": {}, "releases": {}}
        save_callback(state)
    github = state["github"]
    if github["repo"] != REPO or github["namespace"] != namespace or github["base_ref"] != base:
        raise BridgeError("GitHub state does not match this run")
    return github


def initialize(state, save_callback):
    github = _local(state, save_callback)
    repo = _api("")
    actor = json.loads(_call(["gh", "api", "user"]))
    if (repo.get("full_name") != REPO or repo.get("private") is not True or
            not isinstance(repo.get("id"), int) or not actor.get("login")):
        raise BridgeError("Expected authenticated access to the fixed private lifecycle repository")
    if github.get("actor") and github["actor"] != actor["login"]:
        raise BridgeError("Authenticated GitHub actor changed during this run")
    if github.get("repo_id") and github["repo_id"] != repo["id"]:
        raise BridgeError("Fixed lifecycle repository identity changed during this run")
    github["actor"] = actor["login"]
    github["repo_id"] = repo["id"]
    github["actor_verified_at"] = time.time()
    save_callback(state)
    return github


def _cycle(ticket):
    return 1 if ticket <= 5 else ticket - 4


def _work_ref(github, cycle):
    return f"run/{github['namespace']}/stage-{cycle}"


def _branch(github, cycle):
    return f"refs/heads/{_work_ref(github, cycle)}"


def _marker(github):
    return f"<!-- company-lifecycle:{github['namespace']} -->"


def _issue(github, request):
    existing = [item for item in _pages("issues?state=all")
                if _marker(github) in (item.get("body") or "") and "pull_request" not in item]
    if len(existing) > 1:
        raise BridgeError("Run has multiple matching issues")
    if existing:
        issue = existing[0]
    else:
        title = request.get("title") or "Profile service lifecycle"
        body = request.get("body") or "Track the isolated company lifecycle delivery."
        if not isinstance(title, str) or not title.strip() or not isinstance(body, str) or len(body) > 20_000:
            raise BridgeError("Issue title or body is invalid")
        issue = _api("issues", "POST", {"title": title[:200], "body": f"{body}\n\n{_marker(github)}"})
    readback = _api(f"issues/{issue['number']}")
    if _marker(github) not in (readback.get("body") or ""):
        raise BridgeError("Issue readback lost run ownership")
    result = {"number": readback["number"], "state": readback["state"],
              "url": readback["html_url"], "actor": readback["user"]["login"]}
    github["issue"] = result
    return result


def _push(state, github, head, cycle):
    objects = Path(state["objects"])
    if _git(objects, "rev-parse", f"{head}^{{commit}}").strip() != head:
        raise BridgeError("Candidate SHA is absent from the trusted object store")
    initial = state["initial_head"]
    if _git(objects, "rev-parse", f"{initial}^{{commit}}").strip() != initial:
        raise BridgeError("Initial SHA is absent from the trusted object store")
    _git(objects, "merge-base", "--is-ancestor", initial, head)
    base_ref = f"refs/heads/{github['base_ref']}"
    base = _remote_head(base_ref)
    expected_base = github.get("base_head", initial)
    if base is None:
        _git(objects, "push", REMOTE, f"{initial}:{base_ref}")
        base = _remote_head(base_ref)
    if base != expected_base:
        raise BridgeError("Run base ref moved outside the trusted journal")
    branch = _branch(github, cycle)
    old = _remote_head(branch)
    if old and old != head:
        _git(objects, "merge-base", "--is-ancestor", old, head)
    if old != head:
        _git(objects, "push", REMOTE, f"{head}:{branch}")
    if _remote_head(branch) != head:
        raise BridgeError("Pushed branch readback differs from candidate SHA")
    github["base_head"] = base
    github["heads"][str(cycle)] = {"head": head, "ref": branch, "base": base}
    return github["heads"][str(cycle)]


def _pr(github, request, head, cycle):
    if not github.get("issue", {}).get("number"):
        raise BridgeError("Pull request requires the run issue")
    pushed = github["heads"].get(str(cycle), {})
    if pushed.get("head") != head or _remote_head(pushed["ref"]) != head:
        raise BridgeError("PR requires the exact candidate SHA on its run branch")
    matches = [item for item in _pages("pulls?state=all")
               if item["head"]["ref"] == _work_ref(github, cycle)
               and item["base"]["ref"] == github["base_ref"]
               and item["head"]["repo"]["full_name"] == REPO
               and item["base"]["repo"]["full_name"] == REPO]
    if len(matches) > 1:
        raise BridgeError("Run branch has multiple matching pull requests")
    if matches:
        pr = matches[0]
    else:
        title = request.get("title") or f"Lifecycle delivery stage {cycle}"
        body = request.get("body") or "Review the candidate against its lifecycle contract."
        if not isinstance(title, str) or not title.strip() or not isinstance(body, str) or len(body) > 20_000:
            raise BridgeError("Pull request title or body is invalid")
        pr = _api("pulls", "POST", {"title": title[:200], "body": f"{body}\n\n{_marker(github)}",
                                    "head": _work_ref(github, cycle), "base": github["base_ref"]})
    readback = _api(f"pulls/{pr['number']}")
    if (readback["head"]["sha"] != head or readback["base"]["ref"] != github["base_ref"] or
            readback["head"]["repo"]["full_name"] != REPO):
        raise BridgeError("Pull request readback does not match the candidate")
    result = {"number": readback["number"], "head": head, "base_ref": github["base_ref"],
              "head_repo": REPO,
              "state": readback["state"], "merged": readback.get("merged", False),
              "url": readback["html_url"], "actor": readback["user"]["login"]}
    github["prs"][str(cycle)] = result
    return result


def _run_by_name(name):
    found = [item for item in _pages(f"actions/workflows/{WORKFLOW}/runs?event=workflow_dispatch", "workflow_runs")
             if item.get("display_title") == name]
    if len(found) > 1:
        raise BridgeError("CI dispatch has multiple matching runs")
    return found[0] if found else None


def _workflow_digest(head):
    if not SHA.fullmatch(head or ""):
        raise BridgeError("CI workflow source is not an immutable commit SHA")
    source = _optional_api(f"contents/.github/workflows/{WORKFLOW}?ref={head}")
    if not source:
        return None
    if source.get("encoding") != "base64" or not isinstance(source.get("content"), str):
        raise BridgeError("CI workflow source has an invalid GitHub content response")
    remote = base64.b64decode(source["content"].replace("\n", ""), validate=True)
    return hashlib.sha256(remote).hexdigest()


def _ci(state, github, head, cycle, operation_id, prior, entry, save_callback):
    pr = github["prs"].get(str(cycle), {})
    if pr.get("head") != head or pr.get("state") != "open":
        raise BridgeError("CI requires an open pull request at this SHA")
    name = f"company-ci-{github['namespace']}-{operation_id}"
    run = _run_by_name(name)
    if not run:
        attempts = [item for item in prior if item.get("dispatch_attempted")]
        if not attempts:
            entry["dispatch_attempted"] = True
            save_callback(state)
            _api(f"actions/workflows/{WORKFLOW}/dispatches", "POST", {
                "ref": "main", "inputs": {"candidate_sha": head, "run_namespace": github["namespace"],
                                          "operation_id": operation_id}})
            entry["dispatch_confirmed_at"] = time.time()
            save_callback(state)
        elif not attempts[-1].get("dispatch_confirmed_at"):
            raise BridgeError("CI dispatch outcome is unknown; inspect the remote run before retrying")
        elif time.time() - attempts[-1]["dispatch_confirmed_at"] > 300:
            raise BridgeError("Confirmed CI dispatch has no matching run after five minutes")
    if not run or run.get("status") != "completed":
        result = {"status": "pending", "run_id": run.get("id") if run else None,
                  "candidate_head": head, "passed": None,
                  "reason": "Dispatch is awaiting a matching completed workflow run"}
        github["ci"][str(cycle)] = result
        return result
    run = _api(f"actions/runs/{run['id']}")
    if run.get("head_branch") != "main" or run.get("event") != "workflow_dispatch":
        raise BridgeError("CI run is not the trusted main workflow dispatch")
    trusted_workflow_sha256 = hashlib.sha256(Path(__file__).with_name(WORKFLOW).read_bytes()).hexdigest()
    if _workflow_digest(run.get("head_sha")) != trusted_workflow_sha256:
        raise BridgeError("CI run did not execute the pinned trusted workflow contents")
    artifacts = [item for item in _api(f"actions/runs/{run['id']}/artifacts")["artifacts"]
                 if item["name"] == name and not item.get("expired")]
    if len(artifacts) != 1:
        raise BridgeError("Exact CI run has no unique evidence artifact")
    with tempfile.TemporaryDirectory(prefix="company-ci-", dir=state["assets"]) as directory:
        _call(["gh", "run", "download", str(run["id"]), "-R", REPO, "-n", name, "-D", directory],
              timeout=120)
        raw_evidence = (Path(directory) / "result.json").read_bytes()
        evidence = json.loads(raw_evidence)
    if (evidence.get("schema") != "company-ci-evidence/v1" or
            evidence.get("candidate_sha") != head or evidence.get("checked_out_sha") != head or
            evidence.get("run_namespace") != github["namespace"] or
            evidence.get("operation_id") != operation_id or
            type(evidence.get("public_exit")) is not int or
            type(evidence.get("authored_exit")) is not int or
            evidence.get("passed") is not (evidence["public_exit"] == evidence["authored_exit"] == 0)):
        raise BridgeError("CI artifact identity does not match the dispatched candidate")
    result = {"run_id": run["id"], "run_attempt": run["run_attempt"],
              "workflow_head": run["head_sha"], "trusted_workflow_sha256": trusted_workflow_sha256,
              "candidate_head": head,
              "status": run["status"], "conclusion": run["conclusion"],
              "artifact_id": artifacts[0]["id"], "artifact_digest": artifacts[0].get("digest"),
              "evidence_sha256": hashlib.sha256(raw_evidence).hexdigest(), "evidence": evidence,
              "passed": run["conclusion"] == "success" and evidence.get("passed") is True
              and evidence.get("infrastructure_error") is None,
              "actor": run["actor"]["login"], "url": run["html_url"]}
    github["ci"][str(cycle)] = result
    return result


def _review(github, request, head, cycle, operation_id):
    pr = github["prs"].get(str(cycle), {})
    if pr.get("head") != head or pr.get("state") != "open":
        raise BridgeError("Review requires an open pull request at this SHA")
    marker = f"<!-- lifecycle-review:{operation_id} -->"
    reviews = [item for item in _pages(f"pulls/{pr['number']}/reviews")
               if marker in (item.get("body") or "")]
    if len(reviews) > 1:
        raise BridgeError("Review marker is ambiguous")
    if reviews:
        review = reviews[0]
    else:
        body = request.get("body") or "Reviewed the exact candidate SHA and its CI evidence."
        if not isinstance(body, str) or not body.strip() or len(body) > 20_000:
            raise BridgeError("Review body is invalid")
        review = _api(f"pulls/{pr['number']}/reviews", "POST", {
            "event": "COMMENT", "commit_id": head, "body": f"{body}\n\n{marker}"})
    review = _api(f"pulls/{pr['number']}/reviews/{review['id']}")
    if marker not in (review.get("body") or ""):
        raise BridgeError("Review readback lost its operation marker")
    if review.get("commit_id") != head or review.get("state") != "COMMENTED":
        raise BridgeError("Review readback is not a COMMENT on the exact SHA")
    result = {"id": review["id"], "head": head, "event": "COMMENT",
              "actor": review["user"]["login"], "pr_number": pr["number"],
              "url": review["html_url"]}
    github["reviews"][str(cycle)] = result
    return result


def _merge(github, head, cycle):
    pr = github["prs"].get(str(cycle), {})
    ci = github["ci"].get(str(cycle), {})
    review = github["reviews"].get(str(cycle), {})
    if (pr.get("head") != head or ci.get("candidate_head") != head or not ci.get("passed") or
            review.get("head") != head or review.get("event") != "COMMENT"):
        raise BridgeError("Merge requires exact-head PR, successful CI and COMMENT review")
    if _remote_head(f"refs/heads/{github['base_ref']}") != github.get("base_head"):
        raise BridgeError("Run base moved before merge")
    before = _api(f"pulls/{pr['number']}")
    if before["head"]["sha"] != head or before["base"]["ref"] != github["base_ref"]:
        raise BridgeError("Pull request moved before merge")
    if not before.get("merged"):
        _api(f"pulls/{pr['number']}/merge", "PUT", {"sha": head, "merge_method": "merge"})
    after = _api(f"pulls/{pr['number']}")
    merge_head = after.get("merge_commit_sha")
    if not after.get("merged") or not SHA.fullmatch(merge_head or ""):
        raise BridgeError("Merge readback lacks an immutable merge SHA")
    if _remote_head(f"refs/heads/{github['base_ref']}") != merge_head:
        raise BridgeError("Run base does not point at the verified merge SHA")
    result = {"pr_number": pr["number"], "candidate_head": head,
              "merge_head": merge_head, "merged": True,
              "actor": (after.get("merged_by") or {}).get("login")}
    github["merges"][str(cycle)] = result
    github["base_head"] = merge_head
    github["prs"][str(cycle)]["merged"] = True
    github["prs"][str(cycle)]["state"] = "closed"
    return result


def _release(github, request, head, cycle):
    merged = github["merges"].get(str(cycle), {})
    if merged.get("candidate_head") != head or not merged.get("merged"):
        raise BridgeError("Release requires the current candidate to be merged")
    version = request.get("version")
    if not isinstance(version, str) or not VERSION.fullmatch(version):
        raise BridgeError("Release version is invalid")
    tag = f"run/{github['namespace']}/v{version}"
    matches = [item for item in _pages("releases") if item["tag_name"] == tag]
    if len(matches) > 1:
        raise BridgeError("Release tag is ambiguous")
    if matches:
        release = matches[0]
    else:
        release = _api("releases", "POST", {
            "tag_name": tag, "target_commitish": merged["merge_head"],
            "name": f"Lifecycle {version} ({github['namespace']})",
            "body": f"Candidate SHA: {head}\nMerged SHA: {merged['merge_head']}",
            "prerelease": True, "draft": False, "generate_release_notes": False})
    release = _api(f"releases/{release['id']}")
    if release.get("tag_name") != tag:
        raise BridgeError("Release readback lost its run-scoped tag")
    ref = _api(f"git/ref/tags/{quote(tag, safe='/')}")
    if ref.get("object", {}).get("sha") != merged["merge_head"]:
        raise BridgeError("Release tag does not point to the verified merge SHA")
    result = {"id": release["id"], "tag": tag, "candidate_head": head,
              "merge_head": merged["merge_head"], "tag_head": ref["object"]["sha"],
              "exists": True, "actor": release["author"]["login"], "url": release["html_url"]}
    github["releases"][str(cycle)] = result
    return result


def _close_issue(github):
    issue = github.get("issue", {})
    if not issue.get("number") or not github["releases"].get("4"):
        raise BridgeError("Close requires the run issue and final release")
    current = _api(f"issues/{issue['number']}")
    if current["state"] != "closed":
        _api(f"issues/{issue['number']}", "PATCH", {"state": "closed"})
    readback = _api(f"issues/{issue['number']}")
    if readback["state"] != "closed" or _marker(github) not in (readback.get("body") or ""):
        raise BridgeError("Issue closure readback does not match this run")
    github["issue"]["state"] = "closed"
    return github["issue"]


def _inspect(github):
    result = {"issue": None, "prs": {}, "base_head": _remote_head(f"refs/heads/{github['base_ref']}")}
    github["observed_base_head"] = result["base_head"]
    if github.get("issue"):
        issue = _optional_api(f"issues/{github['issue']['number']}")
        result["issue"] = {"number": github["issue"]["number"], "state": issue["state"] if issue else "missing"}
        github["issue"]["state"] = (issue["state"] if _marker(github) in (issue.get("body") or "")
                                     else "ownership_lost") if issue else "missing"
    for cycle, receipt in github["heads"].items():
        receipt["head"] = _remote_head(receipt["ref"])
        result.setdefault("heads", {})[cycle] = receipt["head"]
    for cycle, recorded in github["prs"].items():
        pr = _optional_api(f"pulls/{recorded['number']}")
        result["prs"][cycle] = {"number": recorded["number"], "head": pr["head"]["sha"] if pr else None,
                                "merged": pr.get("merged", False) if pr else False,
                                "merge_head": pr.get("merge_commit_sha") if pr else None}
        recorded.update(head=pr["head"]["sha"] if pr else None,
                        head_repo=pr["head"]["repo"]["full_name"] if pr else None,
                        base_ref=pr["base"]["ref"] if pr else None,
                        state=pr["state"] if pr else "missing", merged=pr.get("merged", False) if pr else False)
        if cycle in github["merges"]:
            github["merges"][cycle]["merged"] = pr.get("merged", False) if pr else False
            github["merges"][cycle]["merge_head"] = pr.get("merge_commit_sha") if pr else None
    for cycle, receipt in github["ci"].items():
        if receipt.get("run_id"):
            run = _optional_api(f"actions/runs/{receipt['run_id']}")
            artifacts = _optional_api(f"actions/runs/{receipt['run_id']}/artifacts") if run else None
            artifact_present = any(item.get("id") == receipt.get("artifact_id") and not item.get("expired")
                                   for item in (artifacts or {}).get("artifacts", []))
            workflow_matches = (bool(run) and run.get("head_sha") == receipt.get("workflow_head")
                                and _workflow_digest(run.get("head_sha")) == receipt.get("trusted_workflow_sha256")
                                == hashlib.sha256(Path(__file__).with_name(WORKFLOW).read_bytes()).hexdigest())
            if (not run or run.get("run_attempt") != receipt.get("run_attempt") or
                    run.get("conclusion") != "success" or run.get("head_branch") != "main" or
                    run.get("event") != "workflow_dispatch" or not artifact_present or not workflow_matches):
                receipt["passed"] = False
            result.setdefault("ci", {})[cycle] = {"run_id": receipt["run_id"],
                                                   "run_attempt": run.get("run_attempt") if run else None,
                                                   "conclusion": run.get("conclusion") if run else None,
                                                   "artifact_present": artifact_present,
                                                   "workflow_matches": workflow_matches}
    for cycle, receipt in github["reviews"].items():
        review = _optional_api(f"pulls/{receipt['pr_number']}/reviews/{receipt['id']}")
        receipt.update(head=review.get("commit_id") if review else None,
                       event=("COMMENT" if review.get("state") == "COMMENTED" else review.get("state"))
                       if review else "missing")
    releases = {item["tag_name"]: item for item in _pages("releases")}
    for cycle, receipt in github["releases"].items():
        release = releases.get(receipt["tag"])
        receipt["tag_head"] = _remote_head(f"refs/tags/{receipt['tag']}") if release else None
        receipt["exists"] = bool(release and release.get("id") == receipt["id"])
        result.setdefault("releases", {})[cycle] = {"tag": receipt["tag"], "head": receipt["tag_head"],
                                                     "exists": receipt["exists"]}
    github["last_inspection"] = result
    return result


def refresh(state, save_callback):
    """Recheck remote receipts at an evaluator checkpoint without mutating GitHub."""
    github = _local(state, save_callback)
    operation_id = hashlib.sha256(
        f"{github['namespace']}:verify:{state['current_ticket']}:{len(github['journal'])}".encode()
    ).hexdigest()[:32]
    entry = {"operation_id": operation_id, "operation": "verify", "origin": "evaluator",
             "ticket": state["current_ticket"], "intent_at": time.time(), "status": "intent",
             "actor": github.get("actor")}
    github["journal"].append(entry)
    save_callback(state)
    try:
        initialize(state, save_callback)
        entry["actor"] = github["actor"]
        result = _inspect(github)
        entry.update(status="completed", completed_at=time.time(), result=result)
        save_callback(state)
        return result
    except Exception as error:
        entry.update(status="failed", completed_at=time.time(), error=str(error)[:1000])
        save_callback(state)
        raise


def operate(state, request, save_callback, validate=None):
    if not isinstance(request, dict):
        raise BridgeError("GitHub request must be an object")
    github = _local(state, save_callback)
    operation = request["operation"]
    ticket = state["current_ticket"]
    cycle = _cycle(ticket)
    head = request.get("head")
    version = request.get("version") or ""
    key = f"{github['namespace']}:{operation}:{cycle}:{head or ''}:{version}"
    operation_id = hashlib.sha256(key.encode()).hexdigest()[:32]
    prior = [item for item in github["journal"] if item["operation_id"] == operation_id]
    entry = {"operation_id": operation_id, "operation": operation, "ticket": ticket,
             "origin": "subject",
             "cycle": cycle, "head": head, "version": version or None,
             "request": {key: str(request[key])[:20_000] for key in ("title", "body") if key in request},
             "intent_at": time.time(), "status": "intent", "actor": github.get("actor")}
    github["journal"].append(entry)
    save_callback(state)
    try:
        if operation not in OPERATIONS:
            raise BridgeError("Unsupported GitHub operation")
        if set(request) - {"operation", "head", "title", "body", "version"}:
            raise BridgeError("GitHub request has unsupported fields")
        if ((operation == "issue" and ticket != 1) or
                (operation == "pr" and ticket < 2) or
                (operation in ("ci", "review") and ticket < 4) or
                (operation == "merge" and (ticket < 5 or ticket == 5 and not state.get("canary_revealed"))) or
                (operation == "release" and ticket not in (5, 8)) or
                (operation == "release" and ticket == 5 and not state.get("canary_revealed")) or
                (operation == "close_issue" and ticket != 8)):
            raise BridgeError("Operation is unavailable at the current lifecycle stage")
        if operation in SOURCE_OPERATIONS and (not isinstance(head, str) or not SHA.fullmatch(head)):
            raise BridgeError("Source operation requires a full candidate SHA")
        if validate is not None:
            validate()
        initialize(state, save_callback)
        entry["actor"] = github["actor"]
        save_callback(state)
        if operation == "issue":
            result = _issue(github, request)
        elif operation == "push":
            result = _push(state, github, head, cycle)
        elif operation == "pr":
            result = _pr(github, request, head, cycle)
        elif operation == "ci":
            result = _ci(state, github, head, cycle, operation_id, prior, entry, save_callback)
        elif operation == "review":
            result = _review(github, request, head, cycle, operation_id)
        elif operation == "merge":
            result = _merge(github, head, cycle)
        elif operation == "release":
            result = _release(github, request, head, cycle)
        elif operation == "close_issue":
            result = _close_issue(github)
        else:
            result = _inspect(github)
        status = "pending" if result.get("status") == "pending" else "completed"
        entry.update(status=status, completed_at=time.time(), result=result)
        save_callback(state)
        return {"operation_id": operation_id, "operation": operation, "status": status, "result": result}
    except Exception as error:
        entry.update(status="failed", completed_at=time.time(), error=str(error)[:1000])
        save_callback(state)
        return {"operation_id": operation_id, "operation": operation,
                "status": "failed", "error": str(error)[:1000]}


def checks(state, ticket, head):
    github = state.get("github", {})
    cycle = _cycle(ticket)
    key = str(cycle)
    issue = github.get("issue", {})
    pushed = github.get("heads", {}).get(key, {})
    pr = github.get("prs", {}).get(key, {})
    ci = github.get("ci", {}).get(key, {})
    review = github.get("reviews", {}).get(key, {})
    merged = github.get("merges", {}).get(key, {})
    released = github.get("releases", {}).get(key, {})
    required = [("issue", issue.get("state") in ("open", "closed"), issue),
                ("push", pushed.get("head") == head, pushed)]
    if "observed_base_head" in github:
        required.append(("base_integrity", github["observed_base_head"] == github.get("base_head"),
                         {"expected": github.get("base_head"), "observed": github["observed_base_head"]}))
    if ticket >= 2:
        required.append(("pr", pr.get("head") == head and pr.get("base_ref") == github.get("base_ref")
                         and pr.get("head_repo") == REPO, pr))
    if ticket >= 4:
        required.extend([("ci", ci.get("candidate_head") == head and ci.get("passed") is True, ci),
                         ("review", review.get("head") == head and review.get("event") == "COMMENT", review)])
    if ticket in (5, 6, 7, 8) and (ticket != 5 or state.get("canary_revealed")):
        required.append(("merge", merged.get("candidate_head") == head and merged.get("merged") is True
                         and bool(SHA.fullmatch(merged.get("merge_head") or ""))
                         and github.get("base_head") == merged.get("merge_head"), merged))
    if ticket == 8 or ticket == 5 and state.get("canary_revealed"):
        required.append(("release", released.get("candidate_head") == head
                         and released.get("exists") is True
                         and bool(SHA.fullmatch(released.get("tag_head") or ""))
                         and released.get("tag_head") == merged.get("merge_head"), released))
    if ticket == 8:
        closed_by_subject = any(entry.get("operation") == "close_issue"
                                and entry.get("origin") == "subject"
                                and entry.get("ticket") == 8 and entry.get("status") == "completed"
                                for entry in github.get("journal", []))
        required.append(("issue_closed", issue.get("state") == "closed" and closed_by_subject, issue))
    return [{"id": f"github.{name}", "passed": bool(passed),
             "reason": "Verified run-scoped GitHub readback" if passed else "Required GitHub evidence is missing or mismatched",
             "evidence": evidence or None} for name, passed, evidence in required]
