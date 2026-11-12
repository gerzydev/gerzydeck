// Gerzy's deck — editor. Talks to the Rust core via Tauri invoke/listen.
import { TEMPLATES, sameAction } from './templates.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const WOW_NOTE = 'Pozor: ve WoW je víc herních akcí na jedno zmáčknutí proti pravidlům Blizzardu.';
const TEXT_NOTE = 'Psaní textu funguje v běžných aplikacích, hry ho mohou ignorovat.';

const ACTION_TYPES = [
  ['none', 'Nic'], ['keys', 'Klávesa / zkratka'], ['macro', 'Makro'], ['launch', 'Spustit program / web'],
  ['media', 'Média a hlasitost'], ['system', 'Systém'], ['profile', 'Přepnout profil'],
];
const MEDIA = [['playpause', 'Přehrát / pauza'], ['next', 'Další skladba'], ['prev', 'Předchozí skladba'], ['mute', 'Ztlumit zvuk'], ['volup', 'Hlasitost +'], ['voldown', 'Hlasitost −']];
const SYSTEM = [['lock', 'Zamknout PC'], ['sleep', 'Uspat PC']];
const STEP_TYPES = [['tap', 'Stisk'], ['down', 'Držet'], ['up', 'Pustit'], ['wait', 'Čekat'], ['text', 'Napsat text']];
const MODS = ['ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight', 'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight'];
const NAMES = {
  ControlLeft: 'Ctrl', ControlRight: 'Pravý Ctrl', ShiftLeft: 'Shift', ShiftRight: 'Pravý Shift', AltLeft: 'Alt', AltRight: 'AltGr',
  MetaLeft: 'Win', MetaRight: 'Pravý Win', Space: 'Mezerník', Enter: 'Enter', Escape: 'Esc', Tab: 'Tab', Backspace: 'Backspace',
  CapsLock: 'Caps Lock', ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→', PageUp: 'PgUp', PageDown: 'PgDn',
  Insert: 'Insert', Delete: 'Delete', PrintScreen: 'PrtSc', ContextMenu: 'Menu', NumLock: 'Num Lock', ScrollLock: 'Scroll Lock',
  Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'", Backquote: '`',
  Comma: ',', Period: '.', Slash: '/', IntlBackslash: '\\ (vedle Shiftu)', NumpadMultiply: 'Num *', NumpadAdd: 'Num +',
  NumpadSubtract: 'Num −', NumpadDivide: 'Num /', NumpadDecimal: 'Num ,', NumpadEnter: 'Num Enter',
};
const PAGES = {
  deck: ['Deck', 'Klikni na tlačítko a nastav mu obrázek a akci. Změny se ukládají hned.'],
  profiles: ['Profily', 'Každý profil má vlastních 15 tlačítek. Profil se může přepnout sám podle programu v popředí.'],
  settings: ['Nastavení', 'Jas, spouštění s Windows a kalibrace tlačítek.'],
};

let config = null;
let state = { connected: false };
let allCodes = [];
let page = 'deck';
let selectedKey = 1;
let showTemplates = false;
let recording = null; // { codes:Set, down:Set, onDone }
let calibration = null; // { step, map, used:Set }
let saveTimer = null;
const imgCache = new Map();

// ---------- helpers ----------
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs ?? {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'style') el.style.cssText = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k in el && k !== 'list') el[k] = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat(Infinity)) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}
const $ = (id) => document.getElementById(id);
function svgIcon(inner) {
  const t = document.createElement('template');
  t.innerHTML = `<svg class="i" viewBox="0 0 24 24">${inner}</svg>`;
  return t.content.firstChild;
}
const option = (value, label, selected) => h('option', { value, selected: value === selected }, label);
const codeName = (c) => NAMES[c] ?? (c.startsWith('Key') ? c.slice(3) : c.startsWith('Digit') ? c.slice(5) : c.startsWith('Numpad') ? `Num ${c.slice(6)}` : c);
const sortCodes = (list) => [...list.filter((c) => MODS.includes(c)), ...list.filter((c) => !MODS.includes(c))];
const newId = () => 'p' + crypto.randomUUID().replace(/-/g, '').slice(0, 10);

function toast(msg) {
  const t = h('div', { class: 'toast' }, String(msg));
  document.body.append(t);
  setTimeout(() => t.remove(), 4000);
}

function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try { await invoke('set_config', { config }); } catch (e) { toast(e); }
  }, 300);
}

const activeId = () => state.activeProfileId || config.activeProfileId;
const activeProfile = () => config.profiles.find((p) => p.id === activeId()) ?? config.profiles[0];
const keyCfg = (k) => activeProfile().keys[String(k)];

async function imageUrl(id) {
  if (!id) return null;
  if (!imgCache.has(id)) imgCache.set(id, invoke('image_data_url', { id }).catch(() => null));
  return imgCache.get(id);
}

function actionSummary(a) {
  switch (a?.type) {
    case 'keys': return a.codes.map(codeName).join(' + ');
    case 'macro': return `Makro (${a.steps.length} kroků)`;
    case 'launch': return `Spustit ${a.target}`;
    case 'media': return MEDIA.find(([v]) => v === a.command)?.[1] ?? 'Média';
    case 'system': return SYSTEM.find(([v]) => v === a.command)?.[1] ?? 'Systém';
    case 'profile': return a.profileId === '__back' ? 'Předchozí profil' : `Profil: ${config.profiles.find((p) => p.id === a.profileId)?.name ?? '?'}`;
    default: return 'Bez akce';
  }
}

// ---------- key tile ----------
function keyTile(k, extra = {}) {
  const cfg = keyCfg(k);
  const btn = h('button', { class: 'deck-key', type: 'button', title: `${k}: ${actionSummary(cfg.action)}`, style: `--key-bg:${cfg.bg}`, ...extra });
  btn.dataset.key = k;
  fillTile(btn, cfg, k);
  return btn;
}

function fillTile(btn, cfg, k) {
  btn.replaceChildren();
  btn.style.setProperty('--key-bg', cfg.bg);
  btn.title = `${k}: ${actionSummary(cfg.action)}`;
  btn.classList.toggle('has-img', Boolean(cfg.image));
  if (cfg.image) {
    const img = h('img', { alt: '' });
    imageUrl(cfg.image).then((u) => { if (u) img.src = u; });
    btn.append(img);
    if (cfg.label) btn.append(h('span', { class: 'shade' }));
  }
  if (cfg.label) btn.append(h('span', { class: `lbl ${cfg.image ? 'bottom' : 'center'}` }, cfg.label));
  btn.append(h('span', { class: 'num' }, k));
}

function refreshTile(k) {
  const btn = document.querySelector(`#page-deck .deck-key[data-key="${k}"]`);
  if (btn) fillTile(btn, keyCfg(k), k);
}

