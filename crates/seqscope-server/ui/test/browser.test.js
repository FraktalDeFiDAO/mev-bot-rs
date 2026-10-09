'use strict';
/* The interface in a real browser against a running seqscope server (scripts/test-seqscope-ui.sh starts one on a
   replayed recording). Every page is exercised the way a person uses it, by clicking and typing, and the browser's
   console must stay clean. */
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
/** Go to a page from somewhere else, so it is built fresh even when it is the page already showing. */
async function fresh(hash) {
  await b.eval(`location.hash = '#stats'; true`);
  await b.waitFor(`document.querySelector('#main h2')?.textContent === 'Stats'`);
  await page(hash);
}
const api = (method, url, body) => b.eval(`fetch(${JSON.stringify(url)}, { method: ${JSON.stringify(method)}, headers: { 'X-Seqscope': '1', 'Content-Type': 'application/json' }, body: ${body === undefined ? 'undefined' : JSON.stringify(JSON.stringify(body))} }).then((r) => r.json())`);
const select = (sel, value) => b.type(sel, value, 'change');
const cardNames = () => b.eval(`[...document.querySelectorAll('.rulecard')].map((c) => c.dataset.rule)`);
const toast = () => b.text('#toast');

async function removeAllRules() {
  const r = await api('GET', '/api/rules');
  for (const x of r.rules || []) await api('POST', '/api/rules/op', { op: 'remove_rule', profile: x.profile, rule: x.name });
}

test('Live: calls are sentences a person can read, with a switch for the technical columns', async () => {
  await page('#live');
  await b.waitFor(`/tx\\/s/.test(document.querySelector('#status').textContent)`, { what: 'the status line' });
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 5`, { what: 'rows' });
  const first = await b.eval(`document.querySelector('tbody tr td.wrap').textContent`);
  assert.match(first, /^(Sold|Added|Removed|Liquidation|Call to)/, first);
  assert.ok(!(await b.eval(`[...document.querySelectorAll('thead th')].some((th) => th.textContent === 'seq')`)), 'technical columns are hidden at first');
  await b.click('#tech-toggle');
  await b.waitFor(`[...document.querySelectorAll('thead th')].some((th) => th.textContent === 'seq')`, { what: 'the technical columns' });
  await b.click('#tech-toggle');
  await b.screenshot(path.join(SHOTS, 'live.png'));
});

test('Live: a row opens an inspector that explains itself and offers to watch for more like it', async () => {
  await page('#live');
  await b.waitFor(`document.querySelectorAll('tbody tr.click').length >= 1`);
  await b.click('tbody tr.click');
  await b.waitFor(`!document.querySelector('#drawer').classList.contains('hidden')`);
  const t = await b.text('#drawer');
  assert.ok(t.includes('pending') && t.includes('intent'), t.slice(0, 300));
  assert.ok(t.includes('Watch for more like this'));
  await b.screenshot(path.join(SHOTS, 'inspector.png'));
  await b.click('#drawer button');
  assert.ok(await b.eval(`document.querySelector('#drawer').classList.contains('hidden')`));
});

test('Live: the search box narrows what is shown', async () => {
  await page('#live');
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 5`);
  await b.type('input[placeholder^="search"]', 'sold');
  await b.waitFor(`document.querySelectorAll('tbody tr').length > 0 && [...document.querySelectorAll('tbody tr')].every((r) => /sold/i.test(r.textContent))`, { what: 'only matching rows' });
  await b.type('input[placeholder^="search"]', '');
});

