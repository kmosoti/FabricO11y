"""Finite signed virtual-credential probes; no product-state edits or private receipts."""
import base64
import copy
import hashlib
import json
import os
import secrets
import subprocess
import time

PROPERTIES = ('registration-replay', 'login-replay', 'wrong-origin', 'wrong-rp', 'missing-uv',
              'attacker-cookie', 'previous-cookie', 'reader-expiry')


def require(value, message):
    if not value:
        raise ValueError(message)


def decode(value):
    return base64.b64decode(value.replace('-', '+').replace('_', '/') + '=' * (-len(value) % 4), validate=True)


def encode(value):
    return base64.urlsafe_b64encode(value).decode().rstrip('=')


def grade(rows):
    require(len(rows) == len(PROPERTIES) and {row['property'] for row in rows} == set(PROPERTIES),
            'security property inventory missing or duplicated')
    for row in rows:
        name = row['property']
        expected = (401, 'sign in required') if name.endswith('cookie') or name == 'reader-expiry' else (403, 'access denied')
        require((row['status'], row['error']) == expected and row['positive_status'] == 200,
                'security property rejected: ' + name)
        if name in ('wrong-origin', 'wrong-rp', 'missing-uv'):
            require(row.get('signature_verified') is True and row.get('positive_signature_verified') is True,
                    'security signature witness missing: ' + name)
        if name.endswith('cookie'):
            require(row.get('old_sha256') != row.get('new_sha256')
                    and all(isinstance(row.get(k), str) and len(row[k]) == 64 for k in ('old_sha256', 'new_sha256')),
                    'security cookie replacement missing: ' + name)
        if name == 'reader-expiry':
            require(row.get('before_status') == row.get('long_reader_status') == 200
                    and row['wait_ns'] >= 4_000_000_000 and row['wall_after_s'] > row['expires_unix_s']
                    and abs(row['wall_elapsed_ns'] - row['wait_ns']) <= 100_000_000,
                    'security reader real expiry witness missing')
    return True


