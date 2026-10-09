'use strict';
/* A minimal Chrome DevTools Protocol client: launch headless Chromium, drive a page, read what happened.
   Node 22+ (built-in WebSocket and fetch). No dependencies. */
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function launch({ port = 9333, chromium = process.env.CHROMIUM || 'chromium' } = {}) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'seqscope-chrome-'));
  const proc = spawn(chromium, ['--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--user-data-dir=${profile}`, `--remote-debugging-port=${port}`, '--window-size=1500,950', 'about:blank'], { stdio: 'ignore' });
  let version;
  for (let i = 0; i < 100; i++) {
    try { version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json(); break; } catch (_) { await sleep(100); }
  }
  if (!version) { proc.kill(); throw new Error('chromium did not start'); }
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = list.find((t) => t.type === 'page');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error('devtools socket failed')); });
  let id = 0;
  const pending = new Map();
  const problems = [];
  const handlers = [];
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) { const { res, rej } = pending.get(m.id); pending.delete(m.id); m.error ? rej(new Error(m.error.message)) : res(m.result); return; }
    if (m.method === 'Runtime.exceptionThrown') problems.push('exception: ' + (m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text));
    if (m.method === 'Runtime.consoleAPICalled' && ['error', 'assert'].includes(m.params.type)) problems.push('console.' + m.params.type + ': ' + m.params.args.map((a) => a.value ?? a.description).join(' '));
    if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') problems.push('log: ' + m.params.entry.text + ' ' + (m.params.entry.url || ''));
    handlers.forEach((h) => h(m));
  };
  const send = (method, params = {}) => new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); });
  await send('Runtime.enable'); await send('Page.enable'); await send('Log.enable');

  const api = {
    problems,
    send,
    async goto(url) {
      await send('Page.navigate', { url });
      for (let i = 0; i < 100; i++) { if ((await api.eval('document.readyState')) === 'complete') return; await sleep(100); }
      throw new Error('page did not load: ' + url);
    },
    async eval(expression) {
      const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) throw new Error('eval failed: ' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text) + '\n' + expression);
      return r.result.value;
    },
    async waitFor(expression, { timeout = 10000, what = expression } = {}) {
      const end = Date.now() + timeout;
      let last;
      while (Date.now() < end) {
        try { last = await api.eval(expression); if (last) return last; } catch (e) { last = e.message; }
        await sleep(100);
      }
      throw new Error(`timed out waiting for ${what} (last: ${JSON.stringify(last)})`);
    },
    text: (sel) => api.eval(`document.querySelector(${JSON.stringify(sel)})?.textContent ?? null`),
    count: (sel) => api.eval(`document.querySelectorAll(${JSON.stringify(sel)}).length`),
    click: (sel) => api.eval(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) throw new Error('no element ' + ${JSON.stringify(sel)}); e.click(); return true; })()`),
    type: (sel, value, event = 'input') => api.eval(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) throw new Error('no element ' + ${JSON.stringify(sel)}); e.value = ${JSON.stringify(value)}; e.dispatchEvent(new Event(${JSON.stringify(event)}, { bubbles: true })); return true; })()`),
    key: (sel, key) => api.eval(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); e.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(key)}, bubbles: true })); return true; })()`),
    async screenshot(file) {
      const r = await send('Page.captureScreenshot', { format: 'png' });
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, Buffer.from(r.data, 'base64'));
    },
    viewport: (width, height = 900) => send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: width < 600 }),
    colorScheme: (v) => send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: v }] }),
    async close() { try { ws.close(); } catch (_) { /* gone */ } proc.kill('SIGKILL'); try { fs.rmSync(profile, { recursive: true, force: true }); } catch (_) { /* busy */ } },
  };
  return api;
}

module.exports = { launch, sleep };
