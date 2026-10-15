#!/usr/bin/env python3
"""Exercise the packaged binary's defaults with real Registry dependencies.

The candidate archive replaces only the harness-e2e artifact in a frozen lock;
the Engine, Compose and every dependency run normally. No control namespace
or E2E database configuration is supplied. Requires iii and PyYAML.

An unreachable Registry is not a defect of the package: after three tries
the smoke emits a GitHub warning and exits 0. Anything else still fails.
"""

import argparse
import functools
import hashlib
import http.client
import http.server
import json
import os
import pathlib
import signal
import socket
import subprocess
import tarfile
import threading
import time
import urllib.error
import urllib.request

import yaml


ROOT = pathlib.Path(__file__).resolve().parents[1]
NAMESPACE = "registry-install-smoke"
DATABASE = "sql-service"
CONFIG = "operator-sql-config"
REGISTRY = "https://api.workers.iii.dev"
ATTEMPTS = 3


class RegistryUnavailable(Exception):
    """The Registry did not answer; says nothing about the package."""


def registry_outage(stderr):
    """Compose could not reach the Registry, or a dependency download failed
    in transport, 5xx or 429. Client errors, the local candidate's download
    and digest mismatches (their own code) are real."""
    if "REGISTRY_UNREACHABLE" in stderr:
        return True
    if "PACKAGE_DOWNLOAD_FAILED" not in stderr or "127.0.0.1" in stderr:
        return False
    return "client error" not in stderr or "429" in stderr