def run(*, api, bearer, js, async_js, wd, export_credentials, origin, principal, work, deadline, registration, inject=None, observations=None):
    rows = observations if observations is not None else []
    def response(row, outcome, positive=200):
        status, body = outcome
        rows.append(dict(property=row, status=status, error=body.get('error') if isinstance(body, dict) else None,
                         positive_status=positive))
        return rows[-1]
    def check_time():
        require(time.monotonic() < deadline, 'security probe deadline exhausted')
    def cookie():
        values = wd('/cookie', method='GET')
        return next(value for value in values if value['name'] == '__Host-fabric-session')
    def credential(start):
        value = async_js('const done=arguments[arguments.length-1];window.fabricPasskey(false,arguments[0]).then(x=>done({value:x})).catch(e=>done({error:String(e)}))', json.dumps(start['public_key']))
        require('value' in value, 'actual virtual assertion unavailable')
        return json.loads(value['value'])
    def start():
        check_time()
        status, value = bearer(None, '/v1/console/auth/login/start', 'POST', {'principal_id': principal})
        require(status == 200, 'fresh login challenge unavailable')
        return value
    def finish(challenge, assertion):
        return api('/v1/console/auth/login/finish', 'POST', {'ceremony_id': challenge['ceremony_id'], 'credential': assertion})
    require(registration and set(registration) == {'ceremony_id', 'credential'}, 'completed registration capture missing')
    require(api('/v1/console/session')[0] == 200, 'registration positive session unavailable')
    response('registration-replay', bearer(None, '/v1/console/auth/register/finish', 'POST', registration))
    challenge = start()
    original = credential(challenge)
    outcome = finish(challenge, original)
    require(outcome[0] == 200, 'real browser assertion positive failed')
    response('login-replay', finish(challenge, original), outcome[0])
    auth = decode(original['response']['authenticatorData'])
    client = json.loads(decode(original['response']['clientDataJSON']))
    require(client.get('type') == 'webauthn.get' and client.get('origin') == origin, 'actual assertion client binding differs')
    require(len(auth) == 37 and auth[32] & 5 == 5 and auth[:32] == hashlib.sha256(b'localhost').digest(),
            'fixture assertion shape/RP/UP/UV differs')
    exported = export_credentials()
    keys = [key for key in exported if decode(key['credentialId']) == decode(original['id'])]
    require(len(keys) == 1 and keys[0].get('privateKey'), 'matching virtual fixture private key unavailable')
    protected = work / 'security-private'
    protected.mkdir(mode=0o700)
    private = protected / 'key.der'
    fd = os.open(private, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as output:
        output.write(decode(keys[0]['privateKey']))
    exported = keys = None
    public = protected / 'public.pem'
    def openssl(argv, data=None):
        check_time()
        process = subprocess.run(['openssl', *argv], input=data, capture_output=True,
                                 timeout=max(.1, min(10, deadline-time.monotonic())))
        require(process.returncode == 0, 'independent signature tool failed')
        return process.stdout
    public.write_bytes(openssl(['pkey', '-inform', 'DER', '-in', str(private), '-pubout']))
    counter = int.from_bytes(auth[33:37], 'big')
    def signed(challenge, change=None):
        nonlocal counter
        counter += 1
        require(counter < 2**32, 'fixture signature counter exhausted')
        client_data = dict(client, challenge=challenge['public_key']['publicKey']['challenge'])
        if change == 'wrong-origin':
            client_data['origin'] = 'https://untrusted.invalid'
        raw_client = json.dumps(client_data, separators=(',', ':')).encode()
        raw_auth = (hashlib.sha256(b'untrusted.invalid').digest() if change == 'wrong-rp' else auth[:32])
        raw_auth += bytes([auth[32] & ~4 if change == 'missing-uv' else auth[32]]) + counter.to_bytes(4, 'big')
        message = raw_auth + hashlib.sha256(raw_client).digest()
        signature = openssl(['dgst', '-sha256', '-keyform', 'DER', '-sign', str(private)], message)
        signature_path = protected / 'signature.der'
        signature_path.write_bytes(signature)
        openssl(['dgst', '-sha256', '-verify', str(public), '-signature', str(signature_path)], message)
        answer = copy.deepcopy(original)
        answer['response'].update(authenticatorData=encode(raw_auth), clientDataJSON=encode(raw_client), signature=encode(signature))
        return answer
    for name in ('wrong-origin', 'wrong-rp', 'missing-uv'):
        challenge = start()
        negative = finish(challenge, signed(challenge, name))
        challenge = start()
        positive = finish(challenge, signed(challenge))
        row = response(name, negative, positive[0])
        row.update(signature_verified=True, positive_signature_verified=True)
    for name in ('attacker-cookie', 'previous-cookie'):
        old = cookie()
        if name == 'attacker-cookie':
            old = dict(old, value=secrets.token_hex(32))
            wd('/cookie', {'cookie': {k: old[k] for k in ('name', 'value', 'path', 'secure', 'httpOnly', 'sameSite')}})
        challenge = start()
        positive = finish(challenge, signed(challenge))
        new = cookie()
        denied = bearer(None, '/v1/console/session', cookie=old['name']+'='+old['value'])
        require(api('/v1/console/session')[0] == 200, 'replacement owner session unavailable')
        row = response(name, denied, positive[0])
        row.update(old_sha256=hashlib.sha256(old['value'].encode()).hexdigest(),
                   new_sha256=hashlib.sha256(new['value'].encode()).hexdigest())
    # CSRF changed with each successful login; use actual current session data.
    current = api('/v1/console/session')[1]
    owner = cookie()
    def issue(name, ttl):
        code, value = bearer(None, '/v1/console/readers', 'POST', {'name': name, 'ttl_s': ttl},
                            cookie=owner['name']+'='+owner['value'], csrf=current['csrf'])
        require(code == 200, 'actual short/long reader issuance failed')
        return value
    long_reader = issue('security-long', 60)
    short_reader = issue('security-short', 3)
    before_ns, wall_before_ns = time.monotonic_ns(), time.time_ns()
    short_before = bearer(short_reader['token'], '/v1/console/nodes')[0]
    while time.monotonic_ns() - before_ns < 4_000_000_000 or time.time() <= short_reader['expires_unix_s']:
        check_time()
        time.sleep(.05)
    after_ns, wall_after_ns = time.monotonic_ns(), time.time_ns()
    denied = bearer(short_reader['token'], '/v1/console/nodes')
    row = response('reader-expiry', denied, api('/v1/console/session')[0])
    row.update(before_status=short_before, long_reader_status=bearer(long_reader['token'], '/v1/console/nodes')[0],
               wait_ns=after_ns-before_ns, wall_elapsed_ns=wall_after_ns-wall_before_ns,
               wall_after_s=wall_after_ns/1e9, expires_unix_s=short_reader['expires_unix_s'])
    for path in protected.iterdir():
        path.unlink()
    protected.rmdir()
    if inject:
        require(inject in PROPERTIES, 'unregistered security defect')
        target = next(row for row in rows if row['property'] == inject)
        target['injection_original'] = {'status': target['status'], 'error': target['error']}
        target['status'] = 200
    return rows
