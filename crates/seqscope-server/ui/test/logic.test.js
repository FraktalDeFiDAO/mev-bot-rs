'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const L = require('../app.js');

test('operators depend on the field type and always allow set()', () => {
  assert.deepEqual(L.opsFor('num'), ['==', '!=', '>', '>=', '<', '<=', 'set']);
  assert.ok(!L.opsFor('addr').includes('>'));
  assert.ok(L.opsFor('str_list').includes('contains') && !L.opsFor('str_list').includes('=='));
  assert.ok(L.opsFor('nonsense').includes('=='), 'an unknown type still gets a usable operator');
});

test('text values are bare words when they can be and quoted when they cannot', () => {
  assert.equal(L.litText('swap', 'str'), 'swap');
  assert.equal(L.litText('aave_v3', 'str'), 'aave_v3');
  for (const kw of ['true', 'in', 'contains', 'set', 'and', 'or', 'not', 'false']) assert.equal(L.litText(kw, 'str'), '"' + kw + '"', kw);
  assert.equal(L.litText('Universal Router', 'str'), '"Universal Router"');
  assert.equal(L.litText('9lives', 'str'), '"9lives"');
  assert.equal(L.litText('say "hi"', 'str'), '"say \\"hi\\""');
  assert.equal(L.litText(' 5e6 ', 'num'), '5e6', 'numbers pass through, trimmed');
  assert.equal(L.litText('0xa415bcad', 'sel'), '0xa415bcad');
});

test('a condition becomes rule text, or null while it is incomplete', () => {
  assert.equal(L.condText({ field: 'kind', op: '==', value: 'swap', ty: 'str' }), 'kind == swap');
  assert.equal(L.condText({ field: 'swap.amount_in', op: '>=', value: '1e18', ty: 'num' }), 'swap.amount_in >= 1e18');
  assert.equal(L.condText({ field: 'selector', op: 'in', value: '0xa415bcad, 0x573ade81', ty: 'sel' }), 'selector in [0xa415bcad, 0x573ade81]');
  assert.equal(L.condText({ field: 'via', op: 'contains', value: 'multicall3', ty: 'str_list' }), 'via contains multicall3');
  assert.equal(L.condText({ field: 'swap.pool', op: 'set', value: '', ty: 'addr' }), 'set(swap.pool)');
  assert.equal(L.condText({ field: 'kind', op: '==', value: '   ', ty: 'str' }), null);
  assert.equal(L.condText({ field: 'selector', op: 'in', value: ' , ', ty: 'sel' }), null);
  assert.equal(L.condText({ field: '', op: '==', value: 'x' }), null);
  assert.equal(L.condText(null), null);
});

test('conditions join with AND or OR, and an empty builder matches everything', () => {
  const a = { field: 'kind', op: '==', value: 'swap', ty: 'str' };
  const b = { field: 'protocol', op: '==', value: 'aave_v3', ty: 'str' };
  const incomplete = { field: 'to', op: '==', value: '', ty: 'addr' };
  assert.equal(L.buildWhen([], 'and'), 'true');
  assert.equal(L.buildWhen([incomplete], 'and'), 'true');
  assert.equal(L.buildWhen([a], 'or'), 'kind == swap');
  assert.equal(L.buildWhen([a, b, incomplete], 'and'), 'kind == swap && protocol == aave_v3');
  assert.equal(L.buildWhen([a, b], 'or'), 'kind == swap || protocol == aave_v3');
});

test('a rule block is valid TOML text with its quotes escaped', () => {
  const t = L.tomlRule({ name: 'big "swaps"', when: 'kind == "a b"', action: 'alert', tag: 't', sample: 5 });
  assert.equal(t, '[[profile.rule]]\nname = "big \\"swaps\\""\nwhen = "kind == \\"a b\\""\naction = "alert"\ntag = "t"\nsample = 5\n');
  assert.equal(L.tomlRule({ name: 'x', when: 'true', action: 'store' }), '[[profile.rule]]\nname = "x"\nwhen = "true"\n', 'defaults are left out');
});

const FILE = 'version = 1\n\n[[profile]]\nname = "swaps"\nmode = "all"\n\n[[profile.rule]]\nname = "a"\nwhen = "kind == swap"\n\n[[profile]]\nname = "gaps"\n\n[[profile.rule]]\nname = "g"\nwhen = "kind == unknown"\n';

