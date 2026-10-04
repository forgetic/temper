"""Step 00a observations, exclusively inside run.py's disposable instance."""
import base64
import json
from pathlib import Path
import subprocess
import time
import urllib.request

RUNNER = Path('/home/free/.local/bin/forgejo-runner')


def observe(call, route, admin, writer, scratch, env, api):
    facts = {}

    def get(name, path, **kw):
        return call(name, 'GET', route + path, token=writer, **kw)

    def branch(name, ref='main'):
        return call('00a-create-' + name, 'POST', route + '/branches',
                    {'new_branch_name': name, 'old_ref_name': ref}, token=writer, expect=(201,))

    def put(branch_name, path, content, previous=None):
        body = {'branch': branch_name, 'message': '00a ' + path,
                'content': base64.b64encode(content.encode()).decode()}
        if previous:
            body['sha'] = previous
        return call('00a-file-' + branch_name + '-' + path, 'PUT' if previous else 'POST',
                    route + '/contents/' + path, body, token=writer, expect=(200, 201))

    def pull(name, target='main'):
        return call('00a-pull-' + name, 'POST', route + '/pulls',
                    {'title': '00a ' + name, 'head': name, 'base': target}, token=writer, expect=(201,))

    def ready(number):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            p = get('poll', f'/pulls/{number}', capture=False, expect=(200,))
            if p.get('mergeable'):
                return p
            time.sleep(.1)
        raise RuntimeError('00a mergeable pull did not settle')

    # The same all-scope token is attached to a non-administrator with write access.
    get('00a-writer-permission', '/collaborators/reviewer/permission', expect=(200,))
    call('00a-writer-user', 'GET', '/user', token=writer, expect=(200,))
    main = get('00a-main', '/branches/main', expect=(200,))['commit']['id']
    made = branch('commit-origin', main)
    facts['branch_at_commit'] = made['commit']['id'] == main
    if not facts['branch_at_commit']:
        raise RuntimeError('branch creation did not use the requested commit')
    call('00a-protect-main', 'POST', route + '/branch_protections',
         {'branch_name': 'main', 'enable_push': True, 'enable_merge_whitelist': False},
         token=admin, expect=(201,))
    get('00a-protection-list-as-writer', '/branch_protections', raw=True, expect=(403,))
    get('00a-protection-as-writer', '/branch_protections/main', raw=True, expect=(403,))

    # Enough files and commits to force actual continuation, rather than one page.
    branch('paged')
    for i in range(3):
        put('paged', f'paged-{i}.txt', f'file {i}\n')
    paged = pull('paged')
    head = paged['head']['sha']
    for page in range(1, 5):
        files = get(f'00a-pull-files-page-{page}', f'/pulls/{paged["number"]}/files?page={page}&limit=1', expect=(200,))
        comparison = get(f'00a-comparison-page-{page}', f'/compare/{main}...{head}?page={page}&limit=1', expect=(200,))
        if len(files) != (1 if page < 4 else 0):
            raise RuntimeError('pull file pagination differs from the recorded observation')
        if len(comparison['files']) != 3 or len(comparison['commits']) != 3:
            raise RuntimeError('comparison pagination differs from the recorded observation')

    # A base and head independently edit the same existing file.
    original = put('main', 'conflict.txt', 'original\n')['content']['sha']
    branch('conflict')
    put('conflict', 'conflict.txt', 'head\n', original)
    conflict = pull('conflict')
    put('main', 'conflict.txt', 'base\n', original)
    ready(paged['number'])
    call('00a-conflicting-update', 'POST', route + f'/pulls/{conflict["number"]}/update?style=merge',
         token=writer, raw=True, expect=(409,))
    refused = get('00a-conflict-head-after', f'/pulls/{conflict["number"]}', expect=(200,))
    if refused['head']['sha'] != conflict['head']['sha']:
        raise RuntimeError('conflicting update moved the head')

    # No downloaded actions or checkout: the failed job is wholly our inline shell.
    workflow = '''name: conformance
on:
  push:
    branches: [clean]
jobs:
  fail:
    runs-on: temper-host
    steps:
      - run: |
          printf 'TEMPER_00A_FAILED_JOB_OUTPUT\\n'
          exit 1
'''
    call('00a-enable-actions', 'PATCH', route, {'has_actions': True}, token=admin, expect=(200,))
    registration = call('registration', 'GET', route + '/actions/runners/registration-token',
                        token=admin, expect=(200,), capture=False)['token']
    runner_dir = scratch / 'runner'
    runner_dir.mkdir()
    config = runner_dir / 'config.yaml'
    config.write_text(f'''log:
  level: info
runner:
  file: {runner_dir}/registration.json
  capacity: 1
  timeout: 1m
  shutdown_timeout: 1s
  fetch_interval: 100ms
  report_interval: 100ms
  labels: [temper-host:host]
cache:
  enabled: false
host:
  workdir_parent: {runner_dir}/jobs
''')
    runner_env = {key: env[key] for key in ('PATH', 'USER', 'LOGNAME', 'LANG', 'GIT_CONFIG_GLOBAL', 'GIT_CONFIG_NOSYSTEM') if key in env}
    version = subprocess.check_output([str(RUNNER), '--version'], text=True, env=runner_env).strip()
    runner_base = [str(RUNNER), '--config', str(config)]
    registered = subprocess.run(runner_base + ['register', '--no-interactive', '--instance', api.removesuffix('/api/v1'),
                                '--token', registration, '--name', 'temper-00a', '--labels', 'temper-host:host'],
                               cwd=runner_dir, env=runner_env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if registered.returncode:
        raise RuntimeError('isolated runner registration failed')
    with (runner_dir / 'runner.log').open('w') as log:
        runner = subprocess.Popen(runner_base + ['daemon'], cwd=runner_dir, env=runner_env, stdout=log, stderr=log)
        try:
            put('main', '.forgejo/workflows/conformance.yaml', workflow)
            branch('clean')
            put('clean', 'clean.txt', 'head\n')
            clean = pull('clean')
            put('main', 'base-only.txt', 'base\n')
            before = ready(clean['number'])['head']['sha']
            call('00a-clean-update', 'POST', route + f'/pulls/{clean["number"]}/update?style=merge',
                 token=writer, expect=(200,))
            updated = get('00a-clean-pull-after', f'/pulls/{clean["number"]}', expect=(200,))
            after = updated['head']['sha']
            facts['update_moves_head'] = after != before
            if not facts['update_moves_head']:
                raise RuntimeError('clean update did not move the head')
            facts['runner_version'] = version
            deadline = time.monotonic() + 75
            found = None
            while time.monotonic() < deadline:
                runs = get('poll', '/actions/runs?page=1&limit=100', expect=(200,), capture=False)
                rows = runs.get('workflow_runs', runs.get('runs', [])) if isinstance(runs, dict) else runs
                found = next((r for r in rows if r.get('commit_sha', r.get('head_sha')) == after and
                              r.get('status') == 'failure'), None)
                if found:
                    break
                if runner.poll() is not None:
                    raise RuntimeError('isolated runner exited')
                time.sleep(.2)
            if not found:
                raise RuntimeError('updated-head Actions failure did not settle within 75 seconds')
            facts['updated_head_failed_run'] = {key: found[key] for key in ('id', 'commit_sha', 'event', 'status')}
            facts['runner_stdout_marker'] = '  | TEMPER_00A_FAILED_JOB_OUTPUT' in (runner_dir / 'runner.log').read_text()
            if not facts['runner_stdout_marker']:
                raise RuntimeError('runner did not record the intentional failure output')
            get('00a-runs-after-update', '/actions/runs?page=1&limit=100', expect=(200,))
            get('00a-tasks-after-update', '/actions/tasks?page=1&limit=100', expect=(200,))
            get('00a-updated-head-status', f'/commits/{after}/status?page=1&limit=100', expect=(200,))
            # Read the actual binary's specification; API logs are probed only if documented.
            with urllib.request.urlopen(api.removesuffix('/api/v1') + '/swagger.v1.json') as response:
                spec = json.load(response)
            facts['actions_api_paths'] = [p for p in spec['paths'] if '/actions/' in p]
            if found:
                run_id = found['id']
                get('00a-run-at-updated-head', f'/actions/runs/{run_id}', expect=(200,))
                for suffix in (f'/actions/runs/{run_id}/jobs', f'/actions/runs/{run_id}/logs'):
                    get('00a-probe-' + suffix.rsplit('/', 1)[-1], suffix, raw=True, expect=(404,))
            # Conditional landing refuses a stale head, then the same current head lands at its retargeted base.
            landing_base = branch('retarget-base')['commit']['id']
            branch('retarget-head')
            put('retarget-head', 'retarget.txt', 'head\n')
            retarget = pull('retarget-head')
            old_head = retarget['head']['sha']
            put('retarget-head', 'retarget-new.txt', 'new head\n')
            current = ready(retarget['number'])
            call('00a-merge-stale-head', 'POST', route + f'/pulls/{retarget["number"]}/merge',
                 {'Do': 'merge', 'head_commit_id': old_head}, token=writer, raw=True, expect=(409,))
            call('00a-retarget-pull', 'PATCH', route + f'/pulls/{retarget["number"]}',
                 {'base': 'retarget-base'}, token=writer, expect=(201, 200))
            current = ready(retarget['number'])
            call('00a-merge-retargeted-current-head', 'POST', route + f'/pulls/{retarget["number"]}/merge',
                 {'Do': 'merge', 'head_commit_id': current['head']['sha']}, token=writer, expect=(200,))
            landed = get('00a-retargeted-merged-pull', f'/pulls/{retarget["number"]}', expect=(200,))
            target_tip = get('00a-retarget-base-after', '/branches/retarget-base', expect=(200,))['commit']['id']
            main_tip = get('00a-main-after-retarget', '/branches/main', expect=(200,))['commit']['id']
            if not landed['merged'] or landed['base']['ref'] != 'retarget-base' or target_tip == landing_base or main_tip != landing_base:
                raise RuntimeError('retargeted merge did not advance only the selected base')
        finally:
            runner.terminate()
            try:
                runner.wait(timeout=5)
            except subprocess.TimeoutExpired:
                runner.kill()
                runner.wait()
    return facts
