'use strict';
/* seqscope web UI. No framework, no network except this server. All text goes in through textContent, never innerHTML. */

// ------------------------------------------------------------------------------------------------------------
// Pure logic (unit-tested with `node --test`)

const OPS_BY_TYPE = {
  num: ['==', '!=', '>', '>=', '<', '<='],
  int: ['==', '!=', '>', '>=', '<', '<='],
  bool: ['==', '!='],
  addr: ['==', '!=', 'in'],
  sel: ['==', '!=', 'in'],
  hash: ['==', '!='],
  str: ['==', '!=', 'in', 'contains'],
  str_list: ['contains'],
  bytes: ['contains'],
};
const KEYWORDS = new Set(['true', 'false', 'and', 'or', 'not', 'in', 'contains', 'set']);

function opsFor(ty) {
  return (OPS_BY_TYPE[ty] || ['==']).concat(['set']);
}

/** A value as the rule language wants it: words bare when they can be, text quoted otherwise, numbers and hex as typed. */
function litText(value, ty) {
  const v = String(value).trim();
  if (ty === 'str' || ty === 'str_list') {
    return /^[A-Za-z_][A-Za-z0-9_]*$/.test(v) && !KEYWORDS.has(v) ? v : '"' + v.replace(/"/g, '\\"') + '"';
  }
  return v;
}

/** One condition `{field, op, value, ty}` as rule text, or null while it is incomplete. */
function condText(c) {
  if (!c || !c.field) return null;
  if (c.op === 'set') return 'set(' + c.field + ')';
  const raw = String(c.value == null ? '' : c.value).trim();
  if (raw === '') return null;
  if (c.op === 'in') {
    const parts = raw.split(',').map((s) => s.trim()).filter(Boolean);
    return parts.length ? c.field + ' in [' + parts.map((p) => litText(p, c.ty)).join(', ') + ']' : null;
  }
  return c.field + ' ' + c.op + ' ' + litText(raw, c.ty);
}

/** Conditions joined into one `when` expression (`true` when there are none). */
function buildWhen(conds, joiner) {
  const parts = (conds || []).map(condText).filter(Boolean);
  if (!parts.length) return 'true';
  if (parts.length === 1) return parts[0];
  const sep = joiner === 'or' ? ' || ' : ' && ';
  return parts.map((p) => (joiner === 'or' && p.includes(' && ') ? '(' + p + ')' : p)).join(sep);
}

function tomlString(s) {
  return '"' + String(s).replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
}

/** A `[[profile.rule]]` block. */
function tomlRule(r) {
  let out = '[[profile.rule]]\nname = ' + tomlString(r.name) + '\nwhen = ' + tomlString(r.when) + '\n';
  if (r.action && r.action !== 'store') out += 'action = ' + tomlString(r.action) + '\n';
  if (r.tag) out += 'tag = ' + tomlString(r.tag) + '\n';
  if (r.sample && Number(r.sample) > 1) out += 'sample = ' + Number(r.sample) + '\n';
  return out;
}

/** Names of the profiles in a rules file. */
function profileNames(text) {
  const names = [];
  const re = /\[\[profile\]\][^\[]*?\bname\s*=\s*"((?:[^"\\]|\\.)*)"/g;
  let m;
  while ((m = re.exec(text))) names.push(m[1].replace(/\\"/g, '"'));
  return names;
}

/** The rules text with a rule added to a profile (created if it does not exist; the file gets a version line if empty). */
function addRuleToText(text, rule, profile) {
  let t = String(text || '');
  if (!/^\s*version\s*=/m.test(t)) t = 'version = 1\n\n' + t.replace(/^\s+/, '');
  const block = tomlRule(rule);
  const head = t.split(/(?=\[\[profile\]\])/);
  for (let i = 0; i < head.length; i++) {
    const m = /^\[\[profile\]\][^\[]*?\bname\s*=\s*"((?:[^"\\]|\\.)*)"/.exec(head[i]);
    if (m && m[1].replace(/\\"/g, '"') === profile) {
      head[i] = head[i].replace(/\s*$/, '\n\n') + block;
      return head.join('');
    }
  }
  return t.replace(/\s*$/, '\n\n') + '[[profile]]\nname = ' + tomlString(profile) + '\nmode = "all"\n\n' + block;
}

function clock(ms) {
  const d = new Date(ms);
  const p = (n, w) => String(n).padStart(w || 2, '0');
  return p(d.getUTCHours()) + ':' + p(d.getUTCMinutes()) + ':' + p(d.getUTCSeconds()) + '.' + p(d.getUTCMilliseconds(), 3);
}

function shortHex(a) {
  if (!a) return '-';
  return a.length > 14 ? a.slice(0, 8) + '…' + a.slice(-4) : a;
}

/** The decoded fields worth showing in a table cell. */
function fieldSummary(row) {
  const f = row.fields || {};
  const pick = ['swap.amount_in', 'swap.pool', 'swap.token_in', 'liq.action', 'liq.token_id', 'liquidation.borrower', 'liquidation.amount'];
  return pick.filter((k) => k in f).map((k) => k.split('.').pop() + '=' + (f[k].length > 20 ? shortHex(f[k]) : f[k])).join(' ');
}

/** A rule that matches calls like this row: same protocol, destination and function. */
function whenLike(row) {
  const parts = [];
  if (row.protocol) parts.push('protocol == ' + row.protocol);
  if (row.to) parts.push('to == ' + row.to);
  if (row.selector) parts.push('selector == ' + row.selector);
  if (!parts.length) parts.push('kind == ' + row.kind);
  return parts.join(' && ');
}

/** A rule for one line of the gaps table (columns: kind, contract, destination, selector, calls). */
function whenForGap(columns, cells) {
  const at = (n) => cells[columns.indexOf(n)];
  const parts = [];
  if (at('destination')) parts.push('to == ' + at('destination'));
  if (at('selector')) parts.push('selector == ' + at('selector'));
  return parts.length ? parts.join(' && ') : 'kind == ' + at('kind');
}

function csvOf(columns, rows) {
  const q = (v) => {
    const s = v == null ? '' : String(v);
    return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s;
  };
  return [columns.map(q).join(',')].concat(rows.map((r) => r.map(q).join(','))).join('\n') + '\n';
}

function parseHash(h) {
  const s = String(h || '').replace(/^#/, '');
  const i = s.indexOf('?');
  const page = (i < 0 ? s : s.slice(0, i)) || 'live';
  return { page, params: new URLSearchParams(i < 0 ? '' : s.slice(i + 1)) };
}

function sparkPoints(values, w, h) {
  if (!values.length) return '';
  const max = Math.max(1, ...values);
  const step = values.length > 1 ? w / (values.length - 1) : 0;
  return values.map((v, i) => (i * step).toFixed(1) + ',' + (h - (v / max) * (h - 2) - 1).toFixed(1)).join(' ');
}

// --- Plain-language helpers -------------------------------------------------------------------------------------

function groupThousands(digits) {
  return String(digits).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** A raw token amount as a person reads it: ("1234567890", 6) -> "1,234.56". Exact (BigInt), no floating point. */
function humanAmount(raw, decimals) {
  const d = String(raw).replace(/[^0-9]/g, '');
  if (!d) return String(raw);
  const dec = Number(decimals) || 0;
  const padded = d.length <= dec ? '0'.repeat(dec + 1 - d.length) + d : d;
  const int = padded.slice(0, padded.length - dec).replace(/^0+(?=\d)/, '') || '0';
  const frac = padded.slice(padded.length - dec).replace(/0+$/, '');
  const keep = int === '0' ? Math.min(frac.length, 6) : Math.min(frac.length, 2);
  return groupThousands(int) + (keep ? '.' + frac.slice(0, keep) : '');
}

/** What a person typed ("10,000.5") as the raw whole number a rule needs, for a token with this many decimals. */
function parseAmount(text, decimals) {
  const t = String(text == null ? '' : text).replace(/[,_\s]/g, '');
  if (t === '') return { ok: false, error: 'Type an amount.' };
  if (!/^\d*\.?\d*$/.test(t) || t === '.') return { ok: false, error: '“' + text + '” is not an amount. Use digits, like 10000 or 0.5.' };
  const [int, frac = ''] = t.split('.');
  if (frac.length > decimals) return { ok: false, error: 'This token has only ' + decimals + ' decimal places.' };
  const raw = (BigInt(int || '0') * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals) || '0')).toString();
  return { ok: true, raw };
}

/** A raw whole number as short rule text: 10000000000 -> 10000e6. */
function rawLiteral(raw) {
  const m = /^(\d*?[1-9]?)(0+)$/.exec(String(raw));
  if (!m || m[2].length < 4 || !m[1]) return String(raw);
  return m[1] + 'e' + m[2].length;
}

const isAddress = (s) => /^0x[0-9a-fA-F]{40}$/.test(String(s || '').trim());
const isSelector = (s) => /^0x[0-9a-fA-F]{8}$/.test(String(s || '').trim());

