// Synthetic browser smoke; this server does not implement product authentication.
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const assert = require('node:assert/strict');
const { setTimeout: delay } = require('node:timers/promises');
const dist = path.resolve(process.argv[2]);
const fixtures = __dirname;
const profile = process.env.UI_PROFILE;
const driver = process.env.CHROMEDRIVER;
const chrome = process.env.CHROME;
if (!profile || !driver || !chrome) throw Error('UI_PROFILE, CHROMEDRIVER and CHROME required');
let mode = 'normal';
const requests = [];
const maxRequests = 8;
let heldResponse = null;
let heldClosed = false;
const server = http.createServer((req, res) => {
  if (req.method === 'POST' && req.url === '/api/v1/query') {
    let chunks = [], length = 0;
    req.on('data', c => { length += c.length; if (length > 262144) req.destroy(); else chunks.push(c); });
    req.on('end', () => {
      try {
        const q = JSON.parse(Buffer.concat(chunks));
        if (requests.length >= maxRequests) { res.writeHead(429); return res.end(); }
        requests.push(q);
        if (mode === 'denied') { res.writeHead(403); return res.end(); }
        if (mode === 'held') {
          heldResponse = res;
          res.on('close', () => { heldClosed = true; });
          return;
        }
        const file = q.operation.operation === 'list_runs' ? 'first.json' : q.operation.operation === 'run_summary' ? 'detail.json' : null;
        if (!file) { res.writeHead(400); return res.end(); }
        res.writeHead(200, {'Content-Type':'application/json','Cache-Control':'no-store'}); res.end(fs.readFileSync(path.join(fixtures,file)));
      } catch { res.writeHead(400); res.end(); }
    });
    return;
  }
  const name = req.url === '/' ? 'index.html' : req.url.slice(1);
  if (req.method !== 'GET' || !/^[a-zA-Z0-9._-]+$/.test(name)) { res.writeHead(404); return res.end(); }
  const ext = path.extname(name), types = {'.html':'text/html','.js':'text/javascript','.wasm':'application/wasm','.css':'text/css'};
  if (!types[ext]) { res.writeHead(404); return res.end(); }
  fs.readFile(path.join(dist,name), (err, bytes) => { if (err) { res.writeHead(404); return res.end(); } res.writeHead(200, {'Content-Type':types[ext]}); res.end(bytes); });
});
async function listen(srv) {
  await Promise.race([
    new Promise((resolve, reject) => {
      srv.once('error', reject);
      srv.listen(0, '127.0.0.1', resolve);
    }),
    delay(5000).then(() => { throw Error('loopback bind timed out'); }),
  ]);
  return srv.address().port;
}
async function call(base, method, route, data, timeoutMs=8000) {
  const r = await fetch(base+route, {method, headers:{'Content-Type':'application/json'}, body:data === undefined ? undefined : JSON.stringify(data), signal:AbortSignal.timeout(timeoutMs)});
  const obj = await r.json(); if (!r.ok) throw Error(`webdriver ${method} ${route}: ${JSON.stringify(obj.value)}`); return obj.value;
}
async function wait(fn, ms=10000) { const end=Date.now()+ms; let last; while(Date.now()<end) { try { last=await fn(); if(last) return last; } catch(e) { last=e.message; } await delay(100); } throw Error(`timeout: ${last}`); }
(async () => {
  let proc, session, wd;
  try {
    const port = await listen(server);
    const probe = http.createServer();
    const dp = await listen(probe);
    await new Promise((resolve, reject) => probe.close(error => error ? reject(error) : resolve()));
    wd = `http://127.0.0.1:${dp}`;
    proc = spawn(driver, [`--port=${dp}`, '--allowed-ips=127.0.0.1'], {stdio:'inherit'});
    let spawnError;
    proc.on('error', error => { spawnError = error; });
    await wait(async () => {
      if (spawnError) throw spawnError;
      if (proc.exitCode !== null) throw Error(`chromedriver exited ${proc.exitCode}`);
      return (await call(wd, 'GET', '/status')).ready;
    });
    // A cold hosted runner can take longer to start Chrome than ordinary commands.
    const sessionStarted = Date.now();
    try {
      session=(await call(wd,'POST','/session',{capabilities:{alwaysMatch:{browserName:'chrome','goog:chromeOptions':{binary:chrome,args:['--headless=new','--disable-dev-shm-usage','--no-first-run','--disable-background-networking','--disable-component-update','--disable-sync','--no-proxy-server','--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1',`--user-data-dir=${profile}`]}}}},30000)).sessionId;
      console.log(`webdriver POST /session completed in ${Date.now() - sessionStarted} ms`);
    } catch (error) {
      throw new Error(`webdriver POST /session failed after ${Date.now() - sessionStarted} ms: ${error.message}`, {cause:error});
    }
    const base=`/session/${session}`;
    const cmd=(method, route, data)=>call(wd,method,base+route,data);
    const js=(script,...args)=>cmd('POST','/execute/sync',{script,args});
    const find=async selector=>(await cmd('POST','/element',{using:'css selector',value:selector}))['element-6066-11e4-a52e-4f735466cecf'];
    const ref=id=>({'element-6066-11e4-a52e-4f735466cecf':id});
    const keys=(id,text)=>cmd('POST',`/element/${id}/value`,{text});
    const status=()=>js("return document.querySelector('[role=status]')?.textContent");
    await cmd('POST','/timeouts',{implicit:0,pageLoad:15000,script:5000});
    await cmd('POST','/window/rect',{width:1200,height:850});
    await cmd('POST','/url',{url:`http://127.0.0.1:${port}/`});
    await wait(()=>js("return document.querySelectorAll('form input').length === 2"));
    const labels=await js("return [...document.querySelectorAll('form input')].map(x=>[...x.labels].map(l=>l.innerText).join(' '))");
    assert.match(labels[0],/Authority/); assert.match(labels[1],/Sources/);
    await keys(await find('form label:nth-of-type(1) input'),'auth-1'); await keys(await find('form label:nth-of-type(2) input'),'alpha');
    let submit=await find('button[type=submit]'); await js('arguments[0].focus()',ref(submit)); await keys(submit,'\uE007');
    await wait(async()=>await status()==='Results ready');
    assert.equal(requests.at(-1).operation.operation,'list_runs'); assert.deepEqual(requests.at(-1).scope,['alpha']); assert.equal(requests.at(-1).authority,'auth-1');
    const run='00000000-0000-4000-8000-000000000001'; let button=await find(`button[aria-label="Inspect summary for ${run}"]`);
    await js('arguments[0].focus()',ref(button)); await keys(button,'\uE007');
    await wait(async()=>requests.at(-1)?.operation?.operation==='run_summary' && await status()==='Results ready');
    assert.deepEqual(requests.at(-1).snapshot,{alpha:'3'}); assert.equal(requests.at(-1).filter.run.run_id,run); assert.equal(requests.at(-1).cursor,null);
    await wait(()=>js("return document.activeElement.id==='results-heading'"));
    const pairs=await js("return Object.fromEntries([...document.querySelectorAll('main section dl')].flatMap(d=>[...d.querySelectorAll('dt')].map(x=>[x.textContent,x.nextElementSibling.textContent])))");
    assert.equal(pairs.Observations,'1'); assert.equal(pairs['Minimum (ns)'],'5'); assert.equal(pairs['Exact mean (ns)'],'5 / 1');
    const back=await find('main > button'); await keys(back,'\uE007'); await wait(async()=>requests.at(-1)?.operation?.operation==='list_runs' && await status()==='Results ready');
    assert.deepEqual(requests.at(-1).snapshot,{alpha:'3'}); await wait(()=>js("return document.activeElement.id==='results-heading'"));
    // Window dimensions do not guarantee content viewport width in headless Chrome.
    await cmd('POST', '/goog/cdp/execute', {
      cmd: 'Emulation.setDeviceMetricsOverride',
      params: {width:320, height:800, deviceScaleFactor:1, mobile:true},
    });
    const widths = await js('return [document.documentElement.scrollWidth, document.documentElement.clientWidth]');
    assert.equal(widths[1], 320, `unexpected viewport ${widths}`);
    assert.ok(widths[0] <= widths[1] + 1, `horizontal overflow ${widths}`);
    assert.ok(await find(`button[aria-label="Inspect summary for ${run}"]`));
    mode = 'held';
    submit = await find('button[type=submit]');
    await keys(submit, '\uE007');
    await wait(() => heldResponse && status());
    const authorityInput = await find('form label:nth-of-type(1) input');
    await js('arguments[0].focus()', ref(authorityInput));
    await keys(authorityInput, '\uE004'); // Tab to source scope.
    const sourceInput = await find('form label:nth-of-type(2) input');
    assert.equal(await js('return document.activeElement === arguments[0]', ref(sourceInput)), true);
    await keys(sourceInput, '\uE004'); // Disabled submit is skipped while loading.
    assert.equal(await js('return document.activeElement.textContent'), 'Stop waiting');
    const stopButton = await js('return document.activeElement');
    await keys(stopButton['element-6066-11e4-a52e-4f735466cecf'], '\uE007');
    await wait(async () => (await status()).includes('Backend work may still continue'));
    await wait(() => heldClosed);
    await wait(() => js("return document.activeElement.id === 'results-heading'"));
    mode = 'normal';
    const heading = await find('#results-heading');
    await keys(heading, '\uE008\uE004\uE000'); // Shift+Tab to retry.
    assert.equal(await js('return document.activeElement.textContent'), 'Retry read');
    const retryButton = await js('return document.activeElement');
    await keys(retryButton['element-6066-11e4-a52e-4f735466cecf'], '\uE007');
    await wait(() => js("return document.activeElement.id === 'results-heading'"));
    await wait(async () => (await status()) === 'Results ready');
    assert.equal(requests.at(-1).operation.operation, 'list_runs');
    assert.equal(requests.at(-1).cursor, null);
    // The server observed the aborted connection. Stale reducer responses are
    // exercised by the shared native/Wasm scenario, not this closed socket.
    assert.ok(await find(`button[aria-label="Inspect summary for ${run}"]`));
    mode = 'denied';
    submit = await find('button[type=submit]');
    await keys(submit, '\uE007');
    await wait(async () => (await status()).includes('Access denied'));
    assert.equal(requests.length, 6, 'unexpected synthetic requests');
    console.log('Synthetic desktop browser smoke passed: list, summary, focus, cancellation, retry, denied, narrow reflow.');
  } finally {
    if(session) { try { await call(wd,'DELETE',`/session/${session}`); } catch {} }
    if (proc && proc.exitCode === null && proc.pid) {
      proc.kill('SIGTERM');
      await Promise.race([new Promise(resolve => proc.once('exit', resolve)), delay(3000)]);
      if (proc.exitCode === null) proc.kill('SIGKILL');
    }
    if (server.listening) {
      server.closeAllConnections();
      await Promise.race([new Promise(resolve => server.close(resolve)), delay(3000)]);
    }
  }
})().catch(e=>{console.error(e); process.exitCode=1;});
