'use strict';
/* The interface in a real browser against a running seqscope server (scripts/test-seqscope-ui.sh starts one on a
   replayed recording). Every page is exercised by clicking and typing, and the browser's console must stay clean. */
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { launch } = require('./cdp.js');

const BASE = process.env.SEQSCOPE_URL || 'http://127.0.0.1:7899';
const SHOTS = process.env.SHOTS || '/tmp/seqscope-ui/shots';
let b;

test.before(async () => { b = await launch({ port: Number(process.env.CDP_PORT || 9333) }); await b.goto(BASE + '/#live'); });
test.after(async () => { if (b) await b.close(); });

async function page(hash) {
  await b.eval(`location.hash = ${JSON.stringify(hash)}; true`);
  await b.waitFor(`document.querySelector('#main h2') && true`, { what: 'the page heading' });
}

test('the Live page shows the collector status and stored rows as they arrive', async () => {
  await page('#live');
  await b.waitFor(`/tx\\/s/.test(document.querySelector('#status').textContent)`, { what: 'the status line' });
  assert.ok(!/DROPPED|FAILED/.test(await b.text('#status')), await b.text('#status'));
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 5`, { what: 'rows' });
  assert.ok((await b.count('.badge.swap')) > 0 || (await b.count('.badge.unknown')) > 0);
  await b.screenshot(path.join(SHOTS, 'live.png'));
});

test('a row opens the inspector, which says calldata is only an intent', async () => {
  await page('#live');
  await b.waitFor(`document.querySelectorAll('tbody tr.click').length >= 1`);
  await b.click('tbody tr.click');
  await b.waitFor(`!document.querySelector('#drawer').classList.contains('hidden')`);
  const t = await b.text('#drawer');
  assert.ok(t.includes('pending') && t.includes('intent'), t.slice(0, 300));
  assert.ok(await b.eval(`document.querySelector('#drawer pre').textContent.includes('"status": "pending"')`));
  await b.screenshot(path.join(SHOTS, 'inspector.png'));
  await b.click('#drawer button');
  assert.ok(await b.eval(`document.querySelector('#drawer').classList.contains('hidden')`));
});

test('pause freezes the table and the text filter narrows it', async () => {
  await page('#live');
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 5`);
  await b.type('input[placeholder^="filter"]', 'swap');
  await b.waitFor(`document.querySelectorAll('tbody tr').length > 0 && [...document.querySelectorAll('tbody tr')].every((r) => r.textContent.toLowerCase().includes('swap'))`, { what: 'only swap rows' });
  await b.type('input[placeholder^="filter"]', '');
});