const AAVE_FUNCS = [
  ['supply', 'Supply (lend)', '0x617ba037'],
  ['borrow', 'Borrow', '0xa415bcad'],
  ['repay', 'Repay', '0x573ade81'],
  ['withdraw', 'Withdraw', '0x69328dec'],
  ['liquidate', 'Liquidate someone', '0x00a718a9'],
];
const ACTION_LABELS = {
  store: 'keeps it',
  alert: 'keeps it and highlights it',
  raw: 'keeps it with the full call data',
  count: 'only counts it',
  drop: 'throws it away',
};
const ACTION_CHOICES = [
  ['store', 'Keep it', 'Save what it catches so I can look at it later.'],
  ['alert', 'Keep it and highlight it', 'Same, and mark it so it stands out.'],
  ['raw', 'Keep it with the full call data', 'For studying how something is called. Uses more space.'],
  ['count', 'Just count it', 'Do not save anything; only keep a tally.'],
];

const tokenBySymbol = (cat, sym) => ((cat && cat.tokens) || []).find((t) => t.symbol === sym);
const tokenByAddress = (cat, a) => ((cat && cat.tokens) || []).find((t) => t.address.toLowerCase() === String(a || '').toLowerCase());
const protocolLabel = (cat, id) => { const p = ((cat && cat.protocols) || []).find((x) => x.id === id); return p ? p.label : id; };
const functionName = (cat, sel) => { const h = String(sel || '').replace(/^0x/, '').toLowerCase(); const f = ((cat && cat.functions) || []).find((x) => x[0] === h); return f ? f[1] : null; };

/** Protocols that people ask about, in the order a person would look for them. */
const PROTOCOL_PICKS = ['uniswap_v3', 'uniswap_v4', 'uniswap_v2', 'uniswap_router', 'pancake_v3', 'balancer', 'aave_v3', 'morpho_blue', 'compound_v3', 'fluid_vaults', 'unidentified'];

/** The things a person can ask to watch. `build` turns the answers into a rule, or says what is missing. */
const RECIPES = [
  {
    id: 'big-swaps', title: 'Big swaps', blurb: 'Someone swaps more than an amount you choose.',
    fields: [
      { id: 'token', label: 'Token being sold', type: 'token', help: 'Pick the token so the amount can be in real units.' },
      { id: 'min', label: 'At least', type: 'text', placeholder: 'e.g. 10000', help: 'In that token, like you would say it: 10000 means ten thousand.' },
      { id: 'protocol', label: 'On', type: 'protocol', any: 'any exchange' },
    ],
    build(v, cat) {
      const parts = ['kind == swap'];
      let name = 'Swaps';
      if (v.protocol) parts.push('protocol == ' + v.protocol);
      if (v.token) {
        const t = tokenBySymbol(cat, v.token);
        if (!t) return { error: 'Unknown token.' };
        parts.push('swap.token_in == ' + t.address);
        name = v.token + ' swaps';
        if (String(v.min || '').trim() !== '') {
          const a = parseAmount(v.min, t.decimals);
          if (!a.ok) return { error: a.error };
          parts.push('swap.amount_in >= ' + rawLiteral(a.raw));
          name = 'Big ' + v.token + ' swaps (≥ ' + String(v.min).trim() + ')';
        }
      } else if (String(v.min || '').trim() !== '') {
        return { error: 'Pick which token the amount is in.' };
      }
      if (v.protocol) name += ' on ' + protocolLabel(cat, v.protocol);
      return { when: parts.join(' && '), name };
    },
  },
  {
    id: 'protocol', title: 'Everything on one exchange or lender', blurb: 'All activity on Uniswap, Aave, Balancer and so on.',
    fields: [
      { id: 'protocol', label: 'Which one', type: 'protocol', required: true },
      { id: 'what', label: 'What kind of activity', type: 'choice', choices: [['', 'All of it'], ['swap', 'Only swaps'], ['liquidity', 'Only liquidity added or removed'], ['liquidation', 'Only liquidations'], ['undecoded_known', 'Only things we cannot read yet']] },
    ],
    build(v, cat) {
      if (!v.protocol) return { error: 'Pick one.' };
      const parts = ['protocol == ' + v.protocol];
      if (v.what) parts.push('kind == ' + v.what);
      const what = { '': 'activity', swap: 'swaps', liquidity: 'liquidity changes', liquidation: 'liquidations', undecoded_known: 'unread calls' }[v.what || ''];
      return { when: parts.join(' && '), name: protocolLabel(cat, v.protocol) + ' ' + what };
    },
  },
  {
    id: 'aave', title: 'Aave lending activity', blurb: 'Supply, borrow, repay, withdraw and liquidations on Aave V3.',
    fields: [{ id: 'which', label: 'Which actions', type: 'checks', choices: AAVE_FUNCS.map((f) => [f[0], f[1]]), defaults: ['borrow', 'liquidate'] }],
    build(v) {
      const chosen = AAVE_FUNCS.filter((f) => (v.which || []).includes(f[0]));
      if (!chosen.length) return { error: 'Tick at least one action.' };
      const sel = chosen.length === 1 ? 'selector == ' + chosen[0][2] : 'selector in [' + chosen.map((f) => f[2]).join(', ') + ']';
      return { when: 'protocol == aave_v3 && ' + sel, name: 'Aave: ' + chosen.map((f) => f[0]).join(', ') };
    },
  },
  {
    id: 'liquidity', title: 'Liquidity added or removed', blurb: 'Someone puts money into, or pulls it out of, a trading pool.',
    fields: [
      { id: 'what', label: 'Which', type: 'choice', choices: [['both', 'Added or removed'], ['add', 'Only added'], ['remove', 'Only removed']] },
      { id: 'protocol', label: 'On', type: 'protocol', any: 'any exchange' },
    ],
    build(v, cat) {
      const parts = ['kind == liquidity'];
      if (v.what === 'add' || v.what === 'remove') parts.push('liq.action == ' + v.what);
      if (v.protocol) parts.push('protocol == ' + v.protocol);
      const w = v.what === 'add' ? 'added' : v.what === 'remove' ? 'removed' : 'added or removed';
      return { when: parts.join(' && '), name: 'Liquidity ' + w + (v.protocol ? ' on ' + protocolLabel(cat, v.protocol) : '') };
    },
  },
  {
    id: 'liquidations', title: 'Liquidations', blurb: 'A loan is being liquidated, on any lender we can read.',
    fields: [{ id: 'protocol', label: 'On', type: 'protocol', any: 'any lender' }],
    build(v, cat) {
      const parts = ['kind == liquidation'];
      if (v.protocol) parts.push('protocol == ' + v.protocol);
      return { when: parts.join(' && '), name: 'Liquidations' + (v.protocol ? ' on ' + protocolLabel(cat, v.protocol) : ''), action: 'alert' };
    },
  },
  {
    id: 'address', title: 'Calls to a contract or wallet', blurb: 'Anything sent to one address, optionally one function.',
    fields: [
      { id: 'address', label: 'Address', type: 'text', placeholder: '0x… (42 characters)', required: true },
      { id: 'fn', label: 'Function (optional)', type: 'function' },
    ],
    build(v, cat) {
      const a = String(v.address || '').trim();
      if (!isAddress(a)) return { error: 'An address starts with 0x and has 40 more characters (letters a–f and digits).' };
      const parts = ['to == ' + a.toLowerCase()];
      let fnName = '';
      if (v.fn) {
        let sel = v.fn;
        if (v.fn === 'custom') sel = String(v.custom || '').trim();
        if (!isSelector(sel)) return { error: 'A function id is 0x followed by 8 characters, like 0xa9059cbb.' };
        parts.push('selector == ' + sel.toLowerCase());
        fnName = ' · ' + (functionName(cat, sel) || sel);
      }
      return { when: parts.join(' && '), name: 'Calls to ' + shortHex(a) + fnName };
    },
  },
  {
    id: 'sender', title: 'Everything one wallet does', blurb: 'All transactions sent by one address.',
    fields: [{ id: 'address', label: 'Wallet address', type: 'text', placeholder: '0x… (42 characters)', required: true }],
    build(v) {
      const a = String(v.address || '').trim();
      if (!isAddress(a)) return { error: 'An address starts with 0x and has 40 more characters (letters a–f and digits).' };
      return { when: 'from == ' + a.toLowerCase(), name: 'Wallet ' + shortHex(a) };
    },
  },
  {
    id: 'unread', title: 'Things we cannot read yet', blurb: 'Calls the decoder does not understand: where to improve it.',
    fields: [{ id: 'what', label: 'Which', type: 'choice', choices: [['both', 'Both kinds'], ['known', 'Known contracts, unknown function'], ['unknown', 'Unknown contracts']] }],
    build(v) {
      const known = 'kind == undecoded_known', unknown = 'kind == unknown && len >= 4';
      const w = v.what || 'both';
      return { when: w === 'known' ? known : w === 'unknown' ? unknown : known + ' || (' + unknown + ')', name: 'Unread calls' + (w === 'both' ? '' : w === 'known' ? ' (known contracts)' : ' (unknown contracts)') };
    },
  },
  {
    id: 'wrapped', title: 'Calls hidden inside bundles', blurb: 'Swaps and other calls packed into a multicall, a smart account or an ERC-4337 bundle.',
    fields: [{ id: 'which', label: 'Inside', type: 'checks', choices: [['multicall3', 'Multicall3'], ['self_multicall', 'A router multicall'], ['account_execute', 'A smart-account call'], ['entrypoint_v6', 'An ERC-4337 bundle (v0.6)'], ['entrypoint_v7', 'An ERC-4337 bundle (v0.7)']], defaults: ['multicall3', 'self_multicall'] }],
    build(v) {
      const w = v.which || [];
      if (!w.length) return { error: 'Tick at least one.' };
      return { when: w.map((x) => 'via contains ' + x).join(' || '), name: 'Calls inside bundles' };
    },
  },
];