test('profiles are found by name', () => {
  assert.deepEqual(L.profileNames(FILE), ['swaps', 'gaps']);
  assert.deepEqual(L.profileNames(''), []);
  assert.deepEqual(L.profileNames('[[profile]]\nname = "has \\"quote\\""\n'), ['has "quote"']);
});

test('a rule is added to the named profile, to a new one, or to an empty file', () => {
  const r = { name: 'new', when: 'kind == liquidity', action: 'store' };
  const into = L.addRuleToText(FILE, r, 'swaps');
  assert.ok(into.indexOf('name = "new"') > into.indexOf('name = "a"') && into.indexOf('name = "new"') < into.indexOf('name = "gaps"'), 'inside the swaps profile, before the next one');
  assert.equal(into.match(/\[\[profile\]\]/g).length, 2);
  const last = L.addRuleToText(FILE, r, 'gaps');
  assert.ok(last.endsWith('[[profile.rule]]\nname = "new"\nwhen = "kind == liquidity"\n'));
  const fresh = L.addRuleToText(FILE, r, 'brand new');
  assert.equal(fresh.match(/\[\[profile\]\]/g).length, 3);
  assert.ok(fresh.includes('name = "brand new"\nmode = "all"'));
  const empty = L.addRuleToText('', r, 'rules');
  assert.ok(empty.startsWith('version = 1\n'));
  assert.deepEqual(L.profileNames(empty), ['rules']);
  assert.ok(!L.addRuleToText('', r, 'rules').includes('undefined'));
});

test('formatting helpers', () => {
  assert.equal(L.clock(1700000000123), '22:13:20.123');
  assert.equal(L.shortHex('0x1234567890abcdef1234567890abcdef12345678'), '0x123456…5678');
  assert.equal(L.shortHex('0xabc'), '0xabc');
  assert.equal(L.shortHex(null), '-');
  const row = { fields: { 'swap.amount_in': '1000', 'swap.pool': '0x1234567890abcdef1234567890abcdef12345678', 'swap.hops': '1' } };
  assert.equal(L.fieldSummary(row), 'amount_in=1000 pool=0x123456…5678');
  assert.equal(L.fieldSummary({}), '');
});

test('"make a rule like this" uses protocol, destination and function', () => {
  assert.equal(L.whenLike({ protocol: 'aave_v3', to: '0xabc', selector: '0xa415bcad', kind: 'unknown' }), 'protocol == aave_v3 && to == 0xabc && selector == 0xa415bcad');
  assert.equal(L.whenLike({ to: '0xabc', selector: null, kind: 'unknown' }), 'to == 0xabc');
  assert.equal(L.whenLike({ kind: 'swap' }), 'kind == swap');
});

test('a gaps line becomes a rule for that destination and function', () => {
  const cols = ['kind', 'contract', 'destination', 'selector', 'calls'];
  assert.equal(L.whenForGap(cols, ['unknown', '(unidentified)', '0xaf88', '0xa9059cbb', 11]), 'to == 0xaf88 && selector == 0xa9059cbb');
  assert.equal(L.whenForGap(cols, ['unknown', 'x', null, null, 1]), 'kind == unknown');
});

test('CSV quotes what needs quoting and ends with a newline', () => {
  assert.equal(L.csvOf(['a', 'b'], [[1, 'x,y'], [null, 'say "hi"'], ['line\nbreak', 0]]), 'a,b\n1,"x,y"\n,"say ""hi"""\n"line\nbreak",0\n');
  assert.equal(L.csvOf(['a'], []), 'a\n');
});

test('hash routes carry a page and parameters', () => {
  assert.deepEqual([L.parseHash('').page, L.parseHash('#').page], ['live', 'live']);
  const r = L.parseHash('#rules?when=kind%20%3D%3D%20swap&preview=1');
  assert.equal(r.page, 'rules');
  assert.equal(r.params.get('when'), 'kind == swap');
  assert.equal(r.params.get('preview'), '1');
  assert.equal(L.parseHash('#explore').params.get('x'), null);
});

