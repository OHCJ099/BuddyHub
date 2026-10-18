"""Exercise public gateway isolation and streaming against a local fake upstream."""
import hashlib, json, os, socket, subprocess, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import httpx

EXE = Path(os.environ.get('BUDDYHUB_SHARE_EXE', str(Path(__file__).resolve().parents[1] / 'dist' / 'buddyhub-share.exe')))
class Upstream(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_POST(self):
        assert self.path == '/v1/chat/completions'
        assert self.headers['Authorization'] == 'Bearer private-test-key'
        assert self.headers.get('Cookie') is None
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert body['reasoning_effort'] == 'high'
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        self.wfile.write(b'data: {"choices":[{"delta":{"content":"OK"}}]}\n\n'); self.wfile.flush()
        time.sleep(1.5)
        self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush()

with tempfile.TemporaryDirectory(prefix='buddyhub-sharing-test-') as folder:
    p=Path(folder)
    server=ThreadingHTTPServer(('127.0.0.1',0),Upstream)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    key='test-public-key'
    settings={'enabled':True,'keys':[{'id':'test-user','sha256':hashlib.sha256(key.encode()).hexdigest(),'enabled':True,'dailyRequests':2,'concurrency':1,'requestsPerMinute':10}]}
    (p/'sharing.json').write_text(json.dumps(settings))
    (p/'config.json').write_text(json.dumps({'port':server.server_port,'localKey':'private-test-key'}))
    with socket.socket() as reserve:
        reserve.bind(('127.0.0.1',0));port=reserve.getsockname()[1]
    args=[str(EXE),str(p),f'127.0.0.1:{port}']
    proc=subprocess.Popen(args,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
    try:
        with httpx.Client(base_url=f'http://127.0.0.1:{port}',timeout=5,trust_env=False) as c:
            for _ in range(50):
                try: c.get('/v1/models'); break
                except httpx.ConnectError: time.sleep(.1)
            headers={'Authorization':f'Bearer {key}','Cookie':'do-not-forward'}
            assert c.get('/v1/models').status_code==401
            for path in ['/health','/admin/status','/admin/client-config','/']:
                assert c.get(path,headers=headers).status_code==404,path
            assert len(c.get('/v1/models',headers=headers).json()['data'])==3
            payload={'model':'deepseek-v4.1-flash-auto','messages':[{'role':'user','content':'test'}],'stream':True,'reasoning_effort':'high'}
            start=time.monotonic()
            with c.stream('POST','/v1/chat/completions',headers=headers,json=payload) as response:
                assert response.status_code==200
                chunks=response.iter_lines()
                assert next(chunks).startswith('data: ')
                assert time.monotonic()-start<1.3,'SSE output was buffered'
                assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==429
                assert '[DONE]' in '\n'.join(chunks)
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==200
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==429
            policy=settings['keys'][0]
            def save(): (p/'sharing.json').write_text(json.dumps(settings))
            policy.update(dailyRequests=0,monthlyRequests=3,requestsPerMinute=0);save()
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==200
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==429
            policy.update(monthlyRequests=0,totalRequests=4);save()
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==200
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==429
            policy.update(totalRequests=0,models=['deepseek-v4.1-flash-cn']);save()
            assert len(c.get('/v1/models',headers=headers).json()['data'])==1
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==403
            settings['maxBodyMb']=1;save()
            assert c.post('/v1/chat/completions',headers=headers,content=b'x'*(1024*1024+1)).status_code==413
            policy['expiresAt']=1;save()
            assert c.get('/v1/models',headers=headers).status_code==403
            policy['expiresAt']=0
            policy['sha256']=hashlib.sha256(b'new-public-key').hexdigest();policy['dailyRequests']=7;save()
            assert c.get('/v1/models',headers=headers).status_code==401
            headers={'Authorization':'Bearer new-public-key'}
            payload['model']='deepseek-v4.1-flash-cn'
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==200
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==429
            proc.terminate();proc.wait(timeout=10)
            proc=subprocess.Popen(args,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
            for _ in range(50):
                try: c.get('/v1/models');break
                except httpx.ConnectError:time.sleep(.1)
            assert c.post('/v1/chat/completions',headers=headers,json=payload).status_code==429
            settings['keys'][0]['enabled']=False
            (p/'sharing.json').write_text(json.dumps(settings))
            assert c.get('/v1/models',headers=headers).status_code==401
            settings['enabled']=False
            (p/'sharing.json').write_text(json.dumps(settings))
            assert c.get('/v1/models',headers=headers).status_code==503
        print('PASS: auth, admin isolation, SSE, high effort forwarding, concurrency, daily/monthly/lifetime limits, model permissions, expiry, body limit, rotation/restart preserving usage, revocation, disable')
    finally:
        proc.terminate(); proc.wait(timeout=10); server.shutdown()