// ---------- sidebar / top ----------
function renderSidebar() {
  $('device-card').classList.toggle('is-on', Boolean(state.connected));
  $('device-sub').textContent = state.connected === undefined ? 'Program neběží'
    : `${state.connected ? 'Připojeno' : 'Nepřipojeno'} · ${state.activeProfileName ?? ''}`;
}

function renderProfileSelect() {
  const sel = $('profile-select');
  sel.replaceChildren(...config.profiles.map((p) => option(p.id, p.name, activeId())));
}

function renderBanners() {
  const b = [];
  const warn = (text) => h('div', { class: 'rm-banner' }, svgIcon('<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>'), text);
  if (state.conflicts?.length) b.push(warn(`Běží software Fifine (${state.conflicts.join(', ')}) — zavři ho, jinak se budou s deckem přetahovat.`));
  if (state.foregroundElevated) b.push(warn('Okno v popředí běží jako správce — klávesy z decku do něj nedojdou.'));
  if (state.connected === false) b.push(warn('Deck není připojený. Zkontroluj USB kabel — program se připojí sám.'));
  $('banners').replaceChildren(...b);
}

function setPage(p) {
  page = p;
  for (const btn of document.querySelectorAll('#nav .rm-nav-item')) btn.classList.toggle('active', btn.dataset.page === p);
  for (const id of Object.keys(PAGES)) $(`page-${id}`).hidden = id !== p;
  $('page-title').textContent = PAGES[p][0];
  $('page-sub').textContent = PAGES[p][1];
  renderPage();
}