test('the sparkline scales to its box and copes with nothing', () => {
  assert.equal(L.sparkPoints([], 100, 20), '');
  assert.equal(L.sparkPoints([5], 100, 20), '0.0,1.0');
  const pts = L.sparkPoints([0, 10, 5], 100, 20).split(' ');
  assert.equal(pts.length, 3);
  assert.equal(pts[0], '0.0,19.0');
  assert.equal(pts[1].split(',')[0], '50.0');
  assert.equal(L.sparkPoints([0, 0], 100, 20).split(' ').length, 2, 'all zeros do not divide by zero');
});

// ------------------------------------------------------------------------------------------------------------
// Plain language

const CAT = {
  tokens: [
    { symbol: 'USDC', address: '0xaf88d065e77c8cc2239327c5edb3a432268e5831', decimals: 6 },
    { symbol: 'WETH', address: '0x82af49447d8a07e3bd95bd0d56f35241523fbab1', decimals: 18 },
  ],
  protocols: [{ id: 'aave_v3', label: 'Aave V3' }, { id: 'uniswap_v3', label: 'Uniswap V3' }],
  functions: [['a415bcad', 'Aave: borrow'], ['a9059cbb', 'token transfer']],
};

test('raw amounts read like money, exactly', () => {
  assert.equal(L.humanAmount('1234567890', 6), '1,234.56');
  assert.equal(L.humanAmount('1000000', 6), '1');
  assert.equal(L.humanAmount('500000', 6), '0.5');
  assert.equal(L.humanAmount('1', 6), '0.000001');
  assert.equal(L.humanAmount('0', 6), '0');
  assert.equal(L.humanAmount('123456789012345678901234', 18), '123,456.78');
  assert.equal(L.humanAmount('abc', 6), 'abc');
  assert.equal(L.humanAmount('42', 0), '42');
});

test('what a person types becomes the raw number a rule needs, with no rounding', () => {
  assert.deepEqual(L.parseAmount('10000', 6), { ok: true, raw: '10000000000' });
  assert.deepEqual(L.parseAmount('10,000.5', 6), { ok: true, raw: '10000500000' });
  assert.deepEqual(L.parseAmount('0.000001', 6), { ok: true, raw: '1' });
  assert.deepEqual(L.parseAmount('.5', 6), { ok: true, raw: '500000' });
  assert.deepEqual(L.parseAmount('1.5', 18), { ok: true, raw: '1500000000000000000' });
  assert.equal(L.parseAmount('0.1234567', 6).ok, false, 'more places than the token has');
  assert.match(L.parseAmount('0.1234567', 6).error, /only 6 decimal places/);
  for (const bad of ['', '  ', 'ten', '1e5', '1.2.3', '-5', '.']) assert.equal(L.parseAmount(bad, 6).ok, false, JSON.stringify(bad));
  // And back: what was typed is what is shown.
  for (const t of ['1', '10000', '0.5', '123456.78']) assert.equal(L.humanAmount(L.parseAmount(t, 6).raw, 6).replace(/,/g, ''), t);
});

test('big raw numbers are written short in a rule', () => {
  assert.equal(L.rawLiteral('10000000000'), '1e10');
  assert.equal(L.rawLiteral('25000000000'), '25e9');
  assert.equal(L.rawLiteral('1500'), '1500');
  assert.equal(L.rawLiteral('10000'), '1e4');
  assert.equal(L.rawLiteral('12345'), '12345');
  assert.equal(L.rawLiteral('0'), '0');
});

test('addresses and function ids are checked in words a person understands', () => {
  assert.ok(L.isAddress('0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45'));
  assert.ok(!L.isAddress('0x1234') && !L.isAddress('68b3465833fb72A70ecDF485E0e4C7bD8665Fc45') && !L.isAddress(null));
  assert.ok(L.isSelector('0xa415bcad') && !L.isSelector('0xa415bc') && !L.isSelector('a415bcad'));
});

const recipe = (id) => L.RECIPES.find((r) => r.id === id);

