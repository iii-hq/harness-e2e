"""Read-only effort check of the iii arm on the isolated stack's traces.

    python3 -I tb_trace_scan.py [--engine ws://127.0.0.1:49634] <job dir> [<job dir> ...]

For each iii trial it finds the trace of the session's `harness::send` (engine::traces::list),
reads its span tree (engine::traces::tree, else engine::traces::spans in pages) through the
`iii` CLI, and writes agent/trace-scan.json next to the trial's other agent files.
Rule, per session: ok when every turn step record (state::set of scope harness_turn with a
stream_request_id) carries thinking_level xhigh, every provider::openai-codex::stream input
carries xhigh or was cut by the engine's 64 KiB payload cap (counted, not a failure), and at
least one provider input is uncut. Any other level, or none in an uncut input, fails. A trace
that cannot be read, or holds no step record or no uncut provider input, is no evidence
either way (ok null). Exit 1 when any trial fails. tb_pair.py --gate and --pilot read the
files. Run it right after each round: the trace store rotates, and an archived trace of
thousands of spans can no longer be read.
"""
import argparse
import glob
import json
import os
import re
import subprocess
import sys
from collections import Counter

EFFORT, PROVIDER_SPAN, STEP_SPAN = "xhigh", "execute provider::openai-codex::stream", "execute state::set"
# The engine sorts keys, so a provider input's own thinking_level comes after "messages" and is
# the unescaped one; the step record may sit in an escaped string.
PROVIDER_LEVEL = re.compile(r'"thinking_level"\s*:\s*"([a-z]+)"')
STEP_LEVEL = re.compile(r'\\?"thinking_level\\?"\s*:\s*\\?"([a-z]+)')
STEP_ID = re.compile(r'\\?"stream_request_id\\?"\s*:\s*\\?"([^"\\]+)')
PAGE, CALL_MS = 500, 120_000


def iii_call(engine, function_id, payload, timeout_ms=CALL_MS):
    # engine::traces::* live in the default namespace; III_URL would point at the live stack.
    env = {k: v for k, v in os.environ.items() if k not in ("III_URL", "III_NAMESPACE")}
    p = subprocess.run(["iii", "call", function_id, "--engine", engine, "--json", json.dumps(payload),
                        "--timeout-ms", str(timeout_ms)], capture_output=True, text=True, env=env,
                       timeout=timeout_ms / 1000 + 30)
    if p.returncode:
        raise RuntimeError(f"{function_id}: {(p.stderr or p.stdout).strip()[-300:]}")
    return json.loads(p.stdout)


def send_traces(call):
    """session id -> trace ids of its harness::send."""
    found, offset = {}, 0
    while True:
        page = call("engine::traces::list", {"name": "harness::send", "limit": PAGE, "offset": offset})
        rows = page.get("traces") or []
        for t in rows:
            sid = (t.get("trace_tags") or {}).get("iii.session.id")
            if sid:
                found.setdefault(sid, []).append(t["trace_id"])
        offset += len(rows)
        if not rows or offset >= (page.get("total") or 0):
            return found


def fetch_tree(call, trace_id):
    try:
        return call("engine::traces::tree", {"trace_id": trace_id})
    except Exception:  # a session with thousands of spans can time out building the tree
        spans, offset = [], 0
        while True:
            page = call("engine::traces::spans", {"trace_id": trace_id, "limit": PAGE, "offset": offset})
            spans += page.get("spans") or []
            offset += len(page.get("spans") or [])
            if not page.get("spans") or offset >= (page.get("total") or 0):
                return {"roots": spans}


def _load(path):
    with open(path) as f:
        return json.load(f)


def _attrs(a):
    return dict(a) if isinstance(a, list) else (a or {})


def scan(tree):
    steps, provider = {}, Counter()
    stack = list(tree.get("roots") or [])
    while stack:
        n = stack.pop()
        stack += n.get("children") or []
        if n.get("name") not in (PROVIDER_SPAN, STEP_SPAN):
            continue
        for e in n.get("events") or []:
            if e.get("name") != "iii.invocation.input":
                continue
            a = _attrs(e.get("attributes"))
            p = a.get("iii.payload.json") or ""
            if n["name"] == PROVIDER_SPAN:
                levels = set(PROVIDER_LEVEL.findall(p))
                cut = str(a.get("iii.payload.truncated")).lower() == "true"
                provider.update(levels or {"cut" if cut else "absent"})
            elif '"harness_turn"' in p and (sid := STEP_ID.search(p)):
                steps.setdefault(sid.group(1), set()).update(STEP_LEVEL.findall(p) or {"absent"})
    step_levels = Counter(level for levels in steps.values() for level in levels)
    return {"steps": len(steps), "step_levels": dict(step_levels), "provider_inputs": dict(provider)}


def verdict(counts):
    """(ok, why); ok is None when the trace shows no level downstream of the harness either way."""
    wrong = ["step at %s" % level for level in sorted(counts["step_levels"]) if level != EFFORT]
    wrong += ["provider input at %s" % level for level in sorted(counts["provider_inputs"])
              if level not in (EFFORT, "cut")]
    if wrong:
        return False, wrong
    if not counts["steps"]:
        return None, ["no turn step record"]
    if not counts["provider_inputs"].get(EFFORT):
        return None, ["no uncut provider input"]
    return True, []


def scan_trial(call, trial, traces):
    meta = ((_load(os.path.join(trial, "result.json")).get("agent_result") or {}).get("metadata") or {})
    sid = meta.get("session_id")
    out = {"session_id": sid}
    try:
        if not traces.get(sid):
            raise RuntimeError("no harness::send trace for the session")
        counts = scan({"roots": [r for t in traces[sid] for r in fetch_tree(call, t).get("roots") or []]})
        ok, why = verdict(counts)
        out.update(trace_ids=traces[sid], **counts, ok=ok, why=why)
    except Exception as e:
        out.update(ok=None, why=[str(e)])
    with open(os.path.join(trial, "agent", "trace-scan.json"), "w") as f:
        json.dump(out, f, indent=1)
    return out


def iii_trials(job):
    for path in sorted(glob.glob(os.path.join(job, "*", "result.json"))):
        try:
            name = (_load(path).get("agent_info") or {}).get("name") or ""
        except json.JSONDecodeError:
            continue
        if "iii" in name.lower():
            yield os.path.dirname(path)


def main(argv=None, call=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--engine", default="ws://127.0.0.1:49634")
    ap.add_argument("jobs", nargs="+")
    args = ap.parse_args(argv)
    call = call or (lambda fn, payload: iii_call(args.engine, fn, payload))
    traces = send_traces(call)
    failed = n = 0
    for job in args.jobs:
        for trial in iii_trials(job):
            os.makedirs(os.path.join(trial, "agent"), exist_ok=True)
            r = scan_trial(call, trial, traces)
            n += 1
            failed += r.get("ok") is False
            state = {True: "OK", False: "FAIL: ", None: "NO EVIDENCE: "}[r["ok"]] + "; ".join(r["why"])
            print(os.path.basename(trial), r["session_id"], "steps", r.get("steps"), "levels", r.get("step_levels"),
                  "provider", r.get("provider_inputs"), state)
    print("TRACE_SCAN: %s trials=%d failed=%d" % ("fail" if failed else "ok", n, failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