/** One call as a sentence a person can read. `cat` is the catalogue (tokens, labels); everything degrades without it. */
function rowSentence(row, cat) {
  const f = row.fields || {};
  const where = row.known ? row.known.split(' (')[0] : row.protocol ? protocolLabel(cat, row.protocol) : shortHex(row.to);
  const inside = (row.via && row.via.length) ? ' (inside ' + row.via.map((w) => ({ multicall3: 'a Multicall3', self_multicall: 'a router multicall', account_execute: 'a smart account call', entrypoint_v6: 'an ERC-4337 bundle', entrypoint_v7: 'an ERC-4337 bundle' }[w] || w)).join(', inside ') + ')' : '';
  if (row.kind === 'swap') {
    const t = tokenByAddress(cat, f['swap.token_in']);
    const amt = f['swap.amount_in'];
    const what = amt == null ? 'tokens' : t ? humanAmount(amt, t.decimals) + ' ' + t.symbol : groupThousands(amt) + ' units of ' + shortHex(f['swap.token_in'] || '?');
    const hops = Number(f['swap.hops']) > 1 ? ' over ' + f['swap.hops'] + ' hops' : '';
    const pool = f['swap.pool'] ? ' in pool ' + shortHex(f['swap.pool']) : '';
    return 'Sold ' + what + hops + pool + ' via ' + where + inside;
  }
  if (row.kind === 'liquidity') {
    const verb = f['liq.action'] === 'remove' ? 'Removed' : f['liq.action'] === 'add' ? 'Added' : 'Changed';
    const pos = f['liq.token_id'] ? ' on position #' + f['liq.token_id'] : '';
    const pool = f['liq.pool'] ? ' in pool ' + shortHex(f['liq.pool']) : '';
    return verb + ' liquidity' + pos + pool + ' via ' + where + inside;
  }
  if (row.kind === 'liquidation') {
    const b = f['liquidation.borrower'] ? ' of ' + shortHex(f['liquidation.borrower']) : '';
    return 'Liquidation' + b + ' on ' + (row.protocol ? protocolLabel(cat, row.protocol) : where) + inside;
  }
  const fn = functionName(cat, row.selector) || row.selector || 'no function';
  if (row.kind === 'undecoded_known') return 'Call to ' + where + ' · ' + fn + ' (known contract, we cannot read this function yet)' + inside;
  return 'Call to ' + shortHex(row.to) + ' · ' + fn + ' (unknown contract)' + inside;
}

const Logic = { groupThousands, humanAmount, parseAmount, rawLiteral, isAddress, isSelector, AAVE_FUNCS, ACTION_LABELS, ACTION_CHOICES, RECIPES, PROTOCOL_PICKS, rowSentence, tokenBySymbol, tokenByAddress, protocolLabel, functionName, OPS_BY_TYPE, opsFor, litText, condText, buildWhen, tomlRule, profileNames, addRuleToText, clock, shortHex, fieldSummary, whenLike, whenForGap, csvOf, parseHash, sparkPoints };
if (typeof module !== 'undefined') module.exports = Logic;

// ------------------------------------------------------------------------------------------------------------
// The interface