test('every recipe has an id, a title, a one-line description and a builder', () => {
  const ids = new Set();
  for (const r of L.RECIPES) {
    assert.ok(r.id && r.title && r.blurb && typeof r.build === 'function', r.id);
    assert.ok(!ids.has(r.id), 'duplicate ' + r.id);
    ids.add(r.id);
    assert.ok(r.blurb.length < 110, r.id + ' description is too long to scan');
  }
  assert.ok(L.RECIPES.length >= 8);
});

test('big swaps: the amount is in the token the person picked', () => {
  const b = recipe('big-swaps');
  assert.deepEqual(b.build({ token: 'USDC', min: '10000', protocol: '' }, CAT), {
    when: 'kind == swap && swap.token_in == 0xaf88d065e77c8cc2239327c5edb3a432268e5831 && swap.amount_in >= 1e10',
    name: 'Big USDC swaps (≥ 10000)',
  });
  const w = b.build({ token: 'WETH', min: '0.5', protocol: 'uniswap_v3' }, CAT);
  assert.equal(w.when, 'kind == swap && protocol == uniswap_v3 && swap.token_in == 0x82af49447d8a07e3bd95bd0d56f35241523fbab1 && swap.amount_in >= 5e17');
  assert.match(w.name, /Big WETH swaps .* on Uniswap V3/);
  assert.equal(b.build({ token: '', min: '', protocol: '' }, CAT).when, 'kind == swap');
  assert.equal(b.build({ token: 'USDC', min: '', protocol: '' }, CAT).when, 'kind == swap && swap.token_in == 0xaf88d065e77c8cc2239327c5edb3a432268e5831');
  assert.match(b.build({ token: '', min: '500', protocol: '' }, CAT).error, /Pick which token/);
  assert.match(b.build({ token: 'USDC', min: 'lots', protocol: '' }, CAT).error, /not an amount/);
  assert.match(b.build({ token: 'DOGE', min: '1', protocol: '' }, CAT).error, /Unknown token/);
});

test('the other recipes write the rule they promise', () => {
  assert.equal(recipe('protocol').build({ protocol: 'aave_v3', what: '' }, CAT).when, 'protocol == aave_v3');
  assert.equal(recipe('protocol').build({ protocol: 'aave_v3', what: 'liquidation' }, CAT).when, 'protocol == aave_v3 && kind == liquidation');
  assert.match(recipe('protocol').build({ protocol: '' }, CAT).error, /Pick one/);
  assert.equal(recipe('aave').build({ which: ['borrow'] }).when, 'protocol == aave_v3 && selector == 0xa415bcad');
  assert.equal(recipe('aave').build({ which: ['supply', 'repay'] }).when, 'protocol == aave_v3 && selector in [0x617ba037, 0x573ade81]');
  assert.match(recipe('aave').build({ which: [] }).error, /at least one/);
  assert.equal(recipe('liquidity').build({ what: 'add', protocol: '' }, CAT).when, 'kind == liquidity && liq.action == add');
  assert.equal(recipe('liquidity').build({ what: 'both', protocol: 'uniswap_v3' }, CAT).when, 'kind == liquidity && protocol == uniswap_v3');
  const lq = recipe('liquidations').build({ protocol: '' }, CAT);
  assert.deepEqual([lq.when, lq.action], ['kind == liquidation', 'alert']);
  assert.equal(recipe('unread').build({ what: 'both' }).when, 'kind == undecoded_known || (kind == unknown && len >= 4)');
  assert.equal(recipe('unread').build({ what: 'known' }).when, 'kind == undecoded_known');
  assert.equal(recipe('wrapped').build({ which: ['multicall3', 'entrypoint_v7'] }).when, 'via contains multicall3 || via contains entrypoint_v7');
  assert.match(recipe('wrapped').build({ which: [] }).error, /at least one/);
});

