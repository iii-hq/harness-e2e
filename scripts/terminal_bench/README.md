# Terminal-Bench with the iii harness

`iii_harbor_agent.py` is a [Harbor](https://github.com/harbor-framework/harbor)
external agent, `iii_harbor_agent:IIIHarnessAgent`. Harbor keeps the task
container and its own verifier, so scores are comparable with the public
Terminal-Bench leaderboard. For each trial the agent:

1. installs `curl bash nodejs npm ripgrep` with Harbor's own helper, the same
   packages Harbor's `codex` agent installs before its first model call;
2. registers one bridge function, `workspace::t<12 hex>::exec`, that runs a command
   in the trial's container through Harbor's `environment.exec`;
3. calls `harness::send` with only that function allowed;
4. polls `harness::metrics` and `harness::status` every 5 s until the session
   tree is done, filling Harbor's token and cost counters as it goes;
5. on a Harbor timeout, calls `harness::stop`, collects the final state and
   unregisters the function. Harbor then grades the container as it is.

It never touches the live stack: the agent requires `engine_url` and
`namespace`, never reads `III_URL`, and every command below names
`--engine ws://127.0.0.1:49634 -n tbench`.

## Agent options

| Option | Meaning |
|---|---|
| `-m <provider>/<model>` | Split on the first `/`: `anthropic/claude-sonnet-4` gives provider `anthropic` and router catalog id `claude-sonnet-4`. OpenAI: `openai/gpt-6.2-sol`, or `openai-codex/codex/gpt-6.2-sol` on a ChatGPT plan. Codex catalog ids carry `codex/`: a bare slug misses the catalog, the context manager falls back to 8K limits, and an overflow ends as `harness.unknown_model` (`ModelNotFoundError`). |
| `--ak engine_url=...` | Required. The isolated engine, `ws://127.0.0.1:49634`. |
| `--ak namespace=...` | Required. `tbench`. |
| `--ak prompt=stock` | Default. The harness's own system prompt and default `agent_call` exposure; the task message names the bridge function, as the Kanban family does. |
| `--ak prompt=override` | A short Terminal-Bench system prompt (`system_prompt_strategy: override`) and `native` exposure, so the bridge is a real tool. |
| `--ak max_turns=1000000` | Generate steps per turn. The default leaves Harbor's task timeout as the only limit, as for terminus-2. A turn that runs out is graded as it stands. |
| `--ak thinking_level=<level>` | Router thinking level: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`. Without it the provider's default applies (`minimal` for anthropic, openai and openai-codex; GPT-6.x sends that as effort `low`, 6 Sol and 6 Luna as `none`) and the metadata records `provider default`. `xhigh` is the ceiling; `max` is rejected. A published number names its level. |

The bridge runs each command in a fresh `bash -c`, merges stdout and stderr, and
keeps the first and last 20 KB, about where Codex CLI's default 10000-token cut falls. `timeout_sec` defaults to 120 and is capped at
270, under the harness's 300 s call limit. A command still running then is not
killed: the result carries its output so far, its `pid` and `log_path`. The
command leads its own process group, so `kill -- -PID` ends its whole tree, and
it keeps the default SIGINT handling.

There is no interactive terminal: the model cannot type into a program that is
already running (a serial console, a REPL, an ssh prompt), which terminus-2
does through tmux. Before blaming the harness for a failed task, check its
`exec.jsonl` for that; if it is the cause, add a tmux-backed terminal function.

## What a trial leaves

In `jobs/<job>/<trial>/agent/`: `iii-send.json` (request and response, written at trial end),
`system-prompt.json`, `exec.jsonl` (one line per bridge call), `metrics.json`,
`status.json` and `transcript.json`. `result.json` holds the usage and
`agent_result.metadata` (harness session id, status, stop reason, failure,
function calls, prompt mode, thinking level, and from the transcript `models_seen`,
`generations`, `reasoning_generations` and `first_model_response`, plus the
`base_tools` probe and `packages_installed`). A generation is an assistant
message with content or output tokens: harness 1.10 appends an empty assistant
before each provider call and fills in only its content and usage, so a failed
call leaves an empty one, and `models_seen` is always the requested id.
`tb_trace_scan.py` adds `agent/trace-scan.json` after the trial (see the rounds
block below). Sessions are stamped `kind: e2e`, title
`Harness E2E: <task>`, metadata `e2e_run_id` (trial), `e2e_scenario` (task)
and `e2e_execution_kind: harbor`, for the later harness-e2e integration.
`agent/` is `/logs/agent` inside the task container, so nothing secret is
written there.

## How failures count

This section is for single-arm runs with `-r 1`. Head-to-head rounds set
`max_retries: 0` and score cells by phase with `tb_pair.py`; see
[Reference arm](#reference-arm-codex-cli-same-model).

Harbor classifies an error by exception class name
(`harbor/models/trial/error_policy.py`) and leaves a trial with an invalid
reward out of the mean and pass@k. So only provider and transport outages raise
an invalid-reward name, as litellm errors do for terminus-2. A failure of the
harness or the model raises `HarnessTurnError`, a name Harbor does not know, and
counts as a 0 like a terminus-2 crash. "Retried" needs `-r` above 0; the runbook
passes `-r 1`.

| Harness outcome | Raised | Harbor |
|---|---|---|
| rate limited | `ApiRateLimitError` | retried, reward invalid |
| other retryable failure, provider unavailable, stream failure | `ApiOverloadedError` | retried, reward invalid |
| engine unreachable, `harness::send` failed, polls failing | `ApiConnectionError` | retried, reward invalid |
| provider rejected the request (no API key, Codex not signed in, wrong model id, ChatGPT plan `usage_limit_reached`, API `insufficient_quota`), turn record gone | `ApiError` | retried, reward invalid |
| auth expired (401/403, revoked or expired Codex session), unresolved `secret://` key reference | `AgentAuthenticationError` | not graded |
| unknown model or provider (`harness.unknown_model`, `router/no_provider_for_model`, `router/ambiguous_model`) | `ModelNotFoundError` | not graded |
| context overflow | `ContextWindowExceededError` | graded, reward valid |
| any other harness failure, llm-router's own in-flight cap (`router/capacity_exceeded`), a cancel Harbor did not ask for, an open ask, zero function calls | `HarnessTurnError` | retried, graded, reward valid |
| Harbor's agent timeout | (Harbor's `AgentTimeoutError`) | graded, reward valid |