function renderPage() {
  if (page === 'deck') renderDeck();
  else if (page === 'profiles') renderProfiles();
  else renderSettings();
}

// ---------- deck page ----------
function renderDeck() {
  const grid = h('div', { class: 'deck-grid' });
  for (let k = 1; k <= 15; k++) {
    grid.append(keyTile(k, { onclick: () => { selectedKey = k; showTemplates = false; renderDeck(); } }));
  }
  grid.querySelector(`[data-key="${selectedKey}"]`)?.classList.add('selected');
  const board = h('div', { class: 'deck-board' }, grid, h('div', { class: 'deck-caption' }, `Profil „${activeProfile().name}“ — rozložení jako na decku`));
  $('page-deck').replaceChildren(h('div', { class: 'deck-layout' }, board, renderEditor()));
}

function renderEditor() {
  const k = selectedKey;
  const cfg = keyCfg(k);
  const actionBox = h('div', { class: 'rm-stack' });
  const hintBox = h('div');

  const redrawAction = () => {
    actionBox.replaceChildren(...actionFields(cfg, () => { refreshTile(k); redrawHint(); save(); }, redrawAction));
    redrawHint();
  };
  const redrawHint = () => {
    const t = TEMPLATES.find((x) => x.hint && sameAction(x.action, cfg.action));
    hintBox.replaceChildren(t ? h('div', { class: 'rm-note is-info' }, t.hint) : '');
  };

  const fileInput = h('input', { type: 'file', accept: 'image/*', hidden: true, onchange: (e) => e.target.files[0] && importImage(e.target.files[0]) });
  const thumb = h('div', { class: 'img-thumb' }, cfg.image ? '' : 'žádný');
  if (cfg.image) imageUrl(cfg.image).then((u) => u && thumb.replaceChildren(h('img', { src: u, alt: '' })));
  const drop = h('div', {
    class: 'img-drop',
    ondragover: (e) => { e.preventDefault(); drop.classList.add('drag'); },
    ondragleave: () => drop.classList.remove('drag'),
    ondrop: (e) => { e.preventDefault(); drop.classList.remove('drag'); const f = e.dataTransfer.files[0]; if (f) importImage(f); },
  },
  thumb,
  h('div', { class: 'rm-stack', style: 'gap:6px' },
    h('div', { class: 'rm-row' },
      h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => fileInput.click() }, 'Vybrat obrázek…'),
      cfg.image && h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', onclick: () => { cfg.image = null; refreshTile(k); save(); renderDeck(); } }, 'Odebrat')),
    h('span', { class: 'rm-meta' }, 'PNG, JPG, GIF… nebo sem obrázek přetáhni. Ořízne se na čtverec.')),
  fileInput);

  async function importImage(file) {
    try {
      const id = await invoke('import_image', new Uint8Array(await file.arrayBuffer()));
      cfg.image = id;
      save();
      renderDeck();
    } catch (e) { toast(e); }
  }

  const tplList = showTemplates && h('div', { class: 'tpl-list' },
    TEMPLATES.map((t) => h('button', {
      class: 'tpl-item', type: 'button',
      onclick: () => { cfg.action = structuredClone(t.action); cfg.label = t.label; showTemplates = false; save(); renderDeck(); },
    }, t.name, h('small', {}, t.group))));

  const panel = h('div', { class: 'rm-panel' },
    h('div', { class: 'rm-panel-head' },
      h('h2', {}, `Tlačítko ${k}`),
      h('div', { class: 'rm-end' },
        h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => { showTemplates = !showTemplates; renderDeck(); } }, showTemplates ? 'Zavřít šablony' : 'Šablony'),
        h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', onclick: () => {
          activeProfile().keys[String(k)] = { image: null, label: '', bg: '#141618', action: { type: 'none' } };
          save(); renderDeck();
        } }, 'Vymazat'))),
    h('div', { class: 'rm-panel-body rm-stack' },
      tplList,
      h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Obrázek'), drop),
      h('div', { class: 'rm-row' },
        h('label', { class: 'rm-field rm-grow' }, h('span', { class: 'rm-label' }, 'Popisek'),
          h('input', { class: 'rm-input', value: cfg.label, maxLength: 40, placeholder: 'např. Fireball', oninput: (e) => { cfg.label = e.target.value; refreshTile(k); save(); } })),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Pozadí'),
          h('input', { class: 'rm-color', type: 'color', value: cfg.bg, oninput: (e) => { cfg.bg = e.target.value; refreshTile(k); save(); } }))),
      h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Akce'),
        h('select', { class: 'rm-select', onchange: (e) => { cfg.action = defaultAction(e.target.value); save(); refreshTile(k); redrawAction(); } },
          ACTION_TYPES.map(([v, l]) => option(v, l, cfg.action.type)))),
      actionBox,
      hintBox));
  redrawAction();
  return panel;
}