if (typeof document !== 'undefined') (function () {
  const $ = (sel, root) => (root || document).querySelector(sel);

  function h(tag, attrs, ...kids) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === 'class') e.className = v;
      else if (k === 'text') e.textContent = v;
      else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
      else if (k === 'value') e.value = v;
      else if (k === 'checked' || k === 'disabled' || k === 'selected') e[k] = !!v;
      else e.setAttribute(k, v === true ? '' : v);
    }
    for (const kid of kids.flat()) if (kid != null && kid !== false) e.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    return e;
  }

  /** Replace an element's children, flattening arrays and skipping null / false (the DOM's own replaceChildren does neither). */
  function fill(el, ...kids) {
    el.replaceChildren(...kids.flat(Infinity).filter((k) => k != null && k !== false).map((k) => (k.nodeType ? k : document.createTextNode(String(k)))));
    return el;
  }

  function toast(msg, bad) {
    const t = $('#toast');
    t.textContent = msg;
    t.classList.remove('hidden');
    t.style.background = bad ? 'var(--bad)' : '';
    clearTimeout(toast.t);
    toast.t = setTimeout(() => t.classList.add('hidden'), 2600);
  }

  async function api(method, path, body) {
    const opts = { method, headers: {} };
    if (method !== 'GET') {
      opts.headers['X-Seqscope'] = '1';
      opts.headers['Content-Type'] = 'application/json';
      opts.body = JSON.stringify(body || {});
    }
    const r = await fetch(path, opts);
    let data = null;
    try { data = await r.json(); } catch (_) { /* not JSON */ }
    if (!r.ok) {
      const err = new Error((data && (data.error || (data.errors && data.errors[0] && data.errors[0].message))) || r.statusText);
      err.data = data;
      err.status = r.status;
      throw err;
    }
    return data;
  }

  let catalogue = null;
  const catalogueReady = api('GET', '/api/catalogue').then((c) => (catalogue = c));

  // --- The permanent socket: stored-rule rows and collector status ---------------------------------------------
  const state = { rows: [], status: null, history: [], paused: false, listeners: new Set() };
  function openSocket(query, onMessage, onState) {
    let closed = false, ws = null, wait = 500;
    function connect() {
      ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/api/live' + query);
      ws.onopen = () => { wait = 500; onState && onState(true); };
      ws.onmessage = (e) => {
        let m;
        try { m = JSON.parse(e.data); } catch (err) { console.error('seqscope: bad frame', err); return; }
        try { onMessage(m); } catch (err) { console.error('seqscope: handler failed', err); }
      };
      ws.onclose = () => { onState && onState(false); if (!closed) setTimeout(connect, wait = Math.min(wait * 2, 8000)); };
    }
    connect();
    return { close() { closed = true; try { ws.close(); } catch (_) { /* already closed */ } } };
  }
  openSocket('', (m) => {
    if (m.type === 'status') {
      state.status = m;
      state.history.push(m.tx_per_s);
      if (state.history.length > 120) state.history.shift();
      renderStatus();
    } else if (m.type === 'row' && !state.paused) {
      state.rows.unshift(m.row);
      if (state.rows.length > 500) state.rows.length = 500;
    }
    state.listeners.forEach((f) => f(m));
  }, (up) => { if (!up) { $('#status').textContent = 'disconnected, retrying…'; $('#status').classList.add('bad'); } });

  function renderStatus() {
    const s = state.status;
    if (!s) return;
    const el = $('#status');
    el.classList.toggle('bad', s.dropped > 0 || s.failed > 0);
    el.textContent = s.tx_per_s + ' tx/s · ' + s.rows + ' rows · written ' + s.written + (s.dropped ? ' · DROPPED ' + s.dropped : '') + (s.failed ? ' · FAILED ' + s.failed : '');
  }

  // --- Shared pieces ------------------------------------------------------------------------------------------
  function badge(kind) { return h('span', { class: 'badge ' + kind, text: kind }); }

  function rowTable(rows, onPick) {
    const head = ['time', 'seq', 'kind', 'protocol', 'to', 'selector', 'via', 'decoded', 'rule'];
    return h('table', {},
      h('thead', {}, h('tr', {}, head.map((c) => h('th', { text: c })))),
      h('tbody', {}, rows.map((r) => h('tr', { class: 'click mono', onclick: () => onPick && onPick(r) },
        h('td', { text: clock(r.t_ms) }),
        h('td', { text: r.seq }),
        h('td', {}, badge(r.kind)),
        h('td', { text: r.protocol || '-' }),
        h('td', { text: shortHex(r.to), title: (r.known || '') + ' ' + (r.to || '') }),
        h('td', { text: r.selector || '-' }),
        h('td', { text: (r.via || []).join('>') }),
        h('td', { text: fieldSummary(r), title: JSON.stringify(r.fields || {}) }),
        h('td', {}, (r.alert ? h('span', { class: 'alert-dot', text: '● ' }) : null), r.profile + '/' + r.rule + (r.tag ? ' #' + r.tag : ''))))));
  }

  function resultTable(columns, rows, onCell) {
    return h('table', {},
      h('thead', {}, h('tr', {}, columns.map((c) => h('th', { text: c })))),
      h('tbody', {}, rows.map((r) => h('tr', { class: 'mono' }, r.map((v, i) => h('td', { text: v == null ? '-' : String(v), title: v == null ? '' : String(v) }, onCell ? onCell(columns[i], v, r) : null))))));
  }

  function openDrawer(row) {
    const d = $('#drawer');
    d.classList.remove('hidden');
    fill(d, 
      h('div', { class: 'row' }, h('h2', { class: 'grow', text: 'Call ' + shortHex(row.hash) }), h('button', { text: 'Close', onclick: () => d.classList.add('hidden') })),
      h('div', { class: 'kv' },
        h('div', { text: 'kind' }), h('div', {}, badge(row.kind)),
        h('div', { text: 'status' }), h('div', { text: row.status + ' (calldata is an intent; it may revert)' }),
        h('div', { text: 'transaction' }), h('div', { class: 'mono', text: row.hash }),
        h('div', { text: 'sender' }), h('div', { class: 'mono', text: row.from || '-' }),
        h('div', { text: 'sent to' }), h('div', { class: 'mono', text: row.tx_to || '-' }),
        h('div', { text: 'this call to' }), h('div', { class: 'mono', text: (row.to || '-') + (row.known ? '  (' + row.known + ')' : '') }),
        h('div', { text: 'function' }), h('div', { class: 'mono', text: row.selector || '-' }),
        h('div', { text: 'through' }), h('div', { text: (row.via || []).join(' > ') || 'called directly' }),
        h('div', { text: 'protocol' }), h('div', { text: row.protocol || '-' }),
        h('div', { text: 'rule' }), h('div', { text: row.profile + ' / ' + row.rule })),
      h('h3', { text: 'Decoded' }),
      h('div', { class: 'kv mono' }, Object.entries(row.fields || {}).flatMap(([k, v]) => [h('div', { text: k }), h('div', { text: v })])),
      h('div', { class: 'row', style: 'margin-top:12px' },
        h('button', { text: 'Copy JSON', onclick: () => navigator.clipboard && navigator.clipboard.writeText(JSON.stringify(row, null, 2)).then(() => toast('copied')) }),
        h('button', { class: 'primary', text: 'Watch for more like this', onclick: () => { d.classList.add('hidden'); location.hash = '#rules?new=' + encodeURIComponent(whenLike(row)); } })),
      h('h3', { text: 'JSON' }),
      h('pre', { text: JSON.stringify(row, null, 2) }));
  }

  function errorList(errors) {
    return h('ul', { class: 'errors' }, (errors || []).map((e) => h('li', {}, e.profile || e.rule ? h('b', { text: [e.profile, e.rule].filter(Boolean).join(' / ') + ': ' }) : null, e.message + (e.column ? ' (column ' + e.column + ')' : ''))));
  }

  // --- Shared: a sentence for a rule, a sentence for a call -----------------------------------------------------------
  let explainTimer = null;
  function explainInto(el, when, onResult) {
    clearTimeout(explainTimer);
    explainTimer = setTimeout(async () => {
      if (!when || !when.trim()) { el.textContent = ''; onResult && onResult(null); return; }
      try {
        const r = await api('POST', '/api/explain', { when });
        el.className = 'explain';
        el.textContent = r.text;
        onResult && onResult({ ok: true, text: r.text });
      } catch (e) {
        el.className = 'explain bad';
        const m = e.data && e.data.error;
        el.textContent = m ? (m.message + (m.column ? ' (at character ' + m.column + ')' : '')) : e.message;
        onResult && onResult({ ok: false });
      }
    }, 250);
  }

  function sentenceList(rows, limit) {
    return h('ul', { class: 'sentences' }, rows.slice(0, limit || 5).map((r) => h('li', {}, badge(r.kind), ' ', h('span', { text: rowSentence(r, catalogue) }))));
  }

  const technical = { on: false };
  try { technical.on = localStorage.getItem('seqscope.technical') === '1'; } catch (_) { /* no storage */ }

  function liveTable(rows, onPick) {
    const cols = technical.on ? ['time', 'kind', 'what happened', 'seq', 'protocol', 'to', 'function', 'via', 'rule'] : ['time', 'kind', 'what happened', 'rule'];
    return h('table', {},
      h('thead', {}, h('tr', {}, cols.map((c) => h('th', { text: c })))),
      h('tbody', {}, rows.map((r) => h('tr', { class: 'click', onclick: () => onPick && onPick(r) },
        h('td', { class: 'mono', text: clock(r.t_ms) }),
        h('td', {}, badge(r.kind)),
        h('td', { class: 'wrap', text: rowSentence(r, catalogue), title: JSON.stringify(r.fields || {}) }),
        technical.on ? [h('td', { class: 'mono', text: r.seq }), h('td', { text: r.protocol || '-' }), h('td', { class: 'mono', text: shortHex(r.to), title: (r.known || '') + ' ' + (r.to || '') }), h('td', { class: 'mono', text: r.selector || '-' }), h('td', { text: (r.via || []).join('>') })] : null,
        h('td', {}, (r.alert ? h('span', { class: 'alert-dot', text: '● ' }) : null), r.rule + (r.tag ? ' #' + r.tag : ''))))));
  }

  // --- Live ----------------------------------------------------------------------------------------------------
  const PROBE_IDEAS = [
    ['Swaps', 'kind == swap'],
    ['Big USDC swaps', 'kind == swap && swap.token_in == 0xaf88d065e77c8cc2239327c5edb3a432268e5831 && swap.amount_in >= 10000e6'],
    ['Liquidity changes', 'kind == liquidity'],
    ['Liquidations', 'kind == liquidation'],
    ['Aave borrows', 'protocol == aave_v3 && selector == 0xa415bcad'],
    ['Things we cannot read', 'kind == undecoded_known'],
  ];
  let cleanup = null;
  function page_live(params) {
    const probeWhen = params.get('probe');
    const body = h('div', { class: 'scroll' });
    const count = h('span', { class: 'muted' });
    const filter = h('input', { placeholder: 'search the rows shown (any word)', class: 'grow' });
    const probe = h('input', { class: 'grow mono', id: 'probe-text', placeholder: 'or type a rule here to try it on live traffic', value: probeWhen || '' });
    const probeEn = h('div', { class: 'explain' });
    const probeErr = h('div');
    const saveAs = h('button', { id: 'probe-save', text: 'Save this as a rule…', disabled: true });
    const banner = h('div');
    let probeRows = [], probeSock = null, probeOk = false;

    api('GET', '/api/rules').then((r) => {
      if (!r.rules || !r.rules.length) fill(banner, h('div', { class: 'panel note' }, h('b', { text: 'Nothing is being saved yet. ' }), 'Rules decide what gets saved. ', h('a', { href: '#rules', text: 'Pick what to watch →' })));
    }).catch(() => {});

    function shown() {
      const src = probeSock ? probeRows : state.rows;
      const f = filter.value.trim().toLowerCase();
      return f ? src.filter((r) => (rowSentence(r, catalogue) + ' ' + JSON.stringify(r)).toLowerCase().includes(f)) : src;
    }
    function paint() {
      const rows = shown().slice(0, 300);
      fill(body, rows.length ? liveTable(rows, openDrawer) : h('div', { class: 'muted', style: 'padding:14px', text: probeSock ? 'Waiting for something that matches…' : 'Nothing saved yet. Rules decide what is saved: open Rules to pick something to watch.' }));
      count.textContent = rows.length + ' shown' + (state.paused ? ' · paused' : '') + (probeSock ? ' · trying your rule (nothing is saved)' : '');
    }
    function startProbe() {
      stopProbe();
      const w = probe.value.trim();
      if (!w) { paint(); return; }
      probeRows = [];
      probeErr.replaceChildren();
      probeSock = openSocket('?mode=probe&when=' + encodeURIComponent(w), (m) => {
        if (m.type === 'row' && !state.paused) { m.history ? probeRows.push(m.row) : probeRows.unshift(m.row); if (probeRows.length > 500) probeRows.length = 500; if (m.history) probeRows.sort((a, b) => b.t_ms - a.t_ms || b.seq - a.seq); }
        if (m.type === 'error') { fill(probeErr, errorList(m.errors)); stopProbe(); }
      });
      paint();
    }
    function stopProbe() { if (probeSock) { probeSock.close(); probeSock = null; } }
    probe.addEventListener('input', () => { explainInto(probeEn, probe.value, (r) => { probeOk = !!(r && r.ok); saveAs.disabled = !probeOk; }); });
    probe.addEventListener('keydown', (e) => { if (e.key === 'Enter') startProbe(); });
    saveAs.addEventListener('click', () => { location.hash = '#rules?new=' + encodeURIComponent(probe.value.trim()); });
    filter.addEventListener('input', paint);
    const tick = setInterval(paint, 500);
    cleanup = () => { clearInterval(tick); stopProbe(); };
    if (probeWhen) { explainInto(probeEn, probeWhen, (r) => { probeOk = !!(r && r.ok); saveAs.disabled = !probeOk; }); setTimeout(startProbe, 0); }
    paint();
    return h('div', {},
      h('h2', { text: 'Live' }),
      banner,
      h('div', { class: 'panel' },
        h('div', { class: 'muted', style: 'margin-bottom:6px', text: 'Try an idea on live traffic. Nothing is saved until you make it a rule.' }),
        h('div', { class: 'chips' }, PROBE_IDEAS.map(([label, when]) => h('button', { class: 'chip', text: label, onclick: () => { probe.value = when; probe.dispatchEvent(new Event('input')); startProbe(); } }))),
        h('div', { class: 'row', style: 'margin-top:8px' }, probe, h('button', { class: 'primary', id: 'probe-run', text: 'Try it', onclick: startProbe }), saveAs, h('button', { text: 'Stop trying', onclick: () => { probe.value = ''; probeEn.textContent = ''; saveAs.disabled = true; stopProbe(); paint(); } })),
        probeEn, probeErr),
      h('div', { class: 'row', style: 'margin-bottom:8px' }, filter,
        h('button', { text: 'Pause / resume', onclick: () => { state.paused = !state.paused; paint(); } }),
        h('button', { text: 'Clear', onclick: () => { state.rows.length = 0; probeRows.length = 0; paint(); } }),
        h('label', { class: 'inline' }, h('input', { type: 'checkbox', id: 'tech-toggle', checked: technical.on, onchange: (e) => { technical.on = e.target.checked; try { localStorage.setItem('seqscope.technical', technical.on ? '1' : '0'); } catch (_) { /* no storage */ } paint(); } }), 'technical columns'), count),
      body);
  }

  // --- Rules: say what you want to watch -----------------------------------------------------------------------
  const DEFAULT_PROFILE = 'My rules';

  /** The form for one rule: new (from a recipe, or typed) or an existing one being changed. */
  function ruleEditor({ title, recipe, initial, mode, existing, onDone }) {
    const vals = Object.assign({}, initial && initial.vals);
    const out = { when: (initial && initial.when) || '', name: (initial && initial.name) || '', action: (initial && initial.action) || 'store', tag: (initial && initial.tag) || '' };
    const english = h('div', { class: 'explain' });
    const caught = h('div', { class: 'caught' });
    const err = h('div', { class: 'err', style: 'margin:6px 0' });
    const nameIn = h('input', { id: 'rule-name-in', value: out.name, placeholder: 'a short name for this rule', class: 'grow' });
    const saveBtn = h('button', { class: 'primary', id: 'rule-save', text: mode === 'edit' ? 'Save changes' : 'Save this rule', disabled: true });
    let previewTimer = null, nameTouched = !!out.name;
    const whenText = h('textarea', { rows: 3, spellcheck: 'false', class: 'mono', id: 'rule-when', placeholder: 'kind == swap && swap.amount_in > 1e18' });
    whenText.value = out.when;

    function recompute() {
      err.textContent = '';
      if (recipe) {
        const r = recipe.build(vals, catalogue);
        if (r.error) { out.when = ''; err.textContent = r.error; saveBtn.disabled = true; english.textContent = ''; caught.replaceChildren(); return; }
        out.when = r.when;
        if (!nameTouched) { out.name = r.name; nameIn.value = r.name; }
        if (r.action && !actionTouched) { out.action = r.action; syncAction(); }
      } else {
        out.when = whenText.value.trim();
      }
      if (!out.when) { saveBtn.disabled = true; english.textContent = ''; caught.replaceChildren(); return; }
      explainInto(english, out.when, (r) => { saveBtn.disabled = !(r && r.ok) || !nameIn.value.trim(); });
      clearTimeout(previewTimer);
      previewTimer = setTimeout(async () => {
        try {
          const p = await api('POST', '/api/preview', { when: out.when, samples: 5 });
          fill(caught, 
            h('div', { id: 'preview-summary', class: p.matched ? 'ok' : 'muted', text: p.matched ? 'It would have caught ' + p.matched + ' call' + (p.matched === 1 ? '' : 's') + ' in the last ' + p.transactions + ' transactions (out of ' + p.calls + ' calls). Examples:' : 'It would have caught nothing in the last ' + p.transactions + ' transactions. That can be right for something rare, or the rule may be too narrow.' }),
            p.samples.length ? sentenceList(p.samples, 5) : null);
        } catch (e) { caught.replaceChildren(); }
      }, 350);
    }

    // What to do when it catches something.
    let actionTouched = false;
    const actionBox = h('div', { class: 'choices' });
    function syncAction() {
      fill(actionBox, ...ACTION_CHOICES.map(([id, label, help]) => h('label', { class: 'choice' }, h('input', { type: 'radio', name: 'action', value: id, checked: out.action === id, onchange: () => { out.action = id; actionTouched = true; } }), h('span', {}, h('b', { text: label }), h('span', { class: 'muted', text: ' — ' + help })))));
    }
    syncAction();

    // The recipe's own questions.
    const fieldBox = h('div', { class: 'fields' });
    if (recipe) {
      for (const f of recipe.fields) {
        let input;
        const set = (v) => { vals[f.id] = v; recompute(); };
        if (f.type === 'token') {
          input = h('select', { id: 'f-' + f.id, onchange: (e) => set(e.target.value) }, h('option', { value: '', text: 'any token' }), ((catalogue && catalogue.tokens) || []).map((t) => h('option', { value: t.symbol, text: t.symbol, selected: vals[f.id] === t.symbol })));
        } else if (f.type === 'protocol') {
          const ids = PROTOCOL_PICKS.filter((id) => ((catalogue && catalogue.protocols) || []).some((p) => p.id === id));
          input = h('select', { id: 'f-' + f.id, onchange: (e) => set(e.target.value) }, h('option', { value: '', text: f.required ? 'choose…' : (f.any || 'any') }), ids.map((id) => h('option', { value: id, text: protocolLabel(catalogue, id), selected: vals[f.id] === id })));
        } else if (f.type === 'choice') {
          if (vals[f.id] === undefined) vals[f.id] = f.choices[0][0];
          input = h('select', { id: 'f-' + f.id, onchange: (e) => set(e.target.value) }, f.choices.map(([v, l]) => h('option', { value: v, text: l, selected: vals[f.id] === v })));
        } else if (f.type === 'checks') {
          if (vals[f.id] === undefined) vals[f.id] = (f.defaults || []).slice();
          input = h('div', { class: 'checks' }, f.choices.map(([v, l]) => h('label', { class: 'inline' }, h('input', { type: 'checkbox', value: v, checked: vals[f.id].includes(v), onchange: (e) => { const cur = new Set(vals[f.id]); e.target.checked ? cur.add(v) : cur.delete(v); set([...cur]); } }), l)));
        } else if (f.type === 'function') {
          const custom = h('input', { class: 'mono', placeholder: '0x + 8 characters', style: 'display:none', oninput: (e) => { vals.custom = e.target.value; recompute(); } });
          const sel = h('select', { id: 'f-' + f.id, onchange: (e) => { custom.style.display = e.target.value === 'custom' ? '' : 'none'; set(e.target.value); } },
            h('option', { value: '', text: 'any function' }),
            ((catalogue && catalogue.functions) || []).map(([hex, name]) => h('option', { value: '0x' + hex, text: name })),
            h('option', { value: 'custom', text: 'another function (type its id)…' }));
          input = h('div', { class: 'row' }, sel, custom);
        } else {
          input = h('input', { id: 'f-' + f.id, class: f.id === 'address' ? 'mono grow' : '', placeholder: f.placeholder || '', value: vals[f.id] || '', oninput: (e) => set(e.target.value) });
        }
        fieldBox.append(h('div', { class: 'field' }, h('label', { for: 'f-' + f.id, text: f.label }), input, f.help ? h('div', { class: 'muted help', text: f.help }) : null));
      }
    }
    nameIn.addEventListener('input', () => { nameTouched = true; out.name = nameIn.value; saveBtn.disabled = !nameIn.value.trim() || !out.when; });
    whenText.addEventListener('input', recompute);

    saveBtn.addEventListener('click', async () => {
      saveBtn.disabled = true;
      const name = nameIn.value.trim();
      const tag = out.tag;
      try {
        if (mode === 'edit') {
          await api('POST', '/api/rules/op', { op: 'update_rule', profile: existing.profile, rule: existing.name, patch: { name, when: out.when, action: out.action, tag } });
          toast('Changes saved.');
        } else {
          await api('POST', '/api/rules/op', { op: 'add_rule', profile: DEFAULT_PROFILE, rule: { name, when: out.when, action: out.action, tag: tag || null } });
          toast('Saved. It is catching things now.');
        }
        onDone && onDone(name);
      } catch (e) { err.textContent = e.message; saveBtn.disabled = false; }
    });

    const panel = h('div', { class: 'panel editor', id: 'rule-editor' },
      h('div', { class: 'row' }, h('h3', { class: 'grow', style: 'margin:0', text: title }), h('button', { id: 'rule-cancel', text: 'Cancel', onclick: () => onDone && onDone(null) })),
      fieldBox,
      recipe ? null : h('div', { class: 'field' }, h('label', { for: 'rule-when', text: 'What it catches (the rule)' }), whenText, h('div', { class: 'muted help', text: 'Written in the rule language. Everything you type is explained in words just below.' })),
      h('div', { class: 'explain-box' }, h('div', { class: 'muted small', text: 'This rule catches…' }), english),
      err,
      caught,
      h('h4', { text: 'When it catches something' }), actionBox,
      h('div', { class: 'row', style: 'margin-top:10px' }, h('label', { for: 'rule-name-in', text: 'Name' }), nameIn, saveBtn));
    recompute();
    return panel;
  }

  function page_rules(params) {
    const root = h('div');
    const editorSlot = h('div');
    const listBox = h('div', { id: 'my-rules' });
    const startersBox = h('div', { class: 'cards3' });
    let advanced = null;

    async function refresh(flash) {
      let data;
      try { data = await api('GET', '/api/rules'); } catch (e) { fill(listBox, h('div', { class: 'err', text: e.message })); return; }
      const rules = data.rules || [];
      const names = new Set(rules.map((r) => r.name));
      fill(listBox, 
        data.errors && data.errors.length ? h('div', { class: 'panel warnbox' }, h('b', { text: 'The rules file has a mistake, so the last good rules are still running. ' }), 'Fix it under “Advanced” below.', errorList(data.errors)) : null,
        rules.length ? rules.map((r) => ruleCard(r, flash === r.name, refresh)) : h('div', { class: 'panel note', id: 'no-rules' }, h('b', { text: 'You have no rules yet. ' }), 'Pick something above to watch, or add a starter set.'));
      document.dispatchEvent(new CustomEvent('seqscope:rules', { detail: { names: [...names] } }));
    }
    function ruleCard(r, flash, after) {
      const sw = h('input', { type: 'checkbox', checked: r.enabled, class: 'switch', title: r.enabled ? 'On: click to turn off' : 'Off: click to turn on', 'aria-label': 'turn ' + r.name + (r.enabled ? ' off' : ' on'),
        onchange: async (e) => { try { await api('POST', '/api/rules/op', { op: 'set_enabled', profile: r.profile, rule: r.name, enabled: e.target.checked }); toast(e.target.checked ? r.name + ' is on.' : r.name + ' is off.'); after(); } catch (er) { toast(er.message, true); after(); } } });
      const confirmBox = h('span');
      const card = h('div', { class: 'rulecard' + (r.enabled ? '' : ' off') + (flash ? ' flash' : ''), 'data-rule': r.name },
        h('div', { class: 'row' },
          sw, h('b', { class: 'grow', text: r.name }),
          h('span', { class: 'badge-action', text: ACTION_LABELS[r.action] || r.action }),
          h('button', { text: 'Edit', onclick: () => openEditor(r) }),
          h('button', { text: 'Copy', onclick: async () => { let n = r.name + ' (copy)', i = 2; const taken = new Set([...document.querySelectorAll('.rulecard')].map((c) => c.dataset.rule)); while (taken.has(n)) n = r.name + ' (copy ' + i++ + ')'; try { await api('POST', '/api/rules/op', { op: 'add_rule', profile: r.profile, rule: { name: n, when: r.when, action: r.action, tag: r.tag } }); toast('Copied.'); after(n); } catch (er) { toast(er.message, true); } } }),
          confirmBox, h('button', { class: 'danger', text: 'Delete', onclick: () => { fill(confirmBox, h('span', { class: 'muted', text: 'Delete it for good? ' }), h('button', { class: 'danger', text: 'Yes, delete', onclick: async () => { try { await api('POST', '/api/rules/op', { op: 'remove_rule', profile: r.profile, rule: r.name }); toast('Deleted.'); after(); } catch (er) { toast(er.message, true); } } }), h('button', { text: 'No', onclick: () => confirmBox.replaceChildren() })); } })),
        h('div', { class: 'explain', text: r.explain }),
        h('div', { class: 'muted small', text: r.enabled ? 'Caught ' + r.matches + ' since the collector started · ' + r.stored + ' saved in total' + (r.tag ? ' · tag “' + r.tag + '”' : '') : 'Switched off. It is not catching anything.' }));
      return card;
    }
    function closeEditor(name) { editorSlot.replaceChildren(); if (name !== null) refresh(name); if (location.hash.includes('new=') || location.hash.includes('recipe=')) history.replaceState(null, '', '#rules'); }
    function openEditor(existing) {
      fill(editorSlot, ruleEditor({ title: 'Change “' + existing.name + '”', mode: 'edit', existing, initial: { when: existing.when, name: existing.name, action: existing.action, tag: existing.tag || '' }, onDone: closeEditor }));
      editorSlot.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
    function openRecipe(r, vals) {
      fill(editorSlot, ruleEditor({ title: r.title, recipe: r, initial: { vals }, mode: 'new', onDone: closeEditor }));
      editorSlot.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
    function openCustom(when) {
      fill(editorSlot, ruleEditor({ title: 'A rule of your own', mode: 'new', initial: { when: when || '', name: '' }, onDone: closeEditor }));
      editorSlot.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }

    const STARTER_TITLES = {
      'aave-lending': ['Aave lending activity', 'Supply, borrow, repay, withdraw and liquidations on Aave V3, one rule for each.'],
      'dex-swaps': ['All swaps, big ones highlighted', 'Every swap we can read, and the very large ones marked.'],
      liquidity: ['Liquidity added and removed', 'Money going into or out of trading pools.'],
      liquidations: ['Liquidations', 'Every liquidation we can read, highlighted, with the full call kept for study.'],
      gaps: ['Things we cannot read yet', 'Calls to known contracts we do not decode, and busy unknown ones: where to improve.'],
      everything: ['A sample of everything', 'One call in a hundred, to look around.'],
    };
    async function loadStarters() {
      try {
        const list = await api('GET', '/api/templates');
        fill(startersBox, ...list.map((t) => h('div', { class: 'card starter', 'data-starter': t.name },
          h('b', { text: (STARTER_TITLES[t.name] || [t.description])[0] }),
          h('div', { class: 'muted small', text: (STARTER_TITLES[t.name] || ['', ''])[1] }),
          h('details', {}, h('summary', { class: 'muted small', text: t.rules.length + ' rule' + (t.rules.length === 1 ? '' : 's') + ' inside' }), h('ul', { class: 'small' }, t.rules.map((x) => h('li', {}, h('b', { text: x.name + ': ' }), x.explain.replace(/^Catches a call when /, 'when ').replace(/\.$/, ''))))),
          t.new === 0 ? h('div', { class: 'ok small', text: '✓ already added' }) : h('button', { text: 'Add ' + t.new + ' rule' + (t.new === 1 ? '' : 's'), onclick: async () => { try { const r = await api('POST', '/api/rules/op', { op: 'add_template', name: t.name }); toast('Added ' + r.added + ' rule' + (r.added === 1 ? '' : 's') + '.'); refresh(); loadStarters(); } catch (e) { toast(e.message, true); } } }))));
      } catch (e) { fill(startersBox, h('div', { class: 'err', text: e.message })); }
    }

    cleanup = () => { if (advanced) advanced.dispose(); };
    catalogueReady.then(() => {
      const recipeCards = h('div', { class: 'cards3' }, RECIPES.map((r) => h('button', { class: 'card recipe', 'data-recipe': r.id, onclick: () => openRecipe(r) }, h('b', { text: r.title }), h('span', { class: 'muted', text: r.blurb }))));
      advanced = advancedEditor(params);
      const adv = h('details', { id: 'advanced', class: 'panel' }, h('summary', { text: 'Advanced: edit the rules file as text, or build a rule by hand' }), advanced.el);
      fill(root, 
        h('h2', { text: 'Rules' }),
        h('p', { class: 'lead', text: 'A rule says what to catch. Anything a rule catches is saved so you can look through it later. Pick something to watch:' }),
        recipeCards,
        h('div', { class: 'row', style: 'margin-top:8px' }, h('button', { id: 'custom-rule', text: 'Something else: write my own rule…', onclick: () => openCustom('') })),
        editorSlot,
        h('h3', { text: 'My rules' }), listBox,
        h('h3', { text: 'Starter sets' }), h('p', { class: 'muted', text: 'Ready-made groups of rules. Adding one never changes what you already have.' }), startersBox,
        adv);
      refresh(); loadStarters();
      const nw = params.get('new');
      if (nw !== null) openCustom(nw);
      const rc = params.get('recipe');
      if (rc) { const r = RECIPES.find((x) => x.id === rc); if (r) openRecipe(r); }
    });
    return root;
  }

  // --- Rules ---------------------------------------------------------------------------------------------------
  function advancedEditor(params) {
    const editor = h('textarea', { rows: 22, spellcheck: 'false', id: 'rules-text' });
    const msgs = h('div');
    const saveBtn = h('button', { class: 'primary', text: 'Save', disabled: true, id: 'rules-save' });
    const info = h('span', { class: 'muted' });
    const summary = h('div');
    let saved = '', timer = null, valid = false;

    async function validate() {
      try {
        const r = await api('POST', '/api/rules/validate', { text: editor.value });
        valid = r.ok;
        fill(msgs, r.ok ? h('div', { class: 'ok', text: '✓ rules are valid' }) : errorList(r.errors));
        fill(summary, ...(r.ok ? r.profiles.map((p) => h('div', { class: 'panel' }, h('b', { text: p.name }), ' ', h('span', { class: 'muted', text: p.mode + (p.enabled ? '' : ' · disabled') }),
          h('table', {}, h('tbody', {}, p.rules.map((x) => h('tr', { class: 'mono' }, h('td', { text: x.name }), h('td', { text: x.action }), h('td', { text: x.when, title: x.when }))))))) : []));
      } catch (e) { valid = false; fill(msgs, h('div', { class: 'err', text: e.message })); }
      saveBtn.disabled = !valid || editor.value === saved;
      info.textContent = editor.value === saved ? '' : 'unsaved changes';
    }
    editor.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(validate, 300); saveBtn.disabled = true; });
    saveBtn.addEventListener('click', async () => {
      try { await api('PUT', '/api/rules', { text: editor.value }); saved = editor.value; toast('saved; the running rules switched'); validate(); }
      catch (e) { fill(msgs, errorList((e.data && e.data.errors) || [{ message: e.message }])); toast('not saved', true); }
    });
    async function load() {
      const r = await api('GET', '/api/rules');
      saved = r.text; editor.value = r.text; $('#rules-path').textContent = r.path + (r.exists ? '' : ' (does not exist yet; saving creates it)');
      if (!r.text) editor.value = '';
      validate();
    }

    // --- The builder ---
    const conds = [];
    const condBox = h('div');
    const expr = h('textarea', { rows: 3, spellcheck: 'false', class: 'mono', id: 'when-text', placeholder: 'kind == swap && swap.amount_in >= 1e18' });
    const joiner = h('select', { id: 'joiner' }, h('option', { value: 'and', text: 'all of these (AND)' }), h('option', { value: 'or', text: 'any of these (OR)' }));
    const result = h('div');
    const profileSel = h('select', { id: 'profile-sel' });
    const ruleName = h('input', { placeholder: 'rule name', id: 'rule-name' });
    const actionSel = h('select', {}, ['store', 'raw', 'alert', 'count', 'drop'].map((a) => h('option', { value: a, text: a })));
    const tagIn = h('input', { placeholder: 'tag (optional)', size: 14 });
    let probeSock = null;

    function fieldsByGroup() {
      const g = {};
      (catalogue ? catalogue.fields : []).forEach((f) => (g[f.group] = g[f.group] || []).push(f));
      return g;
    }
    function condRow(c) {
      const field = h('select', {}, Object.entries(fieldsByGroup()).map(([g, fs]) => h('optgroup', { label: g }, fs.map((f) => h('option', { value: f.name, text: f.name, title: f.doc, selected: f.name === c.field })))));
      const op = h('select', {});
      const val = h('input', { class: 'mono', placeholder: 'value', list: 'dl-' + conds.indexOf(c) });
      const dl = h('datalist', { id: 'dl-' + conds.indexOf(c) });
      function refresh(resetOp) {
        const f = (catalogue.fields.find((x) => x.name === field.value));
        c.field = f.name; c.ty = f.ty;
        const ops = opsFor(f.ty);
        if (resetOp || !ops.includes(c.op)) c.op = ops[0];
        fill(op, ...ops.map((o) => h('option', { value: o, text: o, selected: o === c.op })));
        fill(dl, ...(f.values || []).map((v) => h('option', { value: v })));
        val.disabled = c.op === 'set';
        val.placeholder = f.ty === 'addr' ? '0x… (20 bytes)' : f.ty === 'sel' ? '0x… (4 bytes)' : f.ty === 'num' ? 'whole number, e.g. 5e6' : f.doc.slice(0, 40);
        sync();
      }
      field.addEventListener('change', () => refresh(true));
      op.addEventListener('change', () => { c.op = op.value; val.disabled = c.op === 'set'; sync(); });
      val.addEventListener('input', () => { c.value = val.value; sync(); });
      val.value = c.value || '';
      refresh(false);
      return h('div', { class: 'cond' }, field, op, h('div', {}, val, dl), h('button', { text: '✕', title: 'remove', onclick: () => { conds.splice(conds.indexOf(c), 1); drawConds(); sync(); } }));
    }
    function drawConds() { fill(condBox, ...conds.map(condRow)); }
    function sync() { expr.value = buildWhen(conds, joiner.value); }
    joiner.addEventListener('change', sync);

    async function dryRun() {
      fill(result, h('span', { class: 'muted', text: 'running on recent traffic…' }));
      try {
        const p = await api('POST', '/api/preview', { when: expr.value, samples: 8 });
        fill(result, 
          h('div', { id: 'preview-summary', text: p.matched + ' of ' + p.calls + ' calls match (' + p.transactions + ' recent transactions)' }),
          p.samples.length ? h('div', { class: 'scroll', style: 'margin-top:8px' }, rowTable(p.samples, openDrawer)) : null);
      } catch (e) { fill(result, errorList((e.data && e.data.errors) || [{ message: e.message }])); }
    }
    function watch() {
      if (probeSock) { probeSock.close(); probeSock = null; }
      const rows = [];
      const box = h('div', { class: 'scroll', style: 'margin-top:8px' });
      fill(result, h('div', { class: 'muted', text: 'watching live; matches appear below' }), box, h('button', { text: 'Stop', onclick: () => { probeSock.close(); probeSock = null; } }));
      probeSock = openSocket('?mode=probe&when=' + encodeURIComponent(expr.value), (m) => {
        if (m.type === 'row') { rows.unshift(m.row); rows.length = Math.min(rows.length, 60); fill(box, rowTable(rows, openDrawer)); }
        if (m.type === 'error') { fill(result, errorList(m.errors)); probeSock.close(); probeSock = null; }
      });
    }
    function addToFile() {
      const name = ruleName.value.trim() || 'new rule';
      editor.value = addRuleToText(editor.value, { name, when: expr.value, action: actionSel.value, tag: tagIn.value.trim() }, profileSel.value || 'rules');
      editor.dispatchEvent(new Event('input'));
      toast('added to the editor; check it, then Save');
      editor.scrollTop = editor.scrollHeight;
    }
    function refreshProfiles() {
      const names = profileNames(editor.value);
      fill(profileSel, ...(names.length ? names : ['rules']).map((n) => h('option', { value: n, text: n })), h('option', { value: '', text: '(new profile “rules”)' }));
    }
    editor.addEventListener('input', refreshProfiles);

    const dispose = () => { if (probeSock) probeSock.close(); };
    catalogueReady.then(async () => {
      await load();
      refreshProfiles();
      const w = params.get('when');
      if (w) { expr.value = w; }
      else { conds.push({ field: 'kind', op: '==', value: 'swap' }); drawConds(); sync(); }
      if (w && params.get('preview')) dryRun();
    });
    const el = h('div', {},
      h('div', { class: 'grid2' },
        h('div', {},
          h('div', { class: 'panel' },
            h('div', { class: 'row' }, h('b', { class: 'grow', text: 'Rules file' }), info, h('button', { text: 'Reload from disk', onclick: load }), saveBtn),
            h('div', { class: 'muted mono', id: 'rules-path', style: 'font-size:12px;margin:4px 0' }),
            editor, msgs),
          summary),
        h('div', {},
          h('div', { class: 'panel' },
            h('b', { text: 'Try a rule' }), h('span', { class: 'muted', text: '  build it, test it on recent traffic, then add it to the file' }),
            h('h3', { text: 'Conditions' }),
            condBox,
            h('div', { class: 'row' }, h('button', { text: '+ condition', onclick: () => { conds.push({ field: 'kind', op: '==', value: '' }); drawConds(); sync(); } }), joiner),
            h('h3', { text: 'The rule as text (editable)' }), expr,
            h('div', { class: 'row', style: 'margin-top:8px' },
              h('button', { class: 'primary', id: 'dry-run', text: 'Dry run on recent traffic', onclick: dryRun }),
              h('button', { text: 'Watch live', onclick: watch })),
            result),
          h('div', { class: 'panel' },
            h('b', { text: 'Add to the rules file' }),
            h('div', { class: 'row', style: 'margin-top:8px' }, ruleName, profileSel, actionSel, tagIn, h('button', { id: 'add-rule', text: 'Add', onclick: addToFile })),
            h('div', { class: 'muted', style: 'margin-top:6px;font-size:12px', text: 'store keeps a row; raw keeps the calldata too; alert flags it; count only counts; drop discards the call for this profile.' })))));
    return { el, dispose };
  }

  // --- Explore -------------------------------------------------------------------------------------------------
  const SQL_EXAMPLES = [
    ['rows per kind and protocol', "SELECT kind, protocol, count(*) AS rows FROM rows GROUP BY 1,2 ORDER BY 3 DESC"],
    ['busiest destinations', "SELECT to_addr, known, count(*) AS calls FROM v_rows GROUP BY 1,2 ORDER BY 3 DESC LIMIT 25"],
    ['largest swaps', "SELECT t_ms, to_addr, json_extract(fields,'$.\"swap.amount_in\"') AS amount_in FROM v_rows WHERE kind='swap' ORDER BY length(amount_in) DESC, amount_in DESC LIMIT 20"],
    ['calls per minute', "SELECT t_ms/60000 AS minute, count(*) AS calls FROM rows GROUP BY 1 ORDER BY 1 DESC LIMIT 60"],
    ['wrapped calls by wrapper', "SELECT via, count(*) AS calls FROM rows WHERE via != '' GROUP BY 1 ORDER BY 2 DESC"],
  ];
  function page_explore(params) {
    const f = {};
    const field = (name, ph) => (f[name] = h('input', { placeholder: ph || name, size: 14, value: params.get(name) || '' }));
    const out = h('div');
    const sql = h('textarea', { rows: 4, spellcheck: 'false', id: 'sql-text', value: params.get('sql') || SQL_EXAMPLES[0][1] });
    const ex = h('select', { onchange: (e) => { if (e.target.value) sql.value = e.target.value; } }, h('option', { value: '', text: 'examples…' }), SQL_EXAMPLES.map(([n, q]) => h('option', { value: q, text: n })));
    let last = null;
    async function runRows() {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(f)) if (v.value.trim()) q.set(k, v.value.trim());
      if (alerts.checked) q.set('alerts', 'true');
      try { const rows = await api('GET', '/api/rows?' + q); last = null; fill(out, h('div', { class: 'muted', text: rows.length + ' rows, newest first' }), h('div', { class: 'scroll' }, rowTable(rows, openDrawer))); }
      catch (e) { fill(out, h('div', { class: 'err', text: e.message })); }
    }
    async function runSql() {
      try {
        const r = await api('POST', '/api/sql', { sql: sql.value, max_rows: 2000 });
        last = r;
        fill(out, h('div', { class: 'row' }, h('span', { class: 'muted', id: 'sql-count', text: r.rows.length + ' rows' + (r.truncated ? ' (cut)' : '') }),
          h('button', { text: 'Download CSV', onclick: () => download('seqscope.csv', csvOf(r.columns, r.rows), 'text/csv') }),
          h('button', { text: 'Download JSON', onclick: () => download('seqscope.json', JSON.stringify(r), 'application/json') })),
          h('div', { class: 'scroll' }, resultTable(r.columns, r.rows)));
      } catch (e) { fill(out, h('div', { class: 'err', id: 'sql-error', text: e.message })); }
    }
    const alerts = h('input', { type: 'checkbox' });
    sql.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) runSql(); });
    if (params.get('run') === 'sql') setTimeout(runSql, 0);
    if (params.get('run') === 'rows') setTimeout(runRows, 0);
    return h('div', {},
      h('h2', { text: 'Explore' }),
      h('div', { class: 'panel' }, h('b', { text: 'Filter stored rows' }),
        h('div', { class: 'row', style: 'margin-top:8px' }, field('kind'), field('protocol'), field('selector', 'selector 0x…'), field('to', 'destination 0x…'), field('rule'), field('tag'), field('minutes', 'last N minutes'), field('limit', 'limit'),
          h('label', { class: 'inline' }, alerts, 'alerts only'), h('button', { class: 'primary', id: 'run-rows', text: 'Show', onclick: runRows }))),
      h('div', { class: 'panel' }, h('b', { text: 'SQL' }), h('span', { class: 'muted', text: '  read-only; tables rows and v_rows (text addresses); Ctrl+Enter runs' }),
        h('div', { class: 'row', style: 'margin:8px 0' }, ex, h('button', { class: 'primary', id: 'run-sql', text: 'Run', onclick: runSql })), sql),
      out);
  }
  function download(name, text, type) {
    const a = h('a', { href: URL.createObjectURL(new Blob([text], { type })), download: name });
    document.body.append(a); a.click(); a.remove();
  }

  // --- Gaps ----------------------------------------------------------------------------------------------------
  function page_gaps() {
    const out = h('div', { text: 'loading…' });
    api('GET', '/api/gaps?limit=60').then((r) => {
      if (!r.rows.length) { fill(out, h('div', { class: 'muted', text: 'No undecoded calls stored. Store them with a rule such as: kind == undecoded_known || (kind == unknown && len >= 4)' })); return; }
      fill(out, h('div', { class: 'scroll' }, resultTable(r.columns, r.rows, (col, v, row) => (col === 'calls' ? h('button', { style: 'margin-left:8px', text: 'watch this', onclick: () => { location.hash = '#rules?new=' + encodeURIComponent(whenForGap(r.columns, row)); } }) : null))));
    }).catch((e) => fill(out, h('div', { class: 'err', text: e.message })));
    return h('div', {}, h('h2', { text: 'Gaps' }), h('div', { class: 'muted', style: 'margin-bottom:8px', text: 'Calls the decoder could not read, busiest first. “undecoded_known” are identified contracts with unread functions; fixing those pays most.' }), out);
  }

  // --- Stats ---------------------------------------------------------------------------------------------------
  function page_stats() {
    const out = h('div', { text: 'loading…' });
    async function load() {
      const s = await api('GET', '/api/stats');
      const c = s.collector, d = s.db;
      const card = (label, n, cls) => h('div', { class: 'card' }, h('div', { class: 'muted', text: label }), h('div', { class: 'n ' + (cls || ''), text: n }));
      const max = Math.max(1, ...s.by_kind.rows.map((r) => r[2]));
      fill(out, 
        h('div', { class: 'cards' },
          card('stored rows', d.rows), card('database', (d.bytes / 1e6).toFixed(1) + ' MB'), card('transactions seen', c.txs), card('tx / s now', c.tx_per_s),
          card('rows written', c.written), card('dropped', c.dropped, c.dropped ? 'err' : 'ok'), card('failed', c.failed, c.failed ? 'err' : 'ok'), card('uptime', c.uptime_s + ' s')),
        h('div', { class: 'panel' }, h('b', { text: 'tx / s, last ' + state.history.length + ' s' }),
          h('div', {}, (() => { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 600 60'); svg.setAttribute('width', '100%'); svg.setAttribute('height', '60'); const pl = document.createElementNS('http://www.w3.org/2000/svg', 'polyline'); pl.setAttribute('points', sparkPoints(state.history, 600, 60)); pl.setAttribute('fill', 'none'); pl.setAttribute('stroke', 'currentColor'); pl.setAttribute('stroke-width', '2'); svg.append(pl); return svg; })())),
        h('div', { class: 'grid2' },
          h('div', { class: 'panel' }, h('b', { text: 'stored rows by kind and protocol' }),
            h('table', {}, h('tbody', {}, s.by_kind.rows.map((r) => h('tr', {}, h('td', {}, badge(r[0])), h('td', { text: r[1] }), h('td', { text: r[2], class: 'mono' }), h('td', { style: 'width:40%' }, h('div', { class: 'bar', style: 'width:' + Math.round((r[2] / max) * 100) + '%' }))))))),
          h('div', { class: 'panel' }, h('b', { text: 'matches per rule (since start)' }),
            h('table', {}, h('tbody', {}, (c.rules || []).map((r) => h('tr', { class: 'mono' }, h('td', { text: r.profile + ' / ' + r.rule }), h('td', { text: r.matches })))))),
          h('div', { class: 'panel' }, h('b', { text: 'stored rows per rule' }),
            h('table', {}, h('tbody', {}, s.by_rule.rows.map((r) => h('tr', { class: 'mono' }, h('td', { text: r[0] + ' / ' + r[1] }), h('td', { text: r[2] }))))))));
    }
    load().catch((e) => fill(out, h('div', { class: 'err', text: e.message })));
    const t = setInterval(() => load().catch(() => {}), 3000);
    cleanup = () => clearInterval(t);
    return h('div', {}, h('h2', { text: 'Stats' }), out);
  }

  // --- Router --------------------------------------------------------------------------------------------------
  const pages = { live: page_live, rules: page_rules, explore: page_explore, gaps: page_gaps, stats: page_stats };
  async function route() {
    if (cleanup) { try { cleanup(); } catch (_) { /* page already gone */ } cleanup = null; }
    const { page, params } = parseHash(location.hash);
    const p = pages[page] ? page : 'live';
    document.querySelectorAll('#nav a').forEach((a) => a.classList.toggle('on', a.dataset.page === p));
    $('#drawer').classList.add('hidden');
    await catalogueReady.catch(() => {});
    fill($('#main'), pages[p](params));
    document.title = 'seqscope · ' + p;
  }
  window.addEventListener('hashchange', route);
  route();
})();