After a quota or plan-limit wall in a single-arm run, rerun the dropped trials with
`harbor job resume -p jobs/<job> -f ApiError -f ApiOverloadedError -f CancelledError`
(`-f` replaces the `CancelledError` default). Never in a head-to-head round.

## Runbook

Run it from a normal terminal whose user is in the `docker` group. The shell is
zsh, so repeated flags live in arrays and expand as `"${E[@]}"`.

### 0. Paths

```bash
export TB=~/iii-tbench
export ADAPTER="$(git rev-parse --show-toplevel)/scripts/terminal_bench"   # run inside the harness-e2e checkout
mkdir -p "$TB" && cd "$TB"
E=(--engine ws://127.0.0.1:49634 -n tbench)
```

### 1. Python environment

Harbor and the iii SDK share one venv because the agent imports both.

```bash
uv venv --python 3.13 "$TB/venv"
uv pip install --python "$TB/venv/bin/python" 'harbor==0.27.0' 'iii-sdk==0.26.0'
"$TB/venv/bin/python" -c 'import harbor, iii; print("ok")'
```

### 2. Isolated scoring stack

`worker-compose.yaml` here is the stack: engine on 49634, namespace `tbench`,
harness with `co_author: false` and `default_filesystem_root: "off"`, and no
memory, approval-gate, judge, fp or ade. For an OpenAI model, edit the copy
first: see [OpenAI models](#openai-models).

```bash
cp "$ADAPTER/worker-compose.yaml" "$TB/worker-compose.yaml"    # asks before replacing pinned versions
cd "$TB" && env -u III_URL -u III_NAMESPACE III_TELEMETRY_ENABLED=false \
  III_COMPOSE_STATE_DIR="$TB/.compose-state" iii compose --up      # stays in the foreground
```

If harness does not boot without `fp`, add
`fp: {worker: package://fp, version: latest, config_override: {inject_guidance: false}}`.

From a second terminal, the first two must print a description and the last
two must fail, because memory and approval-gate must be absent:

```bash
iii call harness::send -h "${E[@]}"
iii call harness::metrics -h "${E[@]}"
iii call memory::save -h "${E[@]}"
iii call approval::list-pending -h "${E[@]}"
```

List the stack's router catalog ids, from the harness-e2e checkout:

```bash
III_NAMESPACE=tbench III_TELEMETRY_ENABLED=false \
  cargo run --locked --bin harness-e2e -- models --url ws://127.0.0.1:49634 --provider <anthropic|openai|openai-codex>
```

#### OpenAI models

The shipped `worker-compose.yaml` stays on Anthropic; edit the copy in `$TB`
before `--up`.

API key, `provider-openai`: add `OPENAI_API_KEY: "${OPENAI_API_KEY:-}"` to the
`llm-router` `environment` (the router hands the key to the provider), and add:

```yaml
  provider-openai:
    worker: package://provider-openai
    version: latest
    start_after: [llm-router, state]
    environment: {OPENAI_API_KEY: "${OPENAI_API_KEY:-}"}
```

ChatGPT plan, `provider-openai-codex`: local personal development only, never a
published run. It drives the undocumented Codex backend, plausibly against
OpenAI's terms, and the plan's usage limit can stop a long run
(`usage_limit_reached`, see [How failures count](#how-failures-count)). Its
login lives in the `secrets` vault:

```yaml
  secrets:           {worker: package://secrets,           version: latest}
  provider-openai-codex:
    worker: package://provider-openai-codex
    version: latest
    start_after: [llm-router, state, secrets]
```

Sign in from the second terminal with a device code:

```bash
iii call provider::openai-codex::login::start --json '{}' "${E[@]}"          # verification_uri, user_code, login_id
# open verification_uri, enter user_code, approve
iii call provider::openai-codex::login::poll login_id=<login_id> "${E[@]}"   # repeat until status is ok
iii call provider::openai-codex::auth::status --json '{}' "${E[@]}"          # must show source: managed
```

Until `auth::status` shows `source: managed`, the provider silently uses
`~/.codex/auth.json` (read-only) when it exists. The vault's master key is
written outside `$TB`, to `~/.config/iii/secrets/<vault_id>.key` (`vault_id` is
in the vault's `vault.json`), so it outlives `gio trash ~/iii-tbench`: trash
that one file too, never the other keys there.

### 3. Adapter self-check (no engine, no docker, no model)

From the harness-e2e checkout:

```bash
python3 -m unittest -v tests/python/test_terminal_bench_agent.py tests/python/test_terminal_bench_pair.py
```

### 4. Docker and verifier check with Harbor's oracle agent (no model cost)

```bash
cd "$TB" && ./venv/bin/harbor run -d terminal-bench-sample@2.0 -i regex-log -a oracle \
  -n 1 -o jobs --job-name oracle-regex-log -e docker
```

Expect reward 1 in `jobs/oracle-regex-log/result.json`.

### 5. One-task smoke

```bash
cd "$TB" && export PYTHONPATH="$ADAPTER"
MODEL='<provider>/<catalog id from step 2>'
THINK='<off|minimal|low|medium|high|xhigh>'
AK=(--ak engine_url=ws://127.0.0.1:49634 --ak namespace=tbench --ak "thinking_level=$THINK")
RUN=(./venv/bin/harbor run -a iii_harbor_agent:IIIHarnessAgent -m "$MODEL" "${AK[@]}" -r 1 -o jobs -e docker)

"${RUN[@]}" -d terminal-bench-sample@2.0 -i regex-log -n 1 --job-name smoke-regex-log --print-config  # resolves only
"${RUN[@]}" -d terminal-bench-sample@2.0 -i regex-log -n 1 --job-name smoke-regex-log
```

Never pass provider keys with `--ae`: Harbor puts that environment on container
execs. Gate before going on; look in `jobs/smoke-regex-log/<trial>/`:

- `result.json`: `verifier_result.rewards` is present, `exception_info` is null,
  and the token counts are set (`cost_usd` is null when the router catalog has
  no pricing for the model, as for every GPT-6.x on both OpenAI providers).
- `agent/metrics.json`: `totals.function_calls > 0`.
- `agent/system-prompt.json`: the `runtime` part lists only the
  `workspace::t<hex>::exec` id and has no working-directory or `Co-authored-by`
  line. With `prompt=override` the `built_in` part is the Terminal-Bench
  prompt. Note any `skills` or `injected` part.
- `agent/exec.jsonl`: the commands ran in the container.

### 6. Ten-task sample in both prompt modes

The published number must say which mode it uses. Run the sample both ways and
compare the mean reward, cost and errors in each `result.json`:

```bash
for P in stock override; do
  "${RUN[@]}" -d terminal-bench-sample@2.0 --ak prompt=$P -n 4 --job-name sample-$P-$(date +%m%d-%H%M)
done
```

### 7. Full run

The default tbench.ai leaderboard ranks Terminal-Bench 4.1
(`terminal-bench/terminal-bench@4.1.0`, 66 tasks). An entry needs at least 5
attempts per task (`-k 5`), the default timeouts (8 h per task) and no resource
overrides. Three of the 66 tasks need an H100; on a local Docker host run the
63-task `terminal-bench/terminal-bench-cpu-only@4.1.0` and compare against the
leaderboard rows recomputed on those same 63 tasks, not the published
66-task numbers. The CPU tasks ask for 2 to 16 CPUs and 4 to 16 GB each, so keep
`-n` low enough for the host.

Pin exact worker versions in `worker-compose.yaml`, and keep
`worker-compose.lock` and the dataset digest with the job. Publish the mean
with its confidence interval, the prompt mode, the thinking level and Harbor's
`exception_stats`.

```bash
"${RUN[@]}" -d terminal-bench/terminal-bench-cpu-only@4.1.0 --ak prompt=<chosen> -n 4 -k 5 --job-name tb41-$(date +%m%d-%H%M)
```

Resume an interrupted job with `./venv/bin/harbor job resume` (see
`harbor job --help`).

### 8. Results

```bash
./venv/bin/harbor view ./jobs        # local viewer on 8080-8089: reward, cost, tokens, agent files
cat jobs/<job>/result.json           # pass@k, reward stats, token and cost totals
```

### 9. Shutdown

Press Ctrl-C in the compose terminal. Never `pkill` iii processes: the live
stack shares the machine. Remove the folder with `gio trash ~/iii-tbench`.

## Reference arm: Codex CLI, same model

A head-to-head run puts both arms in one Harbor job, so they share the quota
windows, host load and model drift. Only the harness differs.

| Arm | What runs |
|---|---|
| `iii` | iii loop + stock prompt + provider, bash-only bridge. Harness 1.10.0, llm-router 1.5.7 and provider-openai-codex 0.5.6 on the isolated stack, `prompt=stock`, `thinking_level=xhigh`, model `openai-codex/codex/gpt-6.2-sol`. Every stock tool is denied and the model gets only the bridge, so a result is about this setup, not about iii's own tool surface. |
| `codex` | Harbor's built-in `codex` agent: Codex CLI 0.170.0 inside the task container with its own prompt and tools, `reasoning_effort=xhigh`, `web_search=disabled`, model `openai/gpt-6.2-sol`. |

The isolated stack runs from `$TB/worker-compose.yaml`, pinned to the versions
above, not from this folder's copy, which tracks `latest` with
provider-anthropic. The preregistration's code line records the sha256 of that
file and of its `worker-compose.lock`.

`h2h-ic.yaml` lists iii first and `h2h-ci.yaml` Codex first; rounds alternate
ic, ci, ic. Both set one attempt and `max_retries: 0`, because Harbor deletes a
retried attempt and would retry the iii arm's unknown exception names only.
Both arms get the same packages before their first model call (step 1 above).
Put the dataset on the command line: `-i` and `-x` are refused without `-d`.

### Shared login

Both arms use the host ChatGPT login `~/.codex/auth.json`: the iii provider
reads it read-only (`source: local`) and Harbor copies it into each Codex
container, where commands the Codex model runs can read it. Set
`CODEX_FORCE_AUTH_JSON=1` in the host environment, never with `--ae`, which
reaches container execs, and unset `OPENAI_API_KEY` and `CODEX_API_KEY`:
without the flag Harbor's `codex` agent silently takes the API key, another
account with per-call prices. `h2h_watch` refuses to start otherwise, and
`tb_pair.py` fails a Codex trial whose rollout shows no ChatGPT plan.

Right before the pilot, run a fresh `codex login` on the host: it resets both
clocks below. No stack restart is needed, because the provider reads the file
on every call. Before and after every round, the pilot included, record the
two lines below; neither prints a secret.

```bash
sha256sum ~/.codex/auth.json | cut -d' ' -f1
python3 -I -c 'import json,base64,os,time
from datetime import datetime
a=json.load(open(os.path.expanduser("~/.codex/auth.json")))
p=a["tokens"]["access_token"].split(".")[1]; p+="="*(-len(p)%4)
exp=json.loads(base64.urlsafe_b64decode(p))["exp"]
ref=datetime.fromisoformat(a["last_refresh"]).timestamp()+7*86400
print("hours left: access token %.1f, last_refresh + 7 d %.1f" % ((exp-time.time())/3600, (ref-time.time())/3600))'
```

- A round starts only when both numbers are above 12, one round's planned
  worst case. The second bound keeps every Codex container from refreshing,
  because a refresh rotates the token in a copy that is then thrown away.
- A hash that changed during a round means something on the host refreshed the
  login. The next round waits for a fresh `codex login` on the host and a new
  check. The hash sees host-side changes only: Harbor uploads a copy into each
  Codex container and deletes it with the container, so a refresh in there never
  changes the host file. The 12 h bound is the guard against that.
- Do not use Codex CLI on the host while a round runs.

### Decoys, pilot and rounds

```bash
cd "$TB" && export PYTHONPATH="$ADAPTER" CODEX_FORCE_AUTH_JSON=1 && unset OPENAI_API_KEY CODEX_API_KEY
D=(-d terminal-bench/terminal-bench-cpu-only@4.1.0)
h2h_watch() {  # h2h_watch <job> <command...>: Ctrl-C the job on the first quota or auth error
  local job=$1; shift
  if [ "${CODEX_FORCE_AUTH_JSON:-}" != 1 ] || [ -n "${OPENAI_API_KEY:-}${CODEX_API_KEY:-}" ]; then
    echo "H2H_REFUSED: needs CODEX_FORCE_AUTH_JSON=1 and no OPENAI_API_KEY or CODEX_API_KEY"; return 2
  fi
  "$@" & local pid=$!
  while kill -0 $pid 2>/dev/null; do
    if grep -rlsE --include=result.json '"exception_type": "(ApiRateLimitError|ApiUsageLimitError|AgentAuthenticationError|AuthenticationError)"|usage_limit_reached' "jobs/$job"; then
      kill -INT $pid; wait $pid; echo "H2H_STOP: $job"; return 1
    fi
    sleep 30
  done
  wait $pid
}

# Decoys, no model: nop must score a valid 0 and oracle 1 on every task.
./venv/bin/harbor run "${D[@]}" -a nop    -n 8 -o jobs -e docker --job-name h2h-tb41-nop
./venv/bin/harbor run "${D[@]}" -a oracle -n 8 -o jobs -e docker --job-name h2h-tb41-oracle
X=()   # one -x terminal-bench/<task> per task that missed a decoy (4.1 names carry the prefix); more than 6 ends the plan

# Pilot: 3 tasks per arm, interleaved.
P=(-i terminal-bench/heat-pump-warranty -i terminal-bench/session-window-debug -i terminal-bench/glycan-ms2-elucidation)
h2h_watch h2h-pilot ./venv/bin/harbor run -c "$ADAPTER/h2h-ic.yaml" "${D[@]}" "${P[@]}" -n 2 --n-concurrent-agents 1 --job-name h2h-pilot
python3 -I "$ADAPTER/tb_trace_scan.py" jobs/h2h-pilot     # read-only; writes agent/trace-scan.json per iii trial
python3 -I "$ADAPTER/tb_pair.py" --pilot jobs/h2h-pilot     # one line per trial, then PILOT_READY: yes|no

# Rounds, after the owner approves the pilot report and the login checks pass.
R=1; O=ic    # round 2: O=ci; round 3: O=ic
df -BG --output=avail /var/lib/docker | tail -1   # under 50G: stop, no round starts
date -u    # rounds 2 and 3 start no later than 30 h after round 1 started
h2h_watch h2h-tb41-r$R ./venv/bin/harbor run -c "$ADAPTER/h2h-$O.yaml" "${D[@]}" "${X[@]}" -n 8 --n-concurrent-agents 4 --job-name h2h-tb41-r$R
python3 -I "$ADAPTER/tb_trace_scan.py" jobs/h2h-tb41-r$R   # right away: the trace store rotates
python3 -I "$ADAPTER/tb_pair.py" --gate jobs/h2h-tb41-r$R   # exit 1: no further round
python3 -I "$ADAPTER/tb_pair.py" jobs/h2h-tb41-r1 jobs/h2h-tb41-r2 jobs/h2h-tb41-r3   # a round that fails its gate is VOID
```

`tb_pair.py` classifies each trial by phase, the same way for both arms: a
quota or auth error is a stop cell; an exception before the arm's first model
response, or a verifier failure Harbor grades invalid, is infra, and drops that
task and round from both arms; anything else is valid and scores the graded
reward, or 0. It prints the per-task table, the paired 95% bootstrap interval
over tasks, the realized mdd, the drop-invalid sensitivity reading (with
`HARBOR_INVALID` lines when it disagrees), reasoning tokens per generation per
arm and the `VERDICT`. Its last line is the JSON counts. Each round is gated on
its own: a stop cell; an effort-parity failure (Codex: a rollout `turn_context`
off `gpt-6.2-sol`/`xhigh` or no ChatGPT `plan_type`; iii: `thinking_level` off
`xhigh` or a failing trace scan); more than 2 valid cells per arm without
effort evidence; no passing trace scan in the round; more than 3 infra cells
per arm; a retried trial; a planned trial with no readable `result.json`; or a
task with one arm only. `--gate` exits 1 on any of them; the plain run lists
such a round as `VOID` and leaves it out of the verdict.

`tb_trace_scan.py` reads, through the `iii` CLI and without changing anything,
the trace of each iii session's `harness::send` on the isolated stack: every
turn step record must carry `thinking_level` `xhigh`, every provider input
`xhigh` or be cut by the engine's 64 KiB payload cap, and at least one provider
input must be uncut. The provider sends `xhigh` to gpt-6.2-sol unchanged
(`provider-openai-codex/src/reasoning.rs`, its gpt-6 ladder). An unreadable
trace is no evidence, not a failure.

### Resume, keyed on phase

- Harbor died or the host rebooted: run the round's `h2h_watch` line again with
  `./venv/bin/harbor job resume -p jobs/h2h-tb41-r$R` as the command. Resume
  re-runs unfinished trials and first removes `CancelledError` ones.
- One infra pass per round: move the trials `tb_pair.py` drops as infra out of
  the job, then resume as above, run `tb_trace_scan.py` and gate again. Cells
  still infra after that pass stay dropped.

  ```bash
  mkdir -p "$TB/infra-pass/r$R" && python3 -I "$ADAPTER/tb_pair.py" jobs/h2h-tb41-r$R |
    awk '$1 == "DROPPED:" && $6 == "infra" {print $NF}' | xargs -r mv -t "$TB/infra-pass/r$R"
  ```

- Never resume with `-f <exception>`: the same class can be infra in one trial
  and a scored 0 in another. Never resume a stop or valid cell, and nothing
  after `H2H_STOP`.