function defaultAction(type) {
  switch (type) {
    case 'keys': return { type, codes: [] };
    case 'macro': return { type, steps: [{ type: 'tap', code: 'Digit1' }] };
    case 'launch': return { type, target: '', args: [] };
    case 'media': return { type, command: 'playpause' };
    case 'system': return { type, command: 'lock' };
    case 'profile': return { type, profileId: '__back' };
    default: return { type: 'none' };
  }
}

function codeSelect(value, onchange, placeholder) {
  return h('select', { class: 'rm-select', onchange: (e) => onchange(e.target.value) },
    placeholder && h('option', { value: '' }, placeholder),
    allCodes.map((c) => option(c, `${codeName(c)}  ·  ${c}`, value)));
}

function splitArgs(s) {
  const out = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m;
  while ((m = re.exec(s))) out.push(m[1] ?? m[2]);
  return out;
}
const joinArgs = (args) => args.map((a) => (/\s/.test(a) ? `"${a}"` : a)).join(' ');

function actionFields(cfg, changed, redraw) {
  const a = cfg.action;
  switch (a.type) {
    case 'keys': {
      const recBtn = h('button', { class: 'rm-btn is-sm', type: 'button' }, 'Nahrát zkratku');
      recBtn.onclick = () => {
        if (recording) { stopRecording(); recBtn.classList.remove('is-recording'); recBtn.textContent = 'Nahrát zkratku'; return; }
        recBtn.classList.add('is-recording');
        recBtn.textContent = 'Zmáčkni zkratku na klávesnici…';
        startRecording((codes) => { a.codes = sortCodes(codes).slice(0, 6); changed(); redraw(); });
      };
      const chips = h('div', { class: 'chips' },
        a.codes.length ? a.codes.flatMap((c, i) => [
          i > 0 && h('span', { class: 'rm-plus' }, '+'),
          h('span', { class: 'rm-chip', title: c }, codeName(c),
            h('button', { type: 'button', title: 'Odebrat', onclick: () => { a.codes.splice(i, 1); changed(); redraw(); } }, '×')),
        ]) : h('span', { class: 'rm-meta' }, 'Zatím žádná klávesa.'));
      return [
        h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Klávesy (stisknou se spolu a drží, dokud držíš tlačítko)'), chips),
        h('div', { class: 'rm-row' }, recBtn,
          h('div', { class: 'rm-grow' }, codeSelect('', (c) => { if (c && !a.codes.includes(c) && a.codes.length < 6) { a.codes = sortCodes([...a.codes, c]); changed(); redraw(); } }, '+ přidat klávesu ze seznamu'))),
        h('div', { class: 'rm-meta' }, 'Klávesy se ukládají podle pozice na klávesnici. Zkratky, které si Windows vezme dřív (Win+L, Ctrl+Alt+Del), přidej ze seznamu.'),
      ];
    }
    case 'macro': {
      const hasText = a.steps.some((s) => s.type === 'text');
      const rows = a.steps.map((s, i) => {
        let param;
        if (s.type === 'wait') param = h('input', { class: 'rm-input', type: 'number', min: 0, max: 60000, step: 10, value: s.ms, oninput: (e) => { s.ms = Math.max(0, Math.min(60000, parseInt(e.target.value, 10) || 0)); changed(); } });
        else if (s.type === 'text') param = h('input', { class: 'rm-input', value: s.text, placeholder: 'Text k napsání', oninput: (e) => { s.text = e.target.value; changed(); } });
        else param = codeSelect(s.code, (c) => { s.code = c; changed(); });
        return h('div', { class: 'step' },
          h('select', { class: 'rm-select', onchange: (e) => {
            const t = e.target.value;
            a.steps[i] = t === 'wait' ? { type: t, ms: 100 } : t === 'text' ? { type: t, text: '' } : { type: t, code: s.code ?? 'Digit1' };
            changed(); redraw();
          } }, STEP_TYPES.map(([v, l]) => option(v, l, s.type))),
          param,
          h('div', { class: 'step-tools' },
            h('button', { class: 'rm-icon-btn', type: 'button', title: 'Nahoru', disabled: i === 0, onclick: () => { [a.steps[i - 1], a.steps[i]] = [a.steps[i], a.steps[i - 1]]; changed(); redraw(); } }, '↑'),
            h('button', { class: 'rm-icon-btn', type: 'button', title: 'Dolů', disabled: i === a.steps.length - 1, onclick: () => { [a.steps[i + 1], a.steps[i]] = [a.steps[i], a.steps[i + 1]]; changed(); redraw(); } }, '↓'),
            h('button', { class: 'rm-icon-btn is-danger', type: 'button', title: 'Smazat krok', disabled: a.steps.length === 1, onclick: () => { a.steps.splice(i, 1); changed(); redraw(); } }, '×')));
      });
      return [
        h('div', { class: 'rm-note is-warn' }, WOW_NOTE),
        h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Kroky (proběhnou jednou po zmáčknutí)'), h('div', { class: 'steps' }, rows)),
        hasText && h('div', { class: 'rm-note' }, TEXT_NOTE),
        h('div', { class: 'rm-row' },
          h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => { a.steps.push({ type: 'tap', code: 'Digit1' }); changed(); redraw(); } }, '+ Krok'),
          h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => { a.steps.push({ type: 'wait', ms: 100 }); changed(); redraw(); } }, '+ Čekání')),
      ];
    }
    case 'launch':
      return [
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Program, soubor, složka nebo adresa webu'),
          h('input', { class: 'rm-input', value: a.target, placeholder: 'C:\\Program Files\\…\\program.exe  nebo  https://…', oninput: (e) => { a.target = e.target.value; changed(); } })),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Argumenty (volitelné, jen u programů)'),
          h('input', { class: 'rm-input', value: joinArgs(a.args), oninput: (e) => { a.args = splitArgs(e.target.value); changed(); } })),
      ];
    case 'media':
      return [h('select', { class: 'rm-select', onchange: (e) => { a.command = e.target.value; changed(); } }, MEDIA.map(([v, l]) => option(v, l, a.command)))];
    case 'system':
      return [h('select', { class: 'rm-select', onchange: (e) => { a.command = e.target.value; changed(); } }, SYSTEM.map(([v, l]) => option(v, l, a.command)))];
    case 'profile':
      return [h('select', { class: 'rm-select', onchange: (e) => { a.profileId = e.target.value; changed(); } },
        option('__back', 'Předchozí profil', a.profileId),
        config.profiles.map((p) => option(p.id, p.name, a.profileId)))];
    default:
      return [h('div', { class: 'rm-meta' }, 'Tlačítko nic nedělá. Vyber akci nebo použij šablonu.')];
  }
}