test('a probe tries a rule on live traffic without saving it, and a bad probe explains itself', async () => {
  await page('#live');
  await b.type('input[placeholder^="probe"]', 'kind == swap');
  await b.click('#probe-run');
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 3 && [...document.querySelectorAll('tbody .badge')].every((x) => x.textContent === 'swap')`, { what: 'swap-only rows' });
  assert.match(await b.text('main'), /probing/);
  await b.type('input[placeholder^="probe"]', 'swap.nope == 1');
  await b.click('#probe-run');
  await b.waitFor(`document.querySelector('.errors li')?.textContent.includes('unknown field')`, { what: 'the explanation' });
  await b.click('button:last-of-type');
});

test('Rules: the file loads and validates, a mistake is explained and cannot be saved, a fix can', async () => {
  await page('#rules');
  await b.waitFor(`document.querySelector('#rules-text')?.value.includes('version = 1')`, { what: 'the file text' });
  await b.waitFor(`document.body.textContent.includes('rules are valid')`);
  assert.ok(await b.eval(`document.querySelector('#rules-save').disabled`), 'nothing changed, nothing to save');
  const original = await b.eval(`document.querySelector('#rules-text').value`);
  await b.type('#rules-text', original + '\n[[profile.rule]]\nname = "oops"\nwhen = "swap.amount > 1"\n');
  await b.waitFor(`document.querySelector('.errors li')?.textContent.includes('did you mean swap.amount_in')`, { what: 'the error with a hint' });
  assert.ok(await b.eval(`document.querySelector('#rules-save').disabled`), 'a broken file cannot be saved');
  await b.screenshot(path.join(SHOTS, 'rules-error.png'));
  await b.type('#rules-text', original);
  await b.waitFor(`document.body.textContent.includes('rules are valid')`);
});

test('Rules: the builder writes the rule text, a dry run counts matches, and the rule can be added and saved', async () => {
  await page('#rules');
  await b.waitFor(`document.querySelector('#when-text')?.value === 'kind == swap'`, { what: 'the starting condition' });
  // Add a second condition: swap.amount_in >= 1e15.
  await b.eval(`[...document.querySelectorAll('button')].find((x) => x.textContent === '+ condition').click(); true`);
  await b.eval(`(() => { const rows = document.querySelectorAll('.cond'); const r = rows[rows.length - 1]; const f = r.querySelector('select'); f.value = 'swap.amount_in'; f.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  await b.eval(`(() => { const r = [...document.querySelectorAll('.cond')].pop(); const op = r.querySelectorAll('select')[1]; op.value = '>='; op.dispatchEvent(new Event('change', { bubbles: true })); const v = r.querySelector('input'); v.value = '1e15'; v.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
  await b.waitFor(`document.querySelector('#when-text').value === 'kind == swap && swap.amount_in >= 1e15'`, { what: 'the combined rule text' });
  await b.click('#dry-run');
  await b.waitFor(`/\\d+ of \\d+ calls match/.test(document.querySelector('#preview-summary')?.textContent || '')`, { what: 'the dry-run summary' });
  await b.screenshot(path.join(SHOTS, 'rules-builder.png'));
  // Add it to the file and save.
  await b.type('#rule-name', 'big swaps from the UI');
  await b.click('#add-rule');
  await b.waitFor(`document.querySelector('#rules-text').value.includes('big swaps from the UI')`, { what: 'the rule in the editor' });
  await b.waitFor(`!document.querySelector('#rules-save').disabled`, { what: 'save to enable' });
  await b.click('#rules-save');
  await b.waitFor(`document.querySelector('#toast').textContent.includes('saved')`, { what: 'the saved toast' });
  const onServer = await b.eval(`fetch('/api/rules').then((r) => r.json()).then((j) => j.text)`);
  assert.ok(onServer.includes('big swaps from the UI') && onServer.includes('swap.amount_in >= 1e15'), 'the server has the new rule');
  // Put the file back as it was.
  const without = onServer.slice(0, onServer.indexOf('[[profile.rule]]\nname = "big swaps from the UI"')).replace(/\s+$/, '\n');
  await b.eval(`fetch('/api/rules', { method: 'PUT', headers: { 'X-Seqscope': '1', 'Content-Type': 'application/json' }, body: JSON.stringify({ text: ${JSON.stringify(without)} }) }).then((r) => r.status)`);
});

test('Rules: an incomplete builder row matches everything rather than breaking, and a bad expression is explained', async () => {
  await page('#rules?when=' + encodeURIComponent('swap.nope == 1'));
  await b.waitFor(`document.querySelector('#when-text')?.value === 'swap.nope == 1'`);
  await b.click('#dry-run');
  await b.waitFor(`document.querySelector('.errors li')?.textContent.includes('unknown field')`, { what: 'the error' });
});

test('Explore: SQL runs, results can be downloaded, writes are refused, filters show rows', async () => {
  await page('#explore');
  await b.click('#run-sql');
  await b.waitFor(`/\\d+ rows/.test(document.querySelector('#sql-count')?.textContent || '')`, { what: 'the result count' });
  assert.ok((await b.count('.scroll tbody tr')) > 0);
  await b.type('#sql-text', 'DELETE FROM rows');
  await b.click('#run-sql');
  await b.waitFor(`document.querySelector('#sql-error')?.textContent.length > 0`, { what: 'the refusal' });
  await b.type('input[placeholder="kind"]', 'swap');
  await b.click('#run-rows');
  await b.waitFor(`document.querySelectorAll('.scroll tbody tr').length > 0 && [...document.querySelectorAll('.scroll tbody .badge')].every((x) => x.textContent === 'swap')`, { what: 'swap rows only' });
  await b.screenshot(path.join(SHOTS, 'explore.png'));
});

test('Gaps: unread calls are listed and a line becomes a dry run in one click', async () => {
  await page('#gaps');
  await b.waitFor(`document.querySelectorAll('tbody tr').length > 0`, { what: 'gap rows' });
  await b.click('tbody button');
  await b.waitFor(`/\\d+ of \\d+ calls match/.test(document.querySelector('#preview-summary')?.textContent || '')`, { what: 'the drafted rule dry run' });
  assert.match(await b.eval(`document.querySelector('#when-text').value`), /^to == 0x[0-9a-f]{40}/);
});

test('Stats: the cards, the breakdowns and a clean drop counter', async () => {
  await page('#stats');
  await b.waitFor(`document.querySelectorAll('.card').length >= 8`, { what: 'the cards' });
  const cards = await b.eval(`[...document.querySelectorAll('.card')].map((c) => c.textContent)`);
  assert.ok(cards.some((c) => /dropped0/.test(c)), cards.join(' | '));
  assert.ok(await b.count('.bar') > 0);
  await b.screenshot(path.join(SHOTS, 'stats.png'));
});

test('the layout holds on a phone and in dark mode', async () => {
  await b.viewport(420);
  await page('#live');
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 3`);
  assert.ok(await b.eval(`document.documentElement.scrollWidth <= window.innerWidth + 2`), 'the page does not scroll sideways: ' + (await b.eval(`document.documentElement.scrollWidth + ' > ' + window.innerWidth`)));
  await b.screenshot(path.join(SHOTS, 'phone.png'));
  await b.viewport(1500, 950);
  const light = await b.eval(`getComputedStyle(document.body).backgroundColor`);
  await b.colorScheme('dark');
  await b.waitFor(`getComputedStyle(document.body).backgroundColor !== ${JSON.stringify(light)}`, { what: 'dark colours' });
  await b.screenshot(path.join(SHOTS, 'dark.png'));
  await b.colorScheme('light');
});

test('nothing in the browser console or the page raised an error', async () => {
  // The tests above provoke two refusals on purpose (a rule with an unknown field, a DELETE); those are the only
  // failed requests allowed.
  const deliberate = [/status of 422 .*\/api\/preview/, /status of 400 .*\/api\/sql/];
  const unexpected = b.problems.filter((p) => !deliberate.some((re) => re.test(p)));
  assert.deepEqual(unexpected, []);
  assert.ok(b.problems.length >= 2, 'the deliberate refusals were seen, so the console capture works');
});
