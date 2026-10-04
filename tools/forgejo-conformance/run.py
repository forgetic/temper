#!/usr/bin/env python3
"""Opt-in v15 API observation. Every run creates fresh loopback/SQLite state."""
import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import urllib.parse

BINARY = Path('/tmp/temper-forgejo-15/forgejo-15.0.0-linux-amd64')


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='temper-forgejo-version-') as version_scratch:
        version_env = dict(os.environ, GIT_CONFIG_GLOBAL=str(Path(version_scratch)/'gitconfig'), GIT_CONFIG_NOSYSTEM='1')
        version = subprocess.check_output([str(BINARY), '--version'], text=True, env=version_env).strip()
    if not version.lower().startswith('forgejo version 15.0.0'):
        raise RuntimeError('only the verified isolated v15 binary is authorized')
    digest = hashlib.sha256(BINARY.read_bytes()).hexdigest()
    password = secrets.token_urlsafe(32)
    hooks = []
    class Receiver(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
            self.send_response(204)
            self.end_headers()
            hooks.append({'headers': {k: v for k, v in self.headers.items() if k.lower() in ('x-forgejo-event', 'x-forgejo-signature', 'content-type')}, 'body': body.decode()})
        def log_message(self, *_):
            pass
    receiver = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Receiver)
    thread = threading.Thread(target=receiver.serve_forever, daemon=True)
    thread.start()
    records = []
    with tempfile.TemporaryDirectory(prefix='temper-forgejo-conformance-') as scratch:
        scratch = Path(scratch)
        isolated_env = dict(os.environ, GIT_CONFIG_GLOBAL=str(scratch/'gitconfig'), GIT_CONFIG_NOSYSTEM='1')
        listener = port()
        config = scratch / 'app.ini'
        config.write_text(f'''APP_NAME = temper isolated conformance
RUN_USER = {os.environ.get('USER','free')}
WORK_PATH = {scratch}
[server]
PROTOCOL = http
HTTP_ADDR = 127.0.0.1
HTTP_PORT = {listener}
ROOT_URL = http://127.0.0.1:{listener}/
SSH_DOMAIN = 127.0.0.1
APP_DATA_PATH = {scratch}/data
DISABLE_SSH = true
OFFLINE_MODE = true
[database]
DB_TYPE = sqlite3
PATH = {scratch}/forgejo.db
[repository]
ROOT = {scratch}/repositories
DEFAULT_BRANCH = main
[security]
INSTALL_LOCK = true
SECRET_KEY = {secrets.token_hex(32)}
INTERNAL_TOKEN = {secrets.token_hex(32)}
[service]
DISABLE_REGISTRATION = true
[webhook]
ALLOWED_HOST_LIST = 127.0.0.1
[log]
MODE = console
LEVEL = Error
''')
        base = [str(BINARY), '--work-path', str(scratch), '--config', str(config)]
        subprocess.run(base + ['migrate'], check=True, stdout=subprocess.DEVNULL, env=isolated_env)
        for name in ('fixture', 'reviewer'):
            made = subprocess.run(base + ['admin', 'user', 'create', '--username', name, '--password', password, '--email', name+'@example.invalid', '--admin', '--must-change-password=false'], stdout=subprocess.DEVNULL, env=isolated_env)
            if made.returncode:
                raise RuntimeError('isolated fixture user creation failed')
        log = (scratch / 'server.log').open('w')
        process = subprocess.Popen(base + ['web'], stdout=log, stderr=log, env=isolated_env)
        url = f'http://127.0.0.1:{listener}/api/v1'
        def call(name, method, route, body=None, token=None, basic=None, expect=None, capture=True):
            encoded = None if body is None else json.dumps(body, separators=(',',':')).encode()
            headers = {'Content-Type':'application/json'}
            if token:
                headers['Authorization'] = 'token ' + token
            if basic:
                headers['Authorization'] = 'Basic ' + base64.b64encode((basic+':'+password).encode()).decode()
            req = urllib.request.Request(url+route, data=encoded, method=method, headers=headers)
            try:
                result = urllib.request.urlopen(req, timeout=15)
            except urllib.error.HTTPError as error:
                result = error
            response = result.read().decode()
            if capture:
                records.append({'case':name, 'request':{'method':method,'target':'/api/v1'+route,'body':encoded.decode() if encoded else None},'response':{'status':result.status,'headers':{k:v for k,v in result.headers.items() if k.lower() in ('content-type','date','x-total-count')},'body':response}})
            if expect is not None and result.status not in expect:
                raise RuntimeError(f'{name}: expected {expect}, got {result.status}: {response}')
            return json.loads(response) if response.strip() else None
        try:
            for _ in range(200):
                if process.poll() is not None:
                    raise RuntimeError((scratch/'server.log').read_text())
                try:
                    urllib.request.urlopen(url+'/version', timeout=0.1).close()
                    break
                except (urllib.error.URLError, TimeoutError):
                    time.sleep(0.05)
            else:
                raise RuntimeError('isolated server did not start')
            tokens = {}
            for name in ('fixture','reviewer'):
                tokens[name] = call('create-token','POST',f'/users/{name}/tokens',{'name':'isolated','scopes':['all']},basic=name,expect=(201,),capture=False)['sha1']
            token = tokens['fixture']
            call('api-settings','GET','/settings/api',expect=(200,))
            user = call('current-user','GET','/user',token=token,expect=(200,))
            call('user-search-uid','GET',f'/users/search?uid={user["id"]}&page=1&limit=1',token=token,expect=(200,))
            repo = call('create-repository','POST','/user/repos',{'name':'specimen','auto_init':True,'default_branch':'main'},token=token,expect=(201,))
            route = '/repos/fixture/specimen'
            call('repository','GET',route,token=token,expect=(200,))
            call('create-hook','POST',route+'/hooks',{'type':'forgejo','active':True,'events':['issues','issue_comment','pull_request','pull_request_review','push','wiki'], 'config':{'url':f'http://127.0.0.1:{receiver.server_port}/hooks','content_type':'json','secret':'isolated-fixture-hook-secret'}},token=token,expect=(201,))
            label = call('define-label','POST',route+'/labels',{'name':'temper / label','color':'abcdef'},token=token,expect=(201,))
            call('labels','GET',route+'/labels?page=1&limit=2',token=token,expect=(200,))
            issue = call('create-issue-label-ids','POST',route+'/issues',{'title':'Fixture issue','body':'<!-- temper:key:fixture -->\nbody','labels':[label['id']]},token=token,expect=(201,))
            itemroute = route+f'/issues/{issue["number"]}'
            call('item','GET',itemroute,token=token,expect=(200,))
            call('items-leastupdate','GET',route+'/issues?sort=leastupdate&state=all&page=1&limit=2',token=token,expect=(200,))
            comment = call('post-comment','POST',itemroute+'/comments',{'body':'<!-- temper:nonce:1 -->\ncomment'},token=token,expect=(201,))
            commentroute = route+f'/issues/comments/{comment["id"]}'
            call('edit-comment','PATCH',commentroute,{'body':'edited'},token=token,expect=(200,))
            call('comment','GET',commentroute,token=token,expect=(200,))
            call('comments-unpaged-since','GET',itemroute+'/comments?since=1970-01-01T00%3A00%3A00Z',token=token,expect=(200,))
            call('add-label-names','POST',itemroute+'/labels',{'labels':[label['name']]},token=token,expect=(200,))
            call('remove-label-name','DELETE',itemroute+'/labels/'+urllib.parse.quote(label['name'],safe=''),token=token)
            call('remove-label','DELETE',itemroute+f'/labels/{label["id"]}',token=token)
            call('permission','GET',route+'/collaborators/fixture/permission',token=token,expect=(200,))
            branch = call('branch','GET',route+'/branches/main',token=token,expect=(200,))
            call('create-branch','POST',route+'/branches',{'new_branch_name':'feature/slash','old_ref_name':'main'},token=token,expect=(201,))
            commit = call('create-file','POST',route+'/contents/marker.txt',{'branch':'feature/slash','message':'fixture change','content':base64.b64encode(b'fixture\n').decode()},token=token,expect=(201,))
            pull = call('open-pull','POST',route+'/pulls',{'title':'Fixture pull','body':'pull body','head':'feature/slash','base':'main'},token=token,expect=(201,))
            pullroute = route+f'/pulls/{pull["number"]}'
            call('pull','GET',pullroute,token=token,expect=(200,))
            call('pull-for-direct-encoded-head','GET',route+'/pulls/main/feature%2Fslash',token=token)
            call('grant-reviewer','PUT',route+'/collaborators/reviewer',{'permission':'write'},token=token,expect=(204,))
            call('request-reviewer','POST',pullroute+'/requested_reviewers',{'reviewers':['reviewer']},token=token,expect=(201,))
            call('review','POST',pullroute+'/reviews',{'event':'APPROVED','body':'approved fixture'},token=tokens['reviewer'],expect=(200,))
            call('reviews','GET',pullroute+'/reviews?page=1&limit=2',token=token,expect=(200,))
            sha = pull['head']['sha']
            call('post-status','POST',route+'/statuses/'+sha,{'state':'success','context':'fixture/test','description':'green'},token=token,expect=(201,))
            call('combined-status','GET',route+'/commits/'+sha+'/status?page=1&limit=2',token=token,expect=(200,))
            call('merge-empty-response','POST',pullroute+'/merge',{'Do':'merge','head_commit_id':sha},token=token,expect=(200,))
            call('merged-pull','GET',pullroute,token=token,expect=(200,))
            call('create-wiki','POST',route+'/wiki/new',{'title':'Fixture page','content_base64':base64.b64encode(b'wiki body').decode(),'message':'fixture'},token=token,expect=(201,))
            call('wiki-pages','GET',route+'/wiki/pages?page=1&limit=2',token=token,expect=(200,))
            call('wiki-page','GET',route+'/wiki/page/Fixture%20page',token=token,expect=(200,))
            call('missing-item','GET',route+'/issues/999999',token=token,expect=(404,))
            call('stale-label','POST',itemroute+'/labels',{'labels':['not defined']},token=token)
            call('stale-label-id','POST',itemroute+'/labels',{'labels':[999999]},token=token)
            call('create-issue-stale-label-id','POST',route+'/issues',{'title':'Unknown ID','body':'probe','labels':[999999]},token=token)
            time.sleep(0.5)
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            receiver.shutdown()
        args.output.mkdir(parents=True, exist_ok=True)
        result = {'provenance':{'binary':str(BINARY),'version':version,'sha256':digest,'captured_at_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'isolation':'new temporary directory, SQLite, loopback only; destroyed after capture','actions':'not run; source-derived metadata fixtures only'},'http':records,'webhooks':hooks}
        (args.output/'observations.json').write_text(json.dumps(result,indent=2)+'\n')
        print(f'captured {len(records)} HTTP exchanges and {len(hooks)} webhook deliveries in {args.output}')

if __name__ == '__main__':
    main()