// ---------- shortcut recorder ----------
function onRecordDown(e) {
  if (!recording) return;
  e.preventDefault();
  e.stopPropagation();
  if (allCodes.includes(e.code)) { recording.codes.add(e.code); recording.down.add(e.code); }
}
function onRecordUp(e) {
  if (!recording) return;
  e.preventDefault();
  recording.down.delete(e.code);
  if (recording.down.size === 0 && recording.codes.size > 0) {
    const { codes, onDone } = recording;
    stopRecording();
    onDone([...codes]);
  }
}
function startRecording(onDone) {
  recording = { codes: new Set(), down: new Set(), onDone };
}
function stopRecording() { recording = null; }
window.addEventListener('keydown', onRecordDown, true);
window.addEventListener('keyup', onRecordUp, true);

// ---------- profiles page ----------
let processNames = [];
let pendingDelete = null;

function renderProfiles() {
  const list = h('div', { class: 'prof-list' });
  const datalist = h('datalist', { id: 'proc-list' }, processNames.map((n) => h('option', { value: n })));
  for (const p of config.profiles) {
    const isDefault = p.id === config.defaultProfileId;
    const isActive = p.id === activeId();
    const procInput = h('input', { class: 'rm-input', placeholder: 'např. wow.exe', list: 'proc-list' });
    procInput.setAttribute('list', 'proc-list');
    const addProc = () => {
      const v = procInput.value.trim().toLowerCase();
      if (v && !p.processes.includes(v)) { p.processes.push(v); save(); renderProfiles(); }
    };
    procInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') addProc(); });
    const delArmed = pendingDelete === p.id;
    list.append(h('div', { class: 'rm-panel' },
      h('div', { class: 'rm-panel-head' },
        h('input', { class: 'rm-input', style: 'width:240px;font-weight:600', value: p.name, maxLength: 60, oninput: (e) => { p.name = e.target.value; save(); renderProfileSelect(); } }),
        isDefault && h('span', { class: 'prof-badge' }, 'Výchozí'),
        isActive && h('span', { class: 'prof-badge' }, 'Aktivní'),
        h('div', { class: 'rm-end' },
          !isDefault && h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => { config.defaultProfileId = p.id; save(); renderProfiles(); } }, 'Nastavit jako výchozí'),
          !isActive && h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => switchProfile(p.id) }, 'Přepnout na deck'),
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => {
            const copy = structuredClone(p);
            copy.id = newId(); copy.name = `${p.name} (kopie)`; copy.processes = [];
            config.profiles.splice(config.profiles.indexOf(p) + 1, 0, copy);
            save(); renderProfiles(); renderProfileSelect();
          } }, 'Duplikovat'),
          h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', disabled: config.profiles.length === 1, onclick: () => {
            if (!delArmed) { pendingDelete = p.id; renderProfiles(); setTimeout(() => { if (pendingDelete === p.id) { pendingDelete = null; renderProfiles(); } }, 3000); return; }
            pendingDelete = null;
            config.profiles = config.profiles.filter((x) => x.id !== p.id);
            if (config.defaultProfileId === p.id) config.defaultProfileId = config.profiles[0].id;
            if (config.activeProfileId === p.id) config.activeProfileId = config.defaultProfileId;
            if (state.activeProfileId === p.id) state.activeProfileId = config.activeProfileId;
            save(); renderProfiles(); renderProfileSelect();
          } }, delArmed ? 'Opravdu smazat?' : 'Smazat'))),
      h('div', { class: 'rm-panel-body rm-stack' },
        h('div', { class: 'rm-field' },
          h('span', { class: 'rm-label' }, 'Přepnout automaticky, když je v popředí program:'),
          h('div', { class: 'chips' },
            p.processes.length ? p.processes.map((proc, i) => h('span', { class: 'rm-chip' }, proc,
              h('button', { type: 'button', title: 'Odebrat', onclick: () => { p.processes.splice(i, 1); save(); renderProfiles(); } }, '×')))
              : h('span', { class: 'rm-meta' }, isDefault ? 'Výchozí profil se použije, když žádné pravidlo nesedí.' : 'Žádné pravidlo — profil se přepíná jen ručně.'))),
        h('div', { class: 'rm-row' },
          h('div', { class: 'rm-grow' }, procInput),
          h('button', { class: 'rm-btn is-sm', type: 'button', onclick: addProc }, 'Přidat')))));
  }
  $('page-profiles').replaceChildren(
    h('div', { class: 'rm-row', style: 'margin-bottom:12px' },
      h('button', { class: 'rm-btn is-primary', type: 'button', onclick: () => {
        const keys = {};
        for (let k = 1; k <= 15; k++) keys[k] = { image: null, label: '', bg: '#141618', action: { type: 'none' } };
        config.profiles.push({ id: newId(), name: 'Nový profil', processes: [], keys });
        save(); renderProfiles(); renderProfileSelect();
      } }, '+ Nový profil'),
      h('button', { class: 'rm-btn is-ghost', type: 'button', onclick: loadProcesses }, 'Načíst běžící programy'),
      h('span', { class: 'rm-meta' }, processNames.length ? `Nabídka obsahuje ${processNames.length} běžících programů.` : 'Tip: spusť hru a klikni na „Načíst běžící programy“.')),
    datalist, list);
}