def resolve(registry):
    request = urllib.request.Request(
        registry + "/resolve",
        data=json.dumps({"worker": "harness-e2e", "version": "latest"}).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code < 500 and error.code != 429:
            raise
        error.close()
        raise RegistryUnavailable(f"{registry}/resolve answered {error.code}") from error
    except (OSError, http.client.HTTPException) as error:
        raise RegistryUnavailable(f"{registry}/resolve: {error!r}") from error


def run(binary, directory, registry=REGISTRY):
    directory.mkdir(parents=True, exist_ok=True)
    directory = directory.resolve()
    manifest = json.loads(subprocess.check_output([str(binary), "--manifest"], text=True))
    resolved = resolve(registry)
    (directory / "registry-resolution.json").write_text(json.dumps(resolved, indent=2))
    archive = directory / "candidate.tar.gz"
    with tarfile.open(archive, "w:gz") as output:
        output.add(binary, arcname="harness-e2e")

    class QuietHandler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(QuietHandler, directory=str(directory)),
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    engine = f"ws://127.0.0.1:{port}"
    config_dir = directory / "configuration"
    (directory / "engine.yaml").write_text(yaml.safe_dump({"workers": [
        {"name": "iii-worker-manager", "config": {"host": "127.0.0.1", "port": port}},
        {"name": "configuration", "config": {"adapter": {
            "name": "fs", "config": {"directory": str(config_dir)},
        }}},
    ]}))
    env = dict(os.environ, III_TELEMETRY_ENABLED="false",
               III_COMPOSE_STATE_DIR=str(directory / "compose-state"))
    processes = []
    compose = directory / "worker-compose.yaml"
    containers, lock = {}, {}
    for worker in resolved["graph"]:
        name = DATABASE if worker["name"] == "database" else worker["name"]
        version = manifest["version"] if name == "harness-e2e" else worker["version"]
        reference = "package://api.workers.iii.dev/" + worker["name"]
        containers[name] = {"worker": reference, "version": version}
        package = {"name": worker["name"], "version": version, "type": "binary",
                   "artifacts": worker["binaries"], "default_config": worker["config"]}
        if name == "harness-e2e":
            package["default_config"] = yaml.safe_load((ROOT / "config.yaml").read_text())
            package["artifacts"] = {manifest["supported_targets"][0]: {
                "url": f"http://127.0.0.1:{server.server_port}/candidate.tar.gz",
                "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
            }}
            containers[name]["config_override"] = {"data_dir": str(directory / "evidence")}
            containers[name]["start_after"] = [
                DATABASE if edge["to"] == "database" else edge["to"]
                for edge in resolved["edges"] if edge["from"] == "harness-e2e"
            ]
        elif name == DATABASE:
            containers[name]["config_name"] = CONFIG
        lock[name] = {"worker": reference, "requested": version, "resolved": package}

    def topology(names):
        compose.write_text(yaml.safe_dump({"namespace": NAMESPACE, "containers": {
            name: containers[name] for name in names
        }}, sort_keys=False))
        compose.with_suffix(".lock").write_text(yaml.safe_dump({
            "version": 1, "containers": {name: lock[name] for name in names},
            "graphs": {},
        }, sort_keys=False))

    def call(function, payload=None, namespace=NAMESPACE, timeout=30):
        result = subprocess.run([
            "iii", "trigger", function, "--engine", engine, "--namespace", namespace,
            "--timeout-ms", str(timeout * 1000), "--json", json.dumps(payload or {}),
        ], cwd=directory, env=env, capture_output=True, text=True, timeout=timeout + 5)
        if result.returncode:
            if registry_outage(result.stderr):
                raise RegistryUnavailable(result.stderr.strip())
            raise RuntimeError(result.stderr)
        return json.loads(result.stdout)

    def wait(function, payload=None, namespace=NAMESPACE):
        deadline = time.monotonic() + 75
        while True:
            try:
                return call(function, payload, namespace, timeout=2)
            except (RuntimeError, subprocess.TimeoutExpired) as error:
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"{function} never became ready: {error}") from error
                time.sleep(0.2)

    def start():
        for name, arguments in [
            ("engine", ["iii", "--config", str(directory / "engine.yaml"), "--no-update-check"]),
            ("compose", ["iii", "compose", "--engine", engine, "--namespace", NAMESPACE]),
        ]:
            with (directory / f"{name}.log").open("a") as log:
                processes.append(subprocess.Popen(arguments, cwd=directory, env=env,
                    stdout=log, stderr=subprocess.STDOUT, start_new_session=True))
            wait("engine::workers::list" if name == "engine" else "compose::list",
                 namespace="default" if name == "engine" else NAMESPACE)

    def stop():
        if processes:
            try:
                call("compose::down", {"file": str(compose)}, timeout=15)
            except (RuntimeError, subprocess.TimeoutExpired):
                pass
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        processes.clear()

    try:
        for attempt in range(2):
            topology(containers)
            start()
            if attempt == 0:
                call("compose::up", {"file": str(compose), "container": DATABASE, "frozen": True}, timeout=120)
            before = wait("configuration::get", {"id": CONFIG, "raw": True}, "default")["value"]
            assert "harness_e2e" not in before["databases"], "saved database config was modified"
            saved = (config_dir / f"{CONFIG}.yaml").read_bytes()
            call("compose::up", {"file": str(compose), "frozen": True}, timeout=120)
            catalog = wait("e2e::scenarios-list")
            assert catalog["runner"]["name"] == "harness-e2e"
            assert catalog["scenarios"], "worker registered without a scenario catalog"
            after = call("configuration::get", {"id": CONFIG, "raw": True}, "default")["value"]
            assert after["databases"]["primary"] == before["databases"]["primary"]
            assert after["history_max_entries"] == before["history_max_entries"]
            pool = after["databases"]["harness_e2e"]
            assert pool["pool"]["max"] == 1
            assert pool["url"] == "sqlite:" + str(directory / "evidence/control.sqlite")
            assert (config_dir / f"{CONFIG}.yaml").read_bytes() == saved
            assert call("database::query", {"db": "primary", "sql":
                "SELECT name FROM sqlite_master WHERE name = 'executions'"})["row_count"] == 0
            if attempt == 0:
                call("database::transaction", {"db": "harness_e2e", "statements": [
                    {"sql": "CREATE TABLE bootstrap_smoke (value TEXT)"},
                    {"sql": "INSERT INTO bootstrap_smoke VALUES ('retained')"},
                ]})
            else:
                retained = {"db": "harness_e2e", "sql": "SELECT value FROM bootstrap_smoke"}
                assert call("database::query", retained)["rows"] == [{"value": "retained"}]
                # A saved edit replaces the runtime value, dropping the pool
                # before it returns; only the worker can have put it back.
                call("configuration::set", {"id": CONFIG, "value": before, "flush": True}, "default")
                deadline = time.monotonic() + 30
                while "harness_e2e" not in call("configuration::get", {"id": CONFIG, "raw": True},
                                                "default")["value"]["databases"]:
                    assert time.monotonic() < deadline, "the E2E pool was not restored after a save"
                    time.sleep(0.2)
                assert wait("database::query", retained)["rows"] == [{"value": "retained"}]
            print(f"PASS: packaged install {'after full restart' if attempt else 'from defaults'}; "
                  f"{len(catalog['scenarios'])} scenarios; saved database settings unchanged", flush=True)
            stop()
    finally:
        stop()
        server.shutdown()
        server.server_close()


def main(binary, directory, registry=REGISTRY, delay=30):
    """Retry a Registry outage with backoff, then warn instead of failing."""
    for attempt in range(1, ATTEMPTS + 1):
        try:
            return run(binary, directory / f"attempt-{attempt}", registry)
        except RegistryUnavailable as error:
            reason = " ".join(str(error).split())[:500]
            if attempt == ATTEMPTS:
                print(f"::warning title=Registry unreachable::Packaged install not validated "
                      f"after {ATTEMPTS} attempts: {reason}", flush=True)
                return
            print(f"Registry unreachable (attempt {attempt}/{ATTEMPTS}): {reason}; "
                  f"retrying in {delay}s", flush=True)
            time.sleep(delay)
            delay *= 2


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--registry", default=REGISTRY,
                        help="Registry API base; an unreachable one rehearses an outage")
    args = parser.parse_args()
    main(args.binary.resolve(), args.output, args.registry)
