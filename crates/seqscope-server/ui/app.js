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

const Logic = { OPS_BY_TYPE, opsFor, litText, condText, buildWhen, tomlRule, profileNames, addRuleToText, clock, shortHex, fieldSummary, whenLike, whenForGap, csvOf, parseHash, sparkPoints };
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
    d.replaceChildren(
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
        h('button', { class: 'primary', text: 'Make a rule like this', onclick: () => { d.classList.add('hidden'); location.hash = '#rules?when=' + encodeURIComponent(whenLike(row)); } })),
      h('h3', { text: 'JSON' }),
      h('pre', { text: JSON.stringify(row, null, 2) }));
  }

  function errorList(errors) {
    return h('ul', { class: 'errors' }, (errors || []).map((e) => h('li', {}, e.profile || e.rule ? h('b', { text: [e.profile, e.rule].filter(Boolean).join(' / ') + ': ' }) : null, e.message + (e.column ? ' (column ' + e.column + ')' : ''))));
  }

  // --- Live ----------------------------------------------------------------------------------------------------
  let cleanup = null;
  function page_live(params) {
    const probeWhen = params.get('probe');
    const body = h('div', { class: 'scroll' });
    const count = h('span', { class: 'muted' });
    const filter = h('input', { placeholder: 'filter shown rows (text)', class: 'grow' });
    const probe = h('input', { class: 'grow mono', placeholder: 'probe: try a rule on live traffic, e.g. kind == swap && swap.amount_in > 1e18', value: probeWhen || '' });
    const probeErr = h('div');
    let probeRows = [], probeSock = null;

    function shown() {
      const src = probeSock ? probeRows : state.rows;
      const f = filter.value.trim().toLowerCase();
      return f ? src.filter((r) => JSON.stringify(r).toLowerCase().includes(f)) : src;
    }
    function paint() {
      const rows = shown().slice(0, 300);
      body.replaceChildren(rows.length ? rowTable(rows, openDrawer) : h('div', { class: 'muted', style: 'padding:14px', text: probeSock ? 'Waiting for a match…' : 'No stored rows yet. Rules decide what is stored; try a probe above, or open Rules.' }));
      count.textContent = rows.length + ' shown' + (state.paused ? ' · paused' : '') + (probeSock ? ' · probing' : '');
    }
    function startProbe() {
      stopProbe();
      const w = probe.value.trim();
      if (!w) { paint(); return; }
      probeRows = [];
      probeErr.replaceChildren();
      probeSock = openSocket('?mode=probe&when=' + encodeURIComponent(w), (m) => {
        if (m.type === 'row' && !state.paused) { m.history ? probeRows.push(m.row) : probeRows.unshift(m.row); if (probeRows.length > 500) probeRows.length = 500; if (m.history) probeRows.sort((a, b) => b.t_ms - a.t_ms || b.seq - a.seq); }
        if (m.type === 'error') { probeErr.replaceChildren(errorList(m.errors)); stopProbe(); }
      });
      paint();
    }
    function stopProbe() { if (probeSock) { probeSock.close(); probeSock = null; } }
    const tick = setInterval(paint, 500);
    cleanup = () => { clearInterval(tick); stopProbe(); };
    filter.addEventListener('input', paint);
    probe.addEventListener('keydown', (e) => { if (e.key === 'Enter') startProbe(); });
    if (probeWhen) setTimeout(startProbe, 0);
    paint();
    return h('div', {},
      h('h2', { text: 'Live' }),
      h('div', { class: 'panel' },
        h('div', { class: 'row' }, probe, h('button', { class: 'primary', id: 'probe-run', text: 'Probe', onclick: startProbe }), h('button', { text: 'Back to stored rules', onclick: () => { probe.value = ''; stopProbe(); paint(); } })),
        probeErr,
        h('div', { class: 'row', style: 'margin-top:8px' }, filter,
          h('button', { text: 'Pause / resume', onclick: () => { state.paused = !state.paused; paint(); } }),
          h('button', { text: 'Clear', onclick: () => { state.rows.length = 0; probeRows.length = 0; paint(); } }), count)),
      body);
  }

  // --- Rules ---------------------------------------------------------------------------------------------------
  function page_rules(params) {
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
        msgs.replaceChildren(r.ok ? h('div', { class: 'ok', text: '✓ rules are valid' }) : errorList(r.errors));
        summary.replaceChildren(...(r.ok ? r.profiles.map((p) => h('div', { class: 'panel' }, h('b', { text: p.name }), ' ', h('span', { class: 'muted', text: p.mode + (p.enabled ? '' : ' · disabled') }),
          h('table', {}, h('tbody', {}, p.rules.map((x) => h('tr', { class: 'mono' }, h('td', { text: x.name }), h('td', { text: x.action }), h('td', { text: x.when, title: x.when }))))))) : []));
      } catch (e) { valid = false; msgs.replaceChildren(h('div', { class: 'err', text: e.message })); }
      saveBtn.disabled = !valid || editor.value === saved;
      info.textContent = editor.value === saved ? '' : 'unsaved changes';
    }
    editor.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(validate, 300); saveBtn.disabled = true; });
    saveBtn.addEventListener('click', async () => {
      try { await api('PUT', '/api/rules', { text: editor.value }); saved = editor.value; toast('saved; the running rules switched'); validate(); }
      catch (e) { msgs.replaceChildren(errorList((e.data && e.data.errors) || [{ message: e.message }])); toast('not saved', true); }
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
        op.replaceChildren(...ops.map((o) => h('option', { value: o, text: o, selected: o === c.op })));
        dl.replaceChildren(...(f.values || []).map((v) => h('option', { value: v })));
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
    function drawConds() { condBox.replaceChildren(...conds.map(condRow)); }
    function sync() { expr.value = buildWhen(conds, joiner.value); }
    joiner.addEventListener('change', sync);

    async function dryRun() {
      result.replaceChildren(h('span', { class: 'muted', text: 'running on recent traffic…' }));
      try {
        const p = await api('POST', '/api/preview', { when: expr.value, samples: 8 });
        result.replaceChildren(
          h('div', { id: 'preview-summary', text: p.matched + ' of ' + p.calls + ' calls match (' + p.transactions + ' recent transactions)' }),
          p.samples.length ? h('div', { class: 'scroll', style: 'margin-top:8px' }, rowTable(p.samples, openDrawer)) : null);
      } catch (e) { result.replaceChildren(errorList((e.data && e.data.errors) || [{ message: e.message }])); }
    }
    function watch() {
      if (probeSock) { probeSock.close(); probeSock = null; }
      const rows = [];
      const box = h('div', { class: 'scroll', style: 'margin-top:8px' });
      result.replaceChildren(h('div', { class: 'muted', text: 'watching live; matches appear below' }), box, h('button', { text: 'Stop', onclick: () => { probeSock.close(); probeSock = null; } }));
      probeSock = openSocket('?mode=probe&when=' + encodeURIComponent(expr.value), (m) => {
        if (m.type === 'row') { rows.unshift(m.row); rows.length = Math.min(rows.length, 60); box.replaceChildren(rowTable(rows, openDrawer)); }
        if (m.type === 'error') { result.replaceChildren(errorList(m.errors)); probeSock.close(); probeSock = null; }
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
      profileSel.replaceChildren(...(names.length ? names : ['rules']).map((n) => h('option', { value: n, text: n })), h('option', { value: '', text: '(new profile “rules”)' }));
    }
    editor.addEventListener('input', refreshProfiles);

    cleanup = () => { if (probeSock) probeSock.close(); };
    catalogueReady.then(async () => {
      await load();
      refreshProfiles();
      const w = params.get('when');
      if (w) { expr.value = w; }
      else { conds.push({ field: 'kind', op: '==', value: 'swap' }); drawConds(); sync(); }
      if (w && params.get('preview')) dryRun();
    });
    return h('div', {},
      h('h2', { text: 'Rules' }),
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
      try { const rows = await api('GET', '/api/rows?' + q); last = null; out.replaceChildren(h('div', { class: 'muted', text: rows.length + ' rows, newest first' }), h('div', { class: 'scroll' }, rowTable(rows, openDrawer))); }
      catch (e) { out.replaceChildren(h('div', { class: 'err', text: e.message })); }
    }
    async function runSql() {
      try {
        const r = await api('POST', '/api/sql', { sql: sql.value, max_rows: 2000 });
        last = r;
        out.replaceChildren(h('div', { class: 'row' }, h('span', { class: 'muted', id: 'sql-count', text: r.rows.length + ' rows' + (r.truncated ? ' (cut)' : '') }),
          h('button', { text: 'Download CSV', onclick: () => download('seqscope.csv', csvOf(r.columns, r.rows), 'text/csv') }),
          h('button', { text: 'Download JSON', onclick: () => download('seqscope.json', JSON.stringify(r), 'application/json') })),
          h('div', { class: 'scroll' }, resultTable(r.columns, r.rows)));
      } catch (e) { out.replaceChildren(h('div', { class: 'err', id: 'sql-error', text: e.message })); }
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
      if (!r.rows.length) { out.replaceChildren(h('div', { class: 'muted', text: 'No undecoded calls stored. Store them with a rule such as: kind == undecoded_known || (kind == unknown && len >= 4)' })); return; }
      out.replaceChildren(h('div', { class: 'scroll' }, resultTable(r.columns, r.rows, (col, v, row) => (col === 'calls' ? h('button', { style: 'margin-left:8px', text: 'draft rule', onclick: () => { location.hash = '#rules?preview=1&when=' + encodeURIComponent(whenForGap(r.columns, row)); } }) : null))));
    }).catch((e) => out.replaceChildren(h('div', { class: 'err', text: e.message })));
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
      out.replaceChildren(
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
    load().catch((e) => out.replaceChildren(h('div', { class: 'err', text: e.message })));
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
    $('#main').replaceChildren(pages[p](params));
    document.title = 'seqscope · ' + p;
  }
  window.addEventListener('hashchange', route);
  route();
})();