async function loadProcesses() {
  try { processNames = await invoke('list_processes'); } catch (e) { toast(e); }
  renderProfiles();
}

async function switchProfile(id) {
  try { await invoke('set_active_profile', { id }); config.activeProfileId = id; state.activeProfileId = id; renderPage(); renderProfileSelect(); } catch (e) { toast(e); }
}

// ---------- settings page ----------
let autostart = null;

function renderSettings() {
  const bright = h('span', { class: 'rm-meta', style: 'width:40px;text-align:right' }, `${config.brightness} %`);
  const sw = h('button', { class: 'rm-switch', type: 'button', role: 'switch', 'aria-checked': String(Boolean(autostart)), disabled: autostart === null,
    onclick: async () => {
      try { await invoke('set_autostart', { on: !autostart }); autostart = !autostart; } catch (e) { toast(e); }
      renderSettings();
    } });
  const rows = h('div', { class: 'kv' },
    h('div', {}, h('b', {}, 'Jas displejů'),
      h('div', { class: 'rm-row' }, h('input', { type: 'range', min: 0, max: 100, value: config.brightness, oninput: (e) => { config.brightness = +e.target.value; bright.textContent = `${config.brightness} %`; save(); } }), bright)),
    h('div', {}, h('div', { class: 'rm-stack', style: 'gap:2px' }, h('b', {}, 'Spouštět s Windows'), h('span', { class: 'rm-meta' }, 'Program se spustí schovaný v oznamovací oblasti.')), sw),
    h('div', {}, h('div', { class: 'rm-stack', style: 'gap:2px' }, h('b', {}, 'Kalibrace tlačítek'), h('span', { class: 'rm-meta' }, config.keyMap ? 'Použita vlastní kalibrace.' : 'Výchozí rozložení (ověřené na tvém decku).')),
      h('div', { class: 'rm-row' },
        config.keyMap && h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => { config.keyMap = null; save(); renderSettings(); } }, 'Obnovit výchozí'),
        h('button', { class: 'rm-btn is-sm', type: 'button', onclick: startCalibration }, 'Spustit kalibraci'))),
    h('div', {}, h('b', {}, 'Data'), h('code', {}, '%APPDATA%\\GerzysDeck')));

  const parts = [h('div', { class: 'rm-panel' }, h('div', { class: 'rm-panel-body', style: 'padding-top:8px' }, rows))];
  if (calibration) parts.push(calibrationPanel());
  $('page-settings').replaceChildren(h('div', { class: 'rm-stack' }, parts));
}

