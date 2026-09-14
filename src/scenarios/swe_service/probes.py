#!/usr/bin/env python3
"""Trusted behavioral checks. Never copy this file into a subject workspace."""
import argparse
import contextlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def clean_env(workspace):
    env = {key: value for key, value in os.environ.items() if not key.startswith('PROFILE_')}
    env['PYTHONPATH'] = str(workspace / 'src')
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    return env


def cli(workspace, *args, timeout=15):
    result = subprocess.run([sys.executable, '-m', 'profile_service', *map(str, args)], cwd=workspace,
                            env=clean_env(workspace), capture_output=True, text=True, timeout=timeout)
    require(result.returncode == 0, 'application CLI did not complete successfully')
    return json.loads(result.stdout)


@contextlib.contextmanager
def server(workspace, db, config=None, stop_signal=signal.SIGTERM, stopped=None):
    args = [sys.executable, '-m', 'profile_service', 'serve', '--db', str(db), '--port', '0']
    if config:
        args += ['--config', str(config)]
    process = subprocess.Popen(args, cwd=workspace, env=clean_env(workspace), stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        import selectors
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        ready = selector.select(8)
        selector.close()
        require(bool(ready), 'HTTP service did not announce readiness')
        line = process.stdout.readline()
        require(bool(line), 'HTTP service exited before readiness')
        yield 'http://127.0.0.1:' + str(json.loads(line)['port'])
    finally:
        if process.poll() is None:
            os.killpg(process.pid, stop_signal)
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
        if stopped is not None:
            stopped.append(process.returncode)


def request(base, path, token='alpha-token', data=None):
    headers = {'Authorization': 'Bearer ' + token}
    if data is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(base + path, headers=headers,
                                 data=None if data is None else json.dumps(data).encode())
    try:
        with urllib.request.urlopen(req, timeout=5) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)


def event(event_id, delta, tenant='alpha', profile_id='p', name='Ada'):
    return {'event_id': event_id, 'tenant': tenant, 'profile_id': profile_id, 'delta': delta, 'name': name}


