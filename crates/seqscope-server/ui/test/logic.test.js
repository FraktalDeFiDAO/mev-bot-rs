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