test('Live: an idea can be tried in one click, is explained in words, and nothing is saved', async () => {
  await removeAllRules();
  await page('#live');
  // Let what was already queued reach the database, then measure over a quiet stretch.
  await new Promise((r) => setTimeout(r, 1500));
  const before = await b.eval(`fetch('/api/stats').then((r) => r.json()).then((s) => s.db.rows)`);
  await b.waitFor(`document.querySelectorAll('.chip').length >= 5`);
  // An idea with a real amount in it is explained in the token's own units.
  await b.eval(`[...document.querySelectorAll('.chip')].find((c) => c.textContent === 'Big USDC swaps').click(); true`);
  await b.waitFor(`document.querySelector('.explain')?.textContent.includes('sells at least 10,000 USDC')`, { what: 'the explanation in words' });
  // And a common one shows matches straight away.
  await b.eval(`[...document.querySelectorAll('.chip')].find((c) => c.textContent === 'Swaps').click(); true`);
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 3 && [...document.querySelectorAll('tbody .badge')].every((x) => x.textContent === 'swap')`, { what: 'swap rows' });
  assert.match(await b.text('main'), /nothing is saved/);
  await new Promise((r) => setTimeout(r, 1000));
  const after = await b.eval(`fetch('/api/stats').then((r) => r.json()).then((s) => s.db.rows)`);
  assert.equal(after, before, 'trying rules stored nothing');
  await b.type('#probe-text', 'swap.nope == 1');
  await b.click('#probe-run');
  await b.waitFor(`document.querySelector('.errors li')?.textContent.includes('unknown field')`, { what: 'the explanation of the mistake' });
});

test('Live: a tried idea becomes a rule in one click', async () => {
  await page('#live');
  await b.type('#probe-text', 'kind == liquidity');
  await b.waitFor(`!document.querySelector('#probe-save').disabled`, { what: 'the save button' });
  await b.click('#probe-save');
  await b.waitFor(`document.querySelector('#rule-when')?.value === 'kind == liquidity'`, { what: 'the editor with the rule filled in' });
  await b.waitFor(`document.querySelector('#rule-editor .explain')?.textContent.includes('liquidity change')`, { what: 'its meaning in words' });
  await b.click('#rule-cancel');
});

test('Rules: the page starts from what you want to watch, not from syntax', async () => {
  await removeAllRules();
  await page('#rules');
  await b.waitFor(`document.querySelectorAll('[data-recipe]').length >= 8`, { what: 'the recipes' });
  const titles = await b.eval(`[...document.querySelectorAll('[data-recipe] b')].map((x) => x.textContent)`);
  assert.ok(titles.includes('Big swaps') && titles.includes('Liquidations') && titles.some((t) => /Aave/.test(t)), titles.join(' | '));
  await b.waitFor(`document.querySelector('#no-rules')`, { what: 'the friendly empty state' });
  assert.ok((await b.text('#no-rules')).includes('You have no rules yet'));
  assert.ok(await b.eval(`!document.querySelector('#advanced').open`), 'the text editor is tucked away');
  await b.screenshot(path.join(SHOTS, 'rules-empty.png'));
});

test('Rules: "Big swaps" asks in real units, says what it will catch, shows examples, and saves', async () => {
  await page('#rules');
  await b.waitFor(`document.querySelectorAll('[data-recipe]').length >= 8`);
  await b.click('[data-recipe="big-swaps"]');
  await b.waitFor(`document.querySelector('#f-token')`);
  await select('#f-token', 'USDC');
  await b.type('#f-min', '10000');
  await b.waitFor(`document.querySelector('#rule-editor .explain')?.textContent === 'Catches a call when it is a swap and it sells at least 10,000 USDC.'`, { what: 'the plain-English meaning' });
  await b.waitFor(`/would have caught/.test(document.querySelector('#preview-summary')?.textContent || '')`, { what: 'the preview' });
  assert.equal(await b.eval(`document.querySelector('#rule-name-in').value`), 'Big USDC swaps (≥ 10000)');
  await b.screenshot(path.join(SHOTS, 'rules-recipe.png'));
  await b.click('#rule-save');
  await b.waitFor(`document.querySelector('.rulecard')`, { what: 'the new rule in My rules' });
  assert.deepEqual(await cardNames(), ['Big USDC swaps (≥ 10000)']);
  assert.match(await b.text('.rulecard .explain'), /sells at least 10,000 USDC/);
  assert.ok((await toast()).includes('catching things now'));
  assert.ok(await b.eval(`document.querySelector('#rule-editor') === null`), 'the form closed');
});

test('Rules: a rule can be switched off and on, copied, and deleted after a confirmation', async () => {
  await page('#rules');
  await b.waitFor(`document.querySelector('.rulecard')`);
  await b.click('.rulecard input.switch');
  await b.waitFor(`document.querySelector('.rulecard.off') && /Switched off/.test(document.querySelector('.rulecard').textContent)`, { what: 'the rule shown as off' });
  await b.screenshot(path.join(SHOTS, 'rules-list.png'));
  await b.click('.rulecard input.switch');
  await b.waitFor(`!document.querySelector('.rulecard.off')`, { what: 'the rule on again' });
  await b.eval(`[...document.querySelectorAll('.rulecard button')].find((x) => x.textContent === 'Copy').click(); true`);
  await b.waitFor(`document.querySelectorAll('.rulecard').length === 2`, { what: 'the copy' });
  assert.ok((await cardNames()).some((n) => /\(copy\)/.test(n)));
  // Delete asks first.
  await b.eval(`[...document.querySelectorAll('.rulecard')].find((c) => /copy/.test(c.dataset.rule)).querySelector('button.danger').click(); true`);
  await b.waitFor(`/Delete it for good/.test(document.body.textContent)`, { what: 'the confirmation' });
  await b.eval(`[...document.querySelectorAll('button')].find((x) => x.textContent === 'No').click(); true`);
  assert.equal((await cardNames()).length, 2, 'saying no keeps it');
  await b.eval(`[...document.querySelectorAll('.rulecard')].find((c) => /copy/.test(c.dataset.rule)).querySelector('button.danger').click(); true`);
  await b.eval(`[...document.querySelectorAll('button')].find((x) => x.textContent === 'Yes, delete').click(); true`);
  await b.waitFor(`document.querySelectorAll('.rulecard').length === 1`, { what: 'the copy deleted' });
});

test('Rules: an existing rule can be changed, with the same explanation and examples', async () => {
  await page('#rules');
  await b.waitFor(`document.querySelector('.rulecard')`);
  await b.eval(`[...document.querySelectorAll('.rulecard button')].find((x) => x.textContent === 'Edit').click(); true`);
  await b.waitFor(`document.querySelector('#rule-when')?.value.includes('kind == swap')`, { what: 'the rule loaded into the editor' });
  await b.type('#rule-when', 'kind == swap && swap.amount_in > 5e17');
  await b.waitFor(`document.querySelector('#rule-editor .explain')?.textContent.includes('more than 500,000,000,000,000,000')`, { what: 'the new explanation' });
  await b.type('#rule-name-in', 'Larger swaps');
  await b.waitFor(`!document.querySelector('#rule-save').disabled`);
  await b.click('#rule-save');
  await b.waitFor(`document.querySelector('.rulecard[data-rule="Larger swaps"]')`, { what: 'the renamed rule' });
});

test('Rules: mistakes are explained in words and block saving', async () => {
  await page('#rules');
  await b.waitFor(`document.querySelectorAll('[data-recipe]').length >= 8`);
  await b.click('[data-recipe="address"]');
  await b.waitFor(`document.querySelector('#f-address')`);
  await b.type('#f-address', '0x123');
  await b.waitFor(`/starts with 0x and has 40 more/.test(document.querySelector('#rule-editor .err')?.textContent || '')`, { what: 'the plain explanation' });
  assert.ok(await b.eval(`document.querySelector('#rule-save').disabled`));
  await b.type('#f-address', '0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45');
  await b.waitFor(`document.querySelector('#rule-editor .explain')?.textContent.includes('goes to Uniswap SwapRouter02')`, { what: 'the contract named in words' });
  await b.click('#rule-cancel');
  // The hand-written route: a bad rule is explained, not rejected silently.
  await b.click('#custom-rule');
  await b.waitFor(`document.querySelector('#rule-when')`);
  await b.type('#rule-when', 'swap.amount > 5');
  await b.waitFor(`document.querySelector('#rule-editor .explain.bad')?.textContent.includes('did you mean swap.amount_in')`, { what: 'the hint' });
  assert.ok(await b.eval(`document.querySelector('#rule-save').disabled`));
  await b.click('#rule-cancel');
});

test('Rules: a starter set is described in words and added once', async () => {
  await removeAllRules();
  await fresh('#rules');
  await b.waitFor(`document.querySelector('[data-starter="aave-lending"]')`);
  const text = await b.text('[data-starter="aave-lending"]');
  assert.ok(text.includes('Add 5 rules'), text);
  await b.eval(`document.querySelector('[data-starter="aave-lending"] summary').click(); true`);
  assert.ok((await b.text('[data-starter="aave-lending"] ul')).includes('Aave: borrow'));
  await b.eval(`[...document.querySelectorAll('[data-starter="aave-lending"] button')].find((x) => /^Add/.test(x.textContent)).click(); true`);
  await b.waitFor(`document.querySelectorAll('.rulecard').length === 5`, { what: 'the five rules' });
  await b.waitFor(`/already added/.test(document.querySelector('[data-starter="aave-lending"]').textContent)`);
  await b.screenshot(path.join(SHOTS, 'rules-starter.png'));
  await removeAllRules();
});

test('Rules: the rules file is still there for people who want the text', async () => {
  await api('POST', '/api/rules/op', { op: 'add_rule', profile: 'My rules', rule: { name: 'for the text view', when: 'kind == swap' } });
  await fresh('#rules');
  await b.eval(`document.querySelector('#advanced').open = true; true`);
  await b.waitFor(`document.querySelector('#rules-text')?.value.includes('for the text view')`, { what: 'the file text' });
  await b.waitFor(`document.body.textContent.includes('rules are valid')`);
  const original = await b.eval(`document.querySelector('#rules-text').value`);
  await b.type('#rules-text', original + '\n[[profile.rule]]\nname = "oops"\nwhen = "swap.amount > 1"\n');
  await b.waitFor(`[...document.querySelectorAll('.errors li')].some((li) => li.textContent.includes('did you mean swap.amount_in'))`, { what: 'the error with a hint' });
  assert.ok(await b.eval(`document.querySelector('#rules-save').disabled`), 'a broken file cannot be saved');
  await b.type('#rules-text', original);
  await b.waitFor(`document.body.textContent.includes('rules are valid')`);
  await removeAllRules();
});

test('Explore: SQL runs, results can be downloaded, writes are refused, filters show rows', async () => {
  await api('POST', '/api/rules/op', { op: 'add_rule', profile: 'My rules', rule: { name: 'everything to look at', when: 'true' } });
  await b.waitFor(`fetch('/api/stats').then((r) => r.json()).then((s) => s.db.rows > 50)`, { what: 'rows to explore' });
  await page('#explore');
  await b.click('#run-sql');
  await b.waitFor(`/\\d+ rows/.test(document.querySelector('#sql-count')?.textContent || '')`, { what: 'the result count' });
  assert.ok((await b.count('.scroll tbody tr')) > 0);
  await b.type('#sql-text', 'DELETE FROM rows');
  await b.click('#run-sql');
  await b.waitFor(`document.querySelector('#sql-error')?.textContent.length > 0`, { what: 'the refusal' });
  await b.type('input[placeholder="kind"]', 'swap');
  await b.click('#run-rows');
  await b.waitFor(`document.querySelectorAll('.scroll tbody tr').length > 0`, { what: 'rows' });
  await b.screenshot(path.join(SHOTS, 'explore.png'));
});

test('Gaps: unread calls are listed with names where we have them, and one click starts a rule', async () => {
  await page('#gaps');
  await b.waitFor(`document.querySelectorAll('tbody tr').length > 0`, { what: 'gap rows' });
  await b.click('tbody button');
  await b.waitFor(`/^to == 0x[0-9a-f]{40}/.test(document.querySelector('#rule-when')?.value || '')`, { what: 'the editor with the rule drafted' });
  await b.waitFor(`/goes to/.test(document.querySelector('#rule-editor .explain')?.textContent || '')`, { what: 'its meaning in words' });
  await b.click('#rule-cancel');
});

test('Stats: the cards, the breakdowns and a clean drop counter', async () => {
  await page('#stats');
  await b.waitFor(`document.querySelectorAll('.card').length >= 8`, { what: 'the cards' });
  const cards = await b.eval(`[...document.querySelectorAll('.card')].map((c) => c.textContent)`);
  assert.ok(cards.some((c) => /dropped0/.test(c)), cards.join(' | '));
  await b.screenshot(path.join(SHOTS, 'stats.png'));
});

test('the layout holds on a phone and in dark mode', async () => {
  await b.viewport(420);
  await page('#rules');
  await b.waitFor(`document.querySelectorAll('[data-recipe]').length >= 8`);
  assert.ok(await b.eval(`document.documentElement.scrollWidth <= window.innerWidth + 2`), 'the Rules page does not scroll sideways: ' + (await b.eval(`document.documentElement.scrollWidth + ' > ' + window.innerWidth`)));
  await b.screenshot(path.join(SHOTS, 'phone-rules.png'));
  await page('#live');
  await b.waitFor(`document.querySelectorAll('tbody tr').length >= 3`);
  assert.ok(await b.eval(`document.documentElement.scrollWidth <= window.innerWidth + 2`), 'the Live page does not scroll sideways');
  await b.viewport(1500, 950);
  const light = await b.eval(`getComputedStyle(document.body).backgroundColor`);
  await b.colorScheme('dark');
  await b.waitFor(`getComputedStyle(document.body).backgroundColor !== ${JSON.stringify(light)}`, { what: 'dark colours' });
  await page('#rules');
  await b.screenshot(path.join(SHOTS, 'dark-rules.png'));
  await b.colorScheme('light');
});

test('nothing in the browser console or the page raised an error', async () => {
  await removeAllRules();
  // The tests above provoke a few refusals on purpose (a rule with an unknown field, a DELETE, a bad address
  // explained before it is sent); those are the only failed requests allowed.
  const deliberate = [/status of 422 .*\/api\/(preview|explain|rules\/op)/, /status of 400 .*\/api\/sql/];
  const unexpected = b.problems.filter((p) => !deliberate.some((re) => re.test(p)));
  assert.deepEqual(unexpected, []);
});
