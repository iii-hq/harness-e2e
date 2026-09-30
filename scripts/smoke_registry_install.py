#!/usr/bin/env python3
"""Exercise the packaged binary's defaults with real Registry dependencies.

The candidate archive replaces only the harness-e2e artifact in a frozen lock;
the Engine, Compose and every dependency run normally. No control namespace
or E2E database configuration is supplied. Requires iii and PyYAML.
"""

import argparse
import functools
import hashlib
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
import urllib.request

import yaml


ROOT = pathlib.Path(__file__).resolve().parents[1]
NAMESPACE = "registry-install-smoke"
DATABASE = "sql-service"
CONFIG = "operator-sql-config"


def run(binary, directory):
    directory.mkdir(parents=True, exist_ok=True)
    directory = directory.resolve()
    manifest = json.loads(subprocess.check_output([str(binary), "--manifest"], text=True))
    request = urllib.request.Request(
        "https://api.workers.iii.dev/resolve",
        data=json.dumps({"worker": "harness-e2e", "version": "latest"}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        resolved = json.load(response)
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    run(args.binary.resolve(), args.output)