function startCalibration() {
  calibration = { step: 1, map: new Array(15).fill(0), used: new Set() };
  invoke('set_calibrating', { on: true }).catch(toast);
  renderSettings();
}

function endCalibration(saveIt) {
  if (saveIt) { config.keyMap = calibration.map; save(); toast('Kalibrace uložena.'); }
  calibration = null;
  invoke('set_calibrating', { on: false }).catch(toast);
  renderSettings();
}

function calibrationPanel() {
  const grid = h('div', { class: 'deck-grid' });
  for (let k = 1; k <= 15; k++) {
    const cls = k === calibration.step ? 'cal-target' : k < calibration.step ? 'cal-done' : '';
    grid.append(h('div', { class: `deck-key ${cls}`, style: '--key-bg:var(--surface-3)' }, h('span', { class: 'lbl center' }, k)));
  }
  return h('div', { class: 'rm-panel' },
    h('div', { class: 'rm-panel-head' }, h('h2', {}, `Kalibrace — ${calibration.step} / 15`),
      h('div', { class: 'rm-end' }, h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => endCalibration(false) }, 'Zrušit'))),
    h('div', { class: 'rm-panel-body rm-stack' },
      h('div', { class: 'rm-note is-info' }, 'Zmáčkni na decku tlačítko, které je tady zvýrazněné. Během kalibrace tlačítka nic nespouštějí.'),
      h('div', { class: 'cal-grid' }, grid)));
}