test('address recipes insist on a real address and explain what that is', () => {
  const a = '0x68B3465833fb72A70ecDF485E0e4C7bD8665Fc45';
  const r = recipe('address');
  assert.equal(r.build({ address: a, fn: '' }, CAT).when, 'to == ' + a.toLowerCase());
  assert.equal(r.build({ address: a, fn: '0xa415bcad' }, CAT).when, 'to == ' + a.toLowerCase() + ' && selector == 0xa415bcad');
  assert.equal(r.build({ address: a, fn: 'custom', custom: '0xA9059CBB' }, CAT).when, 'to == ' + a.toLowerCase() + ' && selector == 0xa9059cbb');
  assert.match(r.build({ address: '0x123', fn: '' }, CAT).error, /starts with 0x and has 40 more/);
  assert.match(r.build({ address: a, fn: 'custom', custom: '0x12' }, CAT).error, /8 characters/);
  assert.match(r.build({ address: a, fn: '0xa415bcad' }, CAT).name, /Aave: borrow/);
  assert.equal(recipe('sender').build({ address: a }).when, 'from == ' + a.toLowerCase());
  assert.match(recipe('sender').build({ address: 'nope' }).error, /starts with 0x/);
});

test('calls are described as sentences', () => {
  const swap = { kind: 'swap', protocol: 'unidentified', known: null, to: '0x278d858f05b94576c1e6f73285886876ff6ef8d2', selector: '0xa00597a0', via: [], fields: { 'swap.token_in': '0xaf88d065e77c8cc2239327c5edb3a432268e5831', 'swap.amount_in': '1234567890', 'swap.hops': '1', 'swap.pool': '0xc6962004f452be9203591991d15f6b388e09e8d0' } };
  assert.equal(L.rowSentence(swap, CAT), 'Sold 1,234.56 USDC in pool 0xc69620…e8d0 via unidentified');
  assert.equal(L.rowSentence({ ...swap, via: ['multicall3'], known: 'Uniswap SwapRouter02 (exact-in swaps)' }, CAT), 'Sold 1,234.56 USDC in pool 0xc69620…e8d0 via Uniswap SwapRouter02 (inside a Multicall3)');
  assert.equal(L.rowSentence({ ...swap, fields: { ...swap.fields, 'swap.token_in': '0x1111111111111111111111111111111111111111', 'swap.hops': '3' } }, CAT), 'Sold 1,234,567,890 units of 0x111111…1111 over 3 hops in pool 0xc69620…e8d0 via unidentified');
  assert.match(L.rowSentence(swap, null), /^Sold 1,234,567,890 units of 0xaf88d0…5831/, 'still readable without the catalogue');
  const liq = { kind: 'liquidity', protocol: 'uniswap_v3', known: 'Uniswap V3 NonfungiblePositionManager (mint…)', via: [], fields: { 'liq.action': 'remove', 'liq.token_id': '42' } };
  assert.equal(L.rowSentence(liq, CAT), 'Removed liquidity on position #42 via Uniswap V3 NonfungiblePositionManager');
  assert.equal(L.rowSentence({ ...liq, fields: { 'liq.action': 'add' } }, CAT), 'Added liquidity via Uniswap V3 NonfungiblePositionManager');
  assert.equal(L.rowSentence({ kind: 'liquidation', protocol: 'aave_v3', via: [], fields: { 'liquidation.borrower': '0x1234567890abcdef1234567890abcdef12345678' } }, CAT), 'Liquidation of 0x123456…5678 on Aave V3');
  assert.equal(L.rowSentence({ kind: 'undecoded_known', known: 'Aave V3 Pool (liquidationCall decoded)', selector: '0xa415bcad', via: [] }, CAT), 'Call to Aave V3 Pool · Aave: borrow (known contract, we cannot read this function yet)');
  assert.equal(L.rowSentence({ kind: 'unknown', to: '0xaf88d065e77c8cc2239327c5edb3a432268e5831', selector: '0xa9059cbb', via: ['entrypoint_v6', 'account_execute'] }, CAT), 'Call to 0xaf88d0…5831 · token transfer (unknown contract) (inside an ERC-4337 bundle, inside a smart account call)');
  assert.equal(L.rowSentence({ kind: 'unknown', to: '0xabc', selector: null, via: [] }, CAT), 'Call to 0xabc · no function (unknown contract)');
});

test('the choices of what to do with a catch are worded for people', () => {
  assert.deepEqual(L.ACTION_CHOICES.map((c) => c[0]), ['store', 'alert', 'raw', 'count']);
  for (const [, label, help] of L.ACTION_CHOICES) assert.ok(label.length < 40 && help.length > 10);
  for (const a of ['store', 'alert', 'raw', 'count', 'drop']) assert.ok(L.ACTION_LABELS[a]);
});