def run_check(name, workspace, scratch, previous=None, canary=False):
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(workspace / 'src'))
    from profile_service.service import Service
    from profile_service.replay import replay
    db = scratch / 'profiles.sqlite'
    config = scratch / 'config.json'

    if name == 'baseline':
        public = subprocess.run([sys.executable, '-m', 'unittest', 'discover', '-s', 'tests/reference', '-p', 'test_*.py'],
                                cwd=workspace, env=clean_env(workspace), capture_output=True, text=True, timeout=15)
        require(public.returncode == 0, 'immutable public tests failed')
        authored_directory = workspace / 'tests/agent'
        if authored_directory.is_dir() and any(authored_directory.rglob('test_*.py')):
            authored = subprocess.run(
                [sys.executable, '-m', 'unittest', 'discover', '-s', 'tests/agent', '-p', 'test_*.py'],
                cwd=workspace, env=clean_env(workspace), capture_output=True, text=True, timeout=15)
            require(authored.returncode == 0, 'authored regression tests failed')
        cli(workspace, 'put', '--db', db, '--tenant', 'alpha', '--id', 'hello', '--name', 'Ada', '--score', '2')
        payload = cli(workspace, 'get', '--db', db, '--tenant', 'alpha', '--id', 'hello')
        require(payload == {'id': 'hello', 'name': 'Ada', 'score': 2}, 'CLI persistence contract failed')
        with server(workspace, db) as base:
            status, payload = request(base, '/v1/profiles/hello')
            require(status == 200 and payload['name'] == 'Ada' and payload['score'] == 2, 'HTTP read contract failed')
        return {}

    if name == 'ticket1.config':
        config.write_text(json.dumps({'greeting': 'file', 'cache_size': 3}))
        a = Service(db, config, {'PROFILE_GREETING': 'environment'})
        b = Service(scratch / 'other.sqlite', None, {})
        require(a.settings()['greeting'] == 'environment', 'environment must override file settings')
        require(a.settings()['cache_size'] == 3, 'file settings must override defaults')
        require(b.settings()['greeting'] == 'Hello' and b.settings()['cache_size'] == 64, 'service settings leaked between instances')
        config.write_text(json.dumps({'cache_size': 7}))
        require(a.settings()['greeting'] == 'environment' and a.settings()['cache_size'] == 7, 'partial file reload lost precedence')
        require(Service(scratch / 'zero.sqlite', config, {'PROFILE_CACHE_SIZE': '0'}).settings()['cache_size'] == 0,
                'an explicit zero must override the file')
        overrides = {'greeting': 'command', 'cache_size': 0, 'cache_enabled': False,
                     'tokens': {'private-token': 'private'}}
        c = Service(db, config, {'PROFILE_GREETING': 'environment', 'PROFILE_CACHE_SIZE': '9',
                                'PROFILE_CACHE_ENABLED': 'true'}, cli_overrides=overrides)
        require(c.settings()['greeting'] == 'command' and c.settings()['cache_size'] == 0
                and c.settings()['cache_enabled'] is False, 'CLI overrides must preserve zero and false above environment/file values')
        overrides['greeting'] = 'caller mutation'
        overrides['tokens']['private-token'] = 'other'
        require(c.settings()['greeting'] == 'command' and c.settings()['tokens']['private-token'] == 'private',
                'caller mutation changed stored CLI overrides')
        returned = c.settings()
        returned['tokens']['private-token'] = 'output mutation'
        require(c.settings()['tokens']['private-token'] == 'private', 'returned settings exposed mutable internal configuration')
        d = Service(db, config, {}, cli_overrides={'greeting': 'independent', 'cache_size': 5, 'cache_enabled': True})
        require(d.settings()['greeting'] == 'independent' and d.settings()['cache_size'] == 5
                and d.settings()['cache_enabled'] is True and c.settings()['greeting'] == 'command',
                'distinct CLI option sets shared a configuration cache')
        settings = cli(workspace, 'settings', '--db', scratch / 'cli.sqlite', '--config', config,
                       '--greeting', 'CLI value', '--cache-size', '0', '--cache-enabled', 'false')
        require(settings['greeting'] == 'CLI value' and settings['cache_size'] == 0
                and settings['cache_enabled'] is False, 'CLI option parsing lost explicit zero or false')
        return {}

    if name == 'ticket2.cache':
        a = Service(db, environ={})
        b = Service(db, environ={})
        a.put('alpha', 'p', 'First', 1)
        require(a.get('alpha', 'p')['score'] == 1, 'initial read failed')
        b.put('alpha', 'p', 'Second', 9)
        require(a.get('alpha', 'p') == {'id': 'p', 'name': 'Second', 'score': 9}, 'cached profile survived an external committed revision')
        payload = a.get('alpha', 'p')
        payload['score'] = -100
        require(a.get('alpha', 'p')['score'] == 9, 'caller mutation poisoned the cache')
        for index in range(70):
            a.put('alpha', str(index), 'Bounded', index)
            a.get('alpha', str(index))
        require(len(a.cache) <= a.settings()['cache_size'], 'cache exceeded its configured capacity')
        config.write_text(json.dumps({'cache_size': 2, 'cache_enabled': True}))
        live = Service(scratch / 'live-cache.sqlite', config, {})
        for identity in ('first', 'second'):
            live.put('alpha', identity, identity, 1)
            live.get('alpha', identity)
        require(len(live.cache) == 2, 'cache did not retain the configured working set')
        config.write_text(json.dumps({'cache_size': 1, 'cache_enabled': True}))
        require(live.get('alpha', 'second')['name'] == 'second' and len(live.cache) <= 1,
                'cache hit ignored a lowered positive capacity')
        config.write_text(json.dumps({'cache_size': 0, 'cache_enabled': True}))
        require(live.get('alpha', 'second')['name'] == 'second' and len(live.cache) == 0,
                'cache hit retained entries after capacity was lowered to zero')
        config.write_text(json.dumps({'cache_size': 2, 'cache_enabled': True}))
        live.get('alpha', 'first')
        live.get('alpha', 'second')
        config.write_text(json.dumps({'cache_size': 2, 'cache_enabled': False}))
        require(live.get('alpha', 'second')['name'] == 'second' and len(live.cache) == 0,
                'cache hit retained entries after caching was disabled')
        return {}

    if name == 'ticket3.batch':
        def stream():
            for index in range(11):
                if index >= 4:
                    with sqlite3.connect(db) as connection:
                        total = connection.execute('SELECT COALESCE(SUM(score),0) FROM profiles').fetchone()[0]
                    require(total >= (index // 4) * 4, 'replay consumed input beyond its batch before persisting progress')
                yield event(str(index), 1, name=f'Name {index}')
        result = replay(db, stream(), batch_size=4)
        require(result == {'received': 11, 'applied': 11}, 'batch accounting lost events')
        require(Service(db, environ={}).get('alpha', 'p') == {'id': 'p', 'name': 'Name 10', 'score': 11}, 'ordered remainder batch was lost or reordered')
        empty = replay(db, iter(()), batch_size=4)
        require(empty == {'received': 0, 'applied': 0}, 'empty replay changed its accounting')
        for size in (0, -1, True, False, 1.5, '4'):
            try:
                replay(db, iter([event('invalid', 100)]), batch_size=size)
            except ValueError:
                pass
            else:
                raise AssertionError('batch size must be a positive integer, excluding booleans')
        cursor_db = scratch / 'cursor.sqlite'
        selected = [event('c0', 1), event('c1', 2), event('c2', 4)]
        result = replay(cursor_db, iter(selected), batch_size=2, start_cursor=2)
        require(result == {'received': 1, 'applied': 1}
                and Service(cursor_db, environ={}).get('alpha', 'p')['score'] == 4,
                'exclusive start cursor did not skip exactly the leading input positions')
        require(replay(cursor_db, iter(selected), batch_size=2, start_cursor=99) == {'received': 0, 'applied': 0},
                'cursor beyond end must produce an empty replay')
        for cursor in (-1, True, False, 1.5, '2'):
            try:
                replay(cursor_db, iter(selected), batch_size=2, start_cursor=cursor)
            except ValueError:
                pass
            else:
                raise AssertionError('start cursor must be a nonnegative integer, excluding booleans')
        events_file = scratch / 'cursor-events.json'
        events_file.write_text(json.dumps(selected))
        result = cli(workspace, 'replay', '--db', scratch / 'cli-cursor.sqlite', '--events', events_file,
                     '--batch-size', 2, '--start-cursor', 2)
        require(result == {'received': 1, 'applied': 1}, 'CLI start cursor was not forwarded to replay')
        with server(workspace, scratch / 'http-cursor.sqlite') as base:
            status, result = request(base, '/replay', data={'events': selected, 'batch_size': 2, 'start_cursor': 2})
            require(status == 200 and result == {'received': 1, 'applied': 1},
                    'HTTP start cursor was not forwarded to replay')
            status, payload = request(base, '/v1/profiles/p')
            require(status == 200 and payload['score'] == 4, 'HTTP cursor applied skipped events')
            status, result = request(base, '/replay', data={'events': selected, 'batch_size': 2, 'start_cursor': 99})
            require(status == 200 and result == {'received': 0, 'applied': 0}, 'HTTP cursor beyond end did not produce empty counts')
            status, result = request(base, '/replay', data={'events': [], 'batch_size': 2, 'start_cursor': 0})
            require(status == 200 and result == {'received': 0, 'applied': 0}, 'HTTP empty replay did not produce empty counts')
            invalids = [('batch_size', value) for value in (True, False, 0, -1, 1.5, '2')]
            invalids += [('start_cursor', value) for value in (True, False, -1, 1.5, '2')]
            for field, value in invalids:
                data = {'events': selected, 'batch_size': 2, 'start_cursor': 0}
                data[field] = value
                require(request(base, '/replay', data=data)[0] == 400, 'HTTP replay accepted invalid ' + field)
            require(request(base, '/v1/profiles/p')[1]['score'] == 4, 'rejected HTTP replay changed a profile')
        return {}

    if name == 'ticket4.recovery':
        events_path = scratch / 'events.json'
        events_path.write_text(json.dumps([event('one', 5), event('two', 7)]))
        marker = scratch / 'committed.json'
        # Instrument the real SQLite commit, never the application result. The application
        # suspends AFTER durable profile persistence, exposing the crash consistency window.
        instrument = r'''
import json, os, pathlib, runpy, signal, sqlite3, sys
workspace, db, events, marker = sys.argv[1:]
sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(workspace) / 'src'))
original_connect = sqlite3.connect
class ObservedConnection(sqlite3.Connection):
    def commit(self):
        super().commit()
        try:
            score = self.execute('SELECT COALESCE(SUM(score),0) FROM profiles').fetchone()[0]
        except sqlite3.OperationalError:
            return
        if score > 0 and not pathlib.Path(marker).exists():
            pathlib.Path(marker).write_text(json.dumps({'score': score}))
            os.kill(os.getpid(), signal.SIGSTOP)
def connect(*args, **kwargs):
    kwargs['factory'] = ObservedConnection
    return original_connect(*args, **kwargs)
sqlite3.connect = connect
sys.argv = ['profile_service', 'replay', '--db', db, '--events', events, '--batch-size', '1']
runpy.run_module('profile_service', run_name='__main__')
'''
        process = subprocess.Popen([sys.executable, '-I', '-c', instrument, str(workspace), str(db), str(events_path), str(marker)],
                                   cwd=workspace, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        try:
            deadline = time.monotonic() + 10
            while not marker.exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.02)
            require(marker.exists() and process.poll() is None, 'replay did not reach the controlled durable commit boundary')
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=3)
            require(process.returncode == -signal.SIGKILL, 'application was not terminated at the persistence boundary')
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate(timeout=3)
        cli(workspace, 'replay', '--db', db, '--events', events_path, '--batch-size', 1)
        cli(workspace, 'replay', '--db', db, '--events', events_path, '--batch-size', 2)
        final = Service(db, environ={}).get('alpha', 'p')['score']
        require(final == 12, 'restarting replay duplicated or lost an acknowledged event')
        with sqlite3.connect(db) as connection:
            markers = connection.execute('SELECT COUNT(*) FROM applied_events').fetchone()[0]
        require(markers == 2, 'event acknowledgement ledger is inconsistent')
        return {'signal': 'SIGKILL', 'persistence_boundary_reached': True,
                'application_returncode': process.returncode, 'final_score': final}

    if name == 'ticket5.v2':
        service = Service(db, environ={})
        service.put('alpha', 'p', 'Áda Lovelace', 8)
        with server(workspace, db) as base:
            status, payload = request(base, '/v2/profiles/p')
            require(status == 200 and payload == {'schema_version': 2, 'profile': {'id': 'p', 'display_name': 'Áda Lovelace', 'score': 8}},
                    'v2 profile envelope or display_name contract failed')
            status, _ = request(base, '/v2/profiles/missing')
            require(status == 404, 'missing v2 profile must return HTTP 404')
        require(service.get('alpha', 'p', version=2)['profile']['display_name'] == 'Áda Lovelace', 'v2 service read failed')
        return {}

    if name == 'ticket5.legacy_canary':
        from profile_service.client import LegacyClient
        Service(db, environ={}).put('alpha', 'legacy', 'Existing Customer', 21)
        with server(workspace, db) as base:
            client = LegacyClient(base, 'alpha-token')
            require(client.profile_name('legacy') == 'Existing Customer', 'existing client could not read an unversioned profile')
            status, payload = request(base, '/v1/profiles/legacy')
            require(status == 200 and payload == {'id': 'legacy', 'name': 'Existing Customer', 'score': 21}, 'explicit v1 response changed')
            status, payload = request(base, '/v2/profiles/legacy')
            require(status == 200 and payload['schema_version'] == 2, 'legacy compatibility displaced v2')
        return {}

    if name == 'ticket6.tenants':
        service = Service(db, environ={})
        service.put('alpha', 'shared', 'Alpha Secret', 13)
        service.put('beta', 'shared', 'Beta Secret', 29)
        require(service.get('alpha', 'shared')['name'] == 'Alpha Secret', 'alpha read resolved to another tenant')
        require(service.get('beta', 'shared')['name'] == 'Beta Secret', 'cached or persisted read crossed tenants')
        replay(db, [event('same-event', 3, 'alpha', 'shared'), event('same-event', 4, 'beta', 'shared')], batch_size=2)
        require(service.get('alpha', 'shared')['score'] == 16 and service.get('beta', 'shared')['score'] == 33,
                'event identity crossed tenant boundaries')
        with server(workspace, db) as base:
            require(request(base, '/v2/profiles/shared', 'beta-token')[1]['profile']['score'] == 33, 'authenticated tenant was not used')
            require(request(base, '/v2/profiles/shared?tenant=beta', 'alpha-token')[0] == 403, 'caller-selected tenant bypassed authenticated identity')
            require(request(base, '/v1/profiles/shared', 'unknown')[0] == 401, 'unknown token was accepted')
            status, _ = request(base, '/replay', 'alpha-token', {'events': [event('foreign', 100, 'beta', 'shared')]})
            require(status == 403, 'replay allowed a write outside the authenticated tenant')
        require(Service(db, environ={}).get('beta', 'shared')['score'] == 33, 'rejected cross-tenant replay changed data')
        return {}

    if name == 'ticket7.performance':
        # Count SQLite VM work rather than relying on machine-dependent wall time.
        real_connect = sqlite3.connect
        instructions = [0]
        def connect(*args, **kwargs):
            connection = real_connect(*args, **kwargs)
            def progress():
                instructions[0] += 100
                if instructions[0] > 2000000:
                    return 1
                return 0
            connection.set_progress_handler(progress, 100)
            return connection
        sqlite3.connect = connect
        try:
            records = [event(str(index), 1) for index in range(600)]
            replay(db, records, batch_size=50)
            first = instructions[0]
            replay(db, records, batch_size=50)
            require(Service(db, environ={}).get('alpha', 'p')['score'] == 600, 'optimized replay changed results or duplicate handling')
            require(instructions[0] <= 250000, 'replay exceeds the linear database work budget')
            return {'events': 600, 'sqlite_vm_instructions_sampled': instructions[0], 'first_pass_instructions': first}
        finally:
            sqlite3.connect = real_connect

    if name == 'ticket8.release':
        config.write_text(json.dumps({'greeting': 'Temporary', 'cache_size': 2}))
        service = Service(db, config, {'PROFILE_CACHE_SIZE': '0'})
        require(service.settings()['greeting'] == 'Temporary', 'initial file configuration was not read')
        config.unlink()
        require(service.settings()['greeting'] == 'Hello' and service.settings()['cache_size'] == 0,
                'removed configuration did not fall back to defaults and environment')
        config.write_text(json.dumps({'greeting': 'Restored', 'cache_enabled': False}))
        require(service.settings()['greeting'] == 'Restored', 'configuration recreation was not observed')
        configured = Service(db, config, {'PROFILE_GREETING': 'inherited', 'PROFILE_CACHE_SIZE': '6'},
                             cli_overrides={'greeting': 'explicit', 'cache_size': 0, 'cache_enabled': True})
        require(configured.settings()['greeting'] == 'explicit', 'explicit override was not applied')
        configured.set_cli_overrides({})
        inherited = configured.settings()
        require(inherited['greeting'] == 'inherited' and inherited['cache_size'] == 6
                and inherited['cache_enabled'] is False, 'removing an explicit override did not restore inherited settings')
        configured.set_cli_overrides({'greeting': 'again'})
        configured.set_cli_overrides(None)
        require(configured.settings()['greeting'] == 'inherited', 'clearing CLI overrides did not restore inheritance')
        delivery = workspace / 'docs/delivery.md'
        require(delivery.is_file() and bool(delivery.read_text().strip()), 'a nonempty authored docs/delivery.md is required for handoff')
        return {}

    if name == 'lifecycle.rollout':
        seed = event('rollout-seed', 5, profile_id='rollout', name='Rollout Customer')
        observations = {'starts': 0}
        for start in range(2):
            with server(workspace, db) as base:
                observations['starts'] += 1
                if start == 0:
                    status, acknowledgement = request(base, '/replay', data={'events': [seed]})
                    require(status == 200 and acknowledgement == {'received': 1, 'applied': 1},
                            'rollout HTTP seed write failed')
                    observations['seed_acknowledgement'] = acknowledgement
                status, v2 = request(base, '/v2/profiles/rollout')
                require(status == 200 and v2 == {'schema_version': 2, 'profile': {
                    'id': 'rollout', 'display_name': 'Rollout Customer', 'score': 5}},
                    'rollout v2 contract failed')
                observations[f'start_{start}'] = {'v2': v2}
                if canary:
                    from profile_service.client import LegacyClient
                    expected = {'id': 'rollout', 'name': 'Rollout Customer', 'score': 5}
                    status, v1 = request(base, '/v1/profiles/rollout')
                    require(status == 200 and v1 == expected, 'rollout v1 contract failed')
                    status, legacy = request(base, '/profiles/rollout')
                    require(status == 200 and legacy == expected
                            and LegacyClient(base, 'alpha-token').profile_name('rollout') == 'Rollout Customer',
                            'rollout legacy client contract failed')
                    observations[f'start_{start}'].update({'v1': v1, 'legacy': legacy})
        with sqlite3.connect(db) as connection:
            observations['acknowledgement_rows'] = connection.execute(
                'SELECT tenant, event_id FROM applied_events ORDER BY tenant, event_id').fetchall()
        require(observations['acknowledgement_rows'] == [('alpha', 'rollout-seed')],
                'rollout acknowledgement was not durable')
        return observations

    if name == 'lifecycle.recovery':
        require(previous is not None and (previous / 'src/profile_service').is_dir(),
                'previous release export is required for recovery')
        events = [event('release-seed', 5, profile_id='release', name='Release Customer'),
                  event('candidate-update', 7, profile_id='release', name='Release Customer'),
                  event('rollback-update', 3, profile_id='release', name='Release Customer'),
                  event('upgrade-update', 2, profile_id='release', name='Release Customer')]
        observations = {'stages': [], 'signal': 'SIGKILL'}

        def inspect(base, stage, score):
            expected = {'id': 'release', 'name': 'Release Customer', 'score': score}
            v1_status, v1 = request(base, '/v1/profiles/release')
            require(v1_status == 200 and v1 == expected, stage + ' lost v1 profile data')
            legacy_status, legacy = request(base, '/profiles/release')
            require(legacy_status == 200 and legacy == expected, stage + ' broke legacy API')
            v2_status, v2 = request(base, '/v2/profiles/release')
            require(v2_status == 200 and v2 == {'schema_version': 2, 'profile': {
                    'id': 'release', 'display_name': 'Release Customer', 'score': score}},
                    stage + ' broke v2 API')
            observations['stages'].append({'stage': stage, 'score': score,
                                           'v1_status': v1_status, 'legacy_status': legacy_status,
                                           'v2_status': v2_status, 'v1': v1, 'legacy': legacy, 'v2': v2})

        def apply(base, item, expected_applied):
            status, acknowledgement = request(base, '/replay', data={'events': [item]})
            require(status == 200 and acknowledgement == {'received': 1, 'applied': expected_applied},
                    'replay acknowledgement failed for ' + item['event_id'])
            observations.setdefault('replay_acknowledgements', []).append({
                'event_id': item['event_id'], **acknowledgement})

        with server(previous, db) as base:
            apply(base, events[0], 1)
            inspect(base, 'previous_seed', 5)
        stopped = []
        with server(workspace, db, stop_signal=signal.SIGKILL, stopped=stopped) as base:
            inspect(base, 'candidate_upgrade', 5)
            apply(base, events[0], 0)
            apply(base, events[1], 1)
            inspect(base, 'candidate_update', 12)
        require(stopped == [-signal.SIGKILL], 'candidate process was not killed with SIGKILL')
        observations['killed_returncode'] = stopped[0]
        with server(workspace, db) as base:
            inspect(base, 'candidate_restart', 12)
            apply(base, events[1], 0)
        with server(previous, db) as base:
            inspect(base, 'previous_rollback', 12)
            apply(base, events[2], 1)
            apply(base, events[2], 0)
            inspect(base, 'previous_update', 15)
        with server(workspace, db) as base:
            inspect(base, 'candidate_reupgrade', 15)
            apply(base, events[3], 1)
            for item in events:
                apply(base, item, 0)
            inspect(base, 'candidate_final', 17)
        with sqlite3.connect(db) as connection:
            observations['persisted_profile'] = connection.execute(
                'SELECT tenant, id, name, score, revision FROM profiles').fetchall()
            observations['acknowledgement_rows'] = connection.execute(
                'SELECT tenant, event_id FROM applied_events ORDER BY tenant, event_id').fetchall()
        require(observations['persisted_profile'] == [('alpha', 'release', 'Release Customer', 17, 4)],
                'release cycle lost or duplicated a profile effect')
        require(observations['acknowledgement_rows'] == [('alpha', item['event_id']) for item in sorted(
            events, key=lambda item: item['event_id'])], 'release cycle lost or duplicated an event acknowledgement')
        return observations

    if name == 'lifecycle.performance_repair':
        require(previous is not None and (previous / 'src/profile_service').is_dir(),
                'previous release export is required for performance comparison')
        selected = [event(f'performance-{index}', 1, profile_id='performance') for index in range(600)]
        events_path = scratch / 'performance-events.json'
        events_path.write_text(json.dumps(selected))
        instrument = r'''
import json, pathlib, runpy, sqlite3, sys
workspace, db, events = sys.argv[1:]
sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(workspace) / 'src'))
original_connect = sqlite3.connect
samples = [0]
def connect(*args, **kwargs):
    connection = original_connect(*args, **kwargs)
    def progress():
        samples[0] += 100
        return 0
    connection.set_progress_handler(progress, 100)
    return connection
sqlite3.connect = connect
sys.argv = ['profile_service', 'replay', '--db', db, '--events', events, '--batch-size', '50']
runpy.run_module('profile_service', run_name='__main__')
print(json.dumps({'sqlite_vm_instructions_sampled': samples[0]}))
'''
        observations = {}
        for label, export in (('published', previous), ('repaired', workspace)):
            database = scratch / f'{label}.sqlite'
            process = subprocess.run([sys.executable, '-I', '-c', instrument, str(export), str(database), str(events_path)],
                                     cwd=export, env=clean_env(export), capture_output=True, text=True, timeout=20)
            require(process.returncode == 0, label + ' replay CLI failed')
            lines = process.stdout.splitlines()
            require(len(lines) == 2, label + ' replay CLI did not return a measurement')
            acknowledgement, sample = map(json.loads, lines)
            require(acknowledgement == {'received': 600, 'applied': 600}, label + ' replay CLI lost events')
            require(isinstance(sample.get('sqlite_vm_instructions_sampled'), int)
                    and sample['sqlite_vm_instructions_sampled'] > 0, label + ' SQLite work was not measured')
            with sqlite3.connect(database) as connection:
                score = connection.execute('SELECT score FROM profiles WHERE tenant = ? AND id = ?',
                                           ('alpha', 'performance')).fetchone()[0]
                acknowledged = connection.execute('SELECT COUNT(*) FROM applied_events').fetchone()[0]
            require(score == 600 and acknowledged == 600, label + ' replay persistence is incomplete')
            observations[label] = {**sample, 'acknowledgement': acknowledgement,
                                   'profile_score': score, 'acknowledged_events': acknowledged}
        observations['published_to_repaired_work_ratio'] = (
            observations['published']['sqlite_vm_instructions_sampled'] /
            observations['repaired']['sqlite_vm_instructions_sampled'])
        return observations
    raise ValueError('unknown check')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--through', type=int, choices=range(9), default=0)
    parser.add_argument('--canary', action='store_true')
    parser.add_argument('--lifecycle', action='store_true')
    parser.add_argument('--previous-workspace', type=Path)
    parser.add_argument('--worker', help=argparse.SUPPRESS)
    parser.add_argument('--scratch', type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    workspace = args.workspace.resolve()
    previous = args.previous_workspace.resolve() if args.previous_workspace else None
    if args.worker:
        try:
            evidence = run_check(args.worker, workspace, args.scratch, previous, args.canary)
            print(json.dumps({'id': args.worker, 'passed': True, 'reason': 'behavior verified', 'evidence': evidence}))
        except Exception as error:
            print(json.dumps({'id': args.worker, 'passed': False, 'reason': str(error)[:300], 'error_type': type(error).__name__}))
        return 0
    if not workspace.is_dir() or not (workspace / 'src/profile_service').is_dir():
        print(json.dumps({'passed': False, 'checks': [], 'infrastructure_error': 'workspace is not a profile service export'}))
        return 2
    names = ['baseline', 'ticket1.config', 'ticket2.cache', 'ticket3.batch', 'ticket4.recovery',
             'ticket5.v2', 'ticket6.tenants', 'ticket7.performance', 'ticket8.release'][:args.through + 1]
    if args.canary and args.through >= 5:
        names.append('ticket5.legacy_canary')
    if args.lifecycle and args.through >= 5:
        names.append('lifecycle.rollout')
    if args.lifecycle and args.through >= 7:
        names.append('lifecycle.recovery')
        names.append('lifecycle.performance_repair')
    checks = []
    with tempfile.TemporaryDirectory(prefix='swe-trusted-probe-') as directory:
        for name in names:
            scratch = Path(directory) / name
            scratch.mkdir()
            command = [sys.executable, '-I', str(Path(__file__).resolve()), '--workspace', str(workspace), '--worker', name, '--scratch', str(scratch)]
            if previous:
                command += ['--previous-workspace', str(previous)]
            if args.canary:
                command.append('--canary')
            process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                stdout, stderr = process.communicate(timeout=25)
                result = json.loads(stdout)
                require(isinstance(result, dict) and isinstance(result.get('passed'), bool), 'invalid worker result')
                checks.append(result)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
                checks.append({'id': name, 'passed': False, 'reason': 'behavioral check exceeded its time limit'})
            except Exception:
                checks.append({'id': name, 'passed': False, 'reason': 'application prevented the behavioral check from completing'})
    print(json.dumps({'passed': all(check['passed'] for check in checks), 'checks': checks,
                      'through': args.through, 'canary': args.canary, 'lifecycle': args.lifecycle}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