function onRaw({ deviceId, pressed }) {
  if (!calibration || !pressed || calibration.used.has(deviceId)) return;
  calibration.used.add(deviceId);
  calibration.map[deviceId - 1] = calibration.step;
  calibration.step++;
  if (calibration.step > 15) endCalibration(true);
  else renderSettings();
}

// ---------- live events ----------
function onState(s) {
  const profileChanged = s.activeProfileId !== state.activeProfileId;
  state = s;
  if (config && s.activeProfileId) config.activeProfileId = config.profiles.some((p) => p.id === s.activeProfileId) ? s.activeProfileId : config.activeProfileId;
  renderSidebar();
  renderBanners();
  if (!config) return;
  renderProfileSelect();
  if (profileChanged && page !== 'settings') renderPage();
}

function onKey({ key, pressed }) {
  const tile = document.querySelector(`#page-deck .deck-key[data-key="${key}"]`);
  tile?.classList.toggle('pressed', pressed);
}

// ---------- theme ----------
function applyTheme(choice) {
  if (choice === 'dark' || choice === 'light') document.documentElement.dataset.theme = choice;
  else delete document.documentElement.dataset.theme;
  try { if (choice === 'auto') localStorage.removeItem('deck-theme'); else localStorage.setItem('deck-theme', choice); } catch { /* ignore */ }
  for (const b of document.querySelectorAll('#theme-switch button')) b.setAttribute('aria-pressed', String(b.dataset.themeChoice === choice));
}

// ---------- boot ----------
async function boot() {
  let theme = 'auto';
  try { theme = localStorage.getItem('deck-theme') ?? 'auto'; } catch { /* ignore */ }
  applyTheme(theme);
  for (const b of document.querySelectorAll('#theme-switch button')) b.onclick = () => applyTheme(b.dataset.themeChoice);
  for (const b of document.querySelectorAll('#nav .rm-nav-item')) b.onclick = () => setPage(b.dataset.page);
  $('profile-select').onchange = (e) => switchProfile(e.target.value);

  await listen('deck://state', (e) => onState(e.payload));
  await listen('deck://key', (e) => onKey(e.payload));
  await listen('deck://raw', (e) => onRaw(e.payload));

  try {
    [config, state, allCodes] = await Promise.all([invoke('get_config'), invoke('get_state'), invoke('list_scancodes')]);
  } catch (e) {
    $('page-deck').replaceChildren(h('div', { class: 'rm-note is-warn' }, `Nepodařilo se spojit s jádrem programu: ${e}`));
    return;
  }
  invoke('get_autostart').then((v) => { autostart = v; if (page === 'settings') renderSettings(); }).catch(() => {});
  renderSidebar();
  renderBanners();
  renderProfileSelect();
  setPage('deck');
}

boot();
