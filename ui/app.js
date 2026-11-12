// Gerzy's deck — editor. Talks to the Rust core via Tauri invoke/listen.
import { TEMPLATES, sameAction } from './templates.js';
import { iconById, rasterizeIcon } from './icons.js';
import { renderIconsPage, pickIcon } from './iconlib.js';
import { renderPluginsPage, loadInstalled, pluginActionFields, pickPluginAction, closePi, pluginAction } from './plugins.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const WOW_NOTE = 'Pozor: ve WoW je víc herních akcí na jedno zmáčknutí proti pravidlům Blizzardu.';
const TEXT_NOTE = 'Psaní textu funguje v běžných aplikacích, hry ho mohou ignorovat.';

const ACTION_TYPES = [
  ['none', 'Nic'], ['keys', 'Klávesa / zkratka'], ['macro', 'Makro'], ['launch', 'Spustit program / web'],
  ['media', 'Média a hlasitost'], ['system', 'Systém'], ['profile', 'Přepnout profil'], ['usage', 'Využití AI (Claude, GPT)'], ['plugin', 'Plugin…'],
];
const MEDIA = [['playpause', 'Přehrát / pauza'], ['next', 'Další skladba'], ['prev', 'Předchozí skladba'], ['mute', 'Ztlumit zvuk'], ['volup', 'Hlasitost +'], ['voldown', 'Hlasitost −']];
const SYSTEM = [['lock', 'Zamknout PC'], ['sleep', 'Uspat PC']];
const USAGE_PROVIDERS = [['claude', 'Claude (přihlášení z Claude Code)'], ['codex', 'ChatGPT (přihlášení z Codexu)']];
const USAGE_WINDOWS = [['session', '5hodinové okno'], ['weekly', 'Týdenní limit']];
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
  plugins: ['Pluginy', 'Pluginy pro Stream Deck: OBS, Spotify, Discord, PC metriky a stovky dalších.'],
  profiles: ['Profily', 'Každý profil má vlastních 15 tlačítek. Profil se může přepnout sám podle programu v popředí.'],
  icons: ['Ikony', 'Katalog open-source ikon, tvoje vlastní obrázky a balíky ikon pro Stream Deck.'],
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

/** Pushes pending edits now (the core must know a key's new action before its property inspector opens). */
async function saveNow() {
  clearTimeout(saveTimer);
  try { await invoke('set_config', { config }); } catch (e) { toast(e); }
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
    case 'plugin': return pluginAction(a.plugin, a.action).action?.name ?? 'Plugin';
    case 'usage': return `${a.provider === 'claude' ? 'Claude' : 'GPT'} – ${a.window === 'weekly' ? 'týden' : '5h okno'}`;
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
  if (cfg.action?.type === 'usage') {
    btn.append(usagePreview(cfg));
    btn.append(h('span', { class: 'num' }, k));
    return;
  }
  const pa = cfg.action?.type === 'plugin' && !cfg.image ? pluginAction(cfg.action.plugin, cfg.action.action) : null;
  if (pa?.action?.icon) btn.append(h('img', { class: 'act-icon', src: pa.action.icon, alt: '' }));
  if (pa && !pa.plugin) btn.append(h('span', { class: 'lbl center' }, '⚠'));
  if (cfg.label) btn.append(h('span', { class: `lbl ${cfg.image || pa?.action?.icon ? 'bottom' : 'center'}` }, cfg.label));
  btn.append(h('span', { class: 'num' }, k));
}

function resetText(at) {
  const s = Math.round(at - Date.now() / 1000);
  if (s <= 0) return 'teď';
  if (s < 60) return '<1 min';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ${String(Math.floor(s % 3600 / 60)).padStart(2, '0')}`;
  return `${Math.floor(s / 86400)} d ${Math.floor(s % 86400 / 3600)} h`;
}

/** Same layout as the key on the deck: name, percentage, bar, reset countdown. */
function usagePreview(cfg) {
  const a = cfg.action;
  const name = cfg.label || `${a.provider === 'claude' ? 'Claude' : 'GPT'} ${a.window === 'weekly' ? 'týden' : '5h'}`;
  const u = state.usage?.[a.provider];
  const w = u && (a.window === 'weekly' ? u.weekly : u.session);
  const box = h('span', { class: 'usage-prev' }, h('span', { class: 'u-name' }, name));
  if (u?.error) box.append(h('span', { class: 'u-err' }, '!'), h('span', { class: 'u-msg' }, u.error));
  else if (!w) box.append(h('span', { class: 'u-pct' }, '…'));
  else {
    const color = w.percent >= 90 ? '#ff5a5a' : w.percent >= 70 ? '#f2b84b' : '#46e06c';
    box.append(
      h('span', { class: 'u-pct', style: `color:${color}` }, `${Math.round(w.percent)} %`),
      h('span', { class: 'u-bar' }, h('span', { style: `width:${Math.min(100, Math.max(0, w.percent))}%;background:${color}` })),
      h('span', { class: 'u-reset' }, `za ${resetText(w.resetsAt)}`));
  }
  return box;
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
  if (p !== 'deck') closePi(ui);
  page = p;
  for (const btn of document.querySelectorAll('#nav .rm-nav-item')) btn.classList.toggle('active', btn.dataset.page === p);
  for (const id of Object.keys(PAGES)) $(`page-${id}`).hidden = id !== p;
  $('page-title').textContent = PAGES[p][0];
  $('page-sub').textContent = PAGES[p][1];
  renderPage();
}

function renderPage() {
  if (page === 'deck') renderDeck();
  else if (page === 'plugins') renderPluginsPage(ui, $('page-plugins'));
  else if (page === 'icons') renderIconsPage(ui, $('page-icons'));
  else if (page === 'profiles') renderProfiles();
  else renderSettings();
}

// ---------- deck page ----------
function renderDeck() {
  const grid = h('div', { class: 'deck-grid' });
  for (let k = 1; k <= 15; k++) {
    grid.append(keyTile(k, {
      draggable: true,
      onclick: () => { if (k !== selectedKey) closePi(ui); selectedKey = k; showTemplates = false; renderDeck(); },
      ondragstart: (e) => { e.dataTransfer.setData('text/x-deck-key', String(k)); e.dataTransfer.effectAllowed = 'move'; },
      ondragover: (e) => { if (e.dataTransfer.types.includes('text/x-deck-key')) { e.preventDefault(); e.currentTarget.classList.add('drop-target'); } },
      ondragleave: (e) => e.currentTarget.classList.remove('drop-target'),
      ondrop: async (e) => {
        e.preventDefault();
        e.currentTarget.classList.remove('drop-target');
        const from = Number(e.dataTransfer.getData('text/x-deck-key'));
        if (!from || from === k) return;
        closePi(ui);
        await saveNow();
        try {
          config = await invoke('swap_keys', { profileId: activeProfile().id, a: from, b: k });
          selectedKey = k;
          renderDeck();
        } catch (err) { toast(err); }
      },
    }));
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
    actionBox.replaceChildren(...actionFields(cfg, () => { refreshTile(k); redrawHint(); save(); }, redrawAction, k));
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
      h('button', { class: 'rm-btn is-sm', type: 'button', onclick: async () => {
        const res = await pickIcon(ui);
        if (res === 'file') fileInput.click();
        else if (res) {
          try { cfg.image = await invoke('import_image', res); save(); renderDeck(); } catch (e) { toast(e); }
        }
      } }, 'Vybrat obrázek…'),
      cfg.image && h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', onclick: () => { cfg.image = null; refreshTile(k); save(); renderDeck(); } }, 'Odebrat')),
    h('span', { class: 'rm-meta' }, 'Katalog ikon, tvoje ikony, balíky… nebo sem obrázek přetáhni.')),
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
      onclick: async () => {
        cfg.action = structuredClone(t.action);
        cfg.label = t.label;
        showTemplates = false;
        try { if (t.icon) await applyIcon(cfg, t.icon); } catch (e) { toast(e); }
        save(); renderDeck();
      },
    }, h('span', { class: 'tpl-name' }, t.icon && h('img', { class: 'tpl-icon', src: iconSrc(iconById(t.icon)), alt: '' }), t.name), h('small', {}, t.group))));

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
      h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Obrázek'), drop,
        ),
      h('div', { class: 'rm-row' },
        h('label', { class: 'rm-field rm-grow' }, h('span', { class: 'rm-label' }, 'Popisek'),
          h('input', { class: 'rm-input', value: cfg.label, maxLength: 40, placeholder: 'např. Fireball', oninput: (e) => { cfg.label = e.target.value; refreshTile(k); save(); } })),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Pozadí'),
          h('input', { class: 'rm-color', type: 'color', value: cfg.bg, oninput: (e) => { cfg.bg = e.target.value; refreshTile(k); save(); } }))),
      h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Akce'),
        actionGrid(cfg, async (type) => {
          if (type === 'plugin') {
            const pick = await pickPluginAction(ui);
            if (!pick) return;
            cfg.action = { type: 'plugin', plugin: pick.plugin, action: pick.action, settings: {}, state: 0 };
          } else {
            if (type === cfg.action.type) return;
            closePi(ui);
            cfg.action = defaultAction(type);
          }
          save(); renderDeck();
        })),
      actionBox,
      hintBox));
  redrawAction();
  return panel;
}

/** Puts a built-in icon on a key (rasterised in the webview, stored like any upload). */
async function applyIcon(cfg, iconId) {
  const icon = iconById(iconId);
  if (!icon) return;
  cfg.image = await invoke('import_image', await rasterizeIcon(icon));
}

const iconSrc = (icon) => 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(icon.svg);

const ACTION_ICONS = {
  none: '<circle cx="12" cy="12" r="8"/><path d="M6.5 6.5l11 11"/>',
  keys: '<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/>',
  macro: '<path d="M4 6h10M4 12h10M4 18h6"/><path d="M16 14.5l5 3.5-5 3.5z"/>',
  launch: '<path d="M14 4h6v6"/><path d="M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
  media: '<path d="M9 18V6l11-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>',
  system: '<path d="M12 3v8"/><path d="M6.3 6.6a8 8 0 1 0 11.4 0"/>',
  profile: '<path d="M7 7h12l-3-3"/><path d="M17 17H5l3 3"/>',
  plugin: '<path d="M9 3v4M15 3v4M7 7h10v4a5 5 0 0 1-10 0z"/><path d="M12 16v5"/>',
  usage: '<path d="M4 15a8 8 0 0 1 16 0"/><path d="M12 15l4-5"/><path d="M4 19h16"/>',
};
const ACTION_SHORT = { none: 'Nic', keys: 'Klávesa', macro: 'Makro', launch: 'Spustit', media: 'Média', system: 'Systém', profile: 'Profil', usage: 'Využití AI', plugin: 'Plugin' };

/** 4×2 grid of action types; the Plugin tile shows the chosen plugin action. */
function actionGrid(cfg, onPick) {
  const current = cfg.action.type;
  const pa = current === 'plugin' ? pluginAction(cfg.action.plugin, cfg.action.action) : null;
  return h('div', { class: 'act-grid', role: 'radiogroup' },
    Object.keys(ACTION_SHORT).map((type) => {
      const active = type === current;
      const usePluginIcon = type === 'plugin' && pa?.action?.icon;
      const name = type === 'plugin' && pa?.action ? pa.action.name : ACTION_SHORT[type];
      return h('button', {
        class: `act-tile${active ? ' active' : ''}`, type: 'button', role: 'radio', 'aria-checked': String(active),
        title: ACTION_TYPES.find(([v]) => v === type)?.[1].replace('…', '') ?? name,
        onclick: () => onPick(type),
      },
      usePluginIcon ? h('img', { src: pa.action.icon, alt: '' }) : svgIcon(ACTION_ICONS[type]),
      h('span', {}, name));
    }));
}

function defaultAction(type) {
  switch (type) {
    case 'keys': return { type, codes: [] };
    case 'macro': return { type, steps: [{ type: 'tap', code: 'Digit1' }] };
    case 'launch': return { type, target: '', args: [] };
    case 'media': return { type, command: 'playpause' };
    case 'system': return { type, command: 'lock' };
    case 'profile': return { type, profileId: '__back' };
    case 'usage': return { type, provider: 'claude', window: 'session' };
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

function actionFields(cfg, changed, redraw, k) {
  const a = cfg.action;
  switch (a.type) {
    case 'plugin':
      return pluginActionFields(ui, activeProfile().id, k, cfg, (rerender) => { save(); if (rerender) renderDeck(); else changed(); });
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
    case 'usage':
      return [
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Služba'),
          h('select', { class: 'rm-select', onchange: (e) => { a.provider = e.target.value; changed(); } }, USAGE_PROVIDERS.map(([v, l]) => option(v, l, a.provider)))),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Limit'),
          h('select', { class: 'rm-select', onchange: (e) => { a.window = e.target.value; changed(); } }, USAGE_WINDOWS.map(([v, l]) => option(v, l, a.window)))),
        h('div', { class: 'rm-meta' }, 'Tlačítko ukáže procenta a čas do resetu, obnovuje se každou minutu a stiskem hned. Použije přihlášení z Claude Code / Codexu – ty nemusí běžet, stačí v nich být přihlášený. Popisek nahradí název nahoře.'),
      ];
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

  const parts = [h('div', { class: 'rm-panel' }, h('div', { class: 'rm-panel-body', style: 'padding-top:8px' }, rows)), bootLogoPanel()];
  if (calibration) parts.push(calibrationPanel());
  $('page-settings').replaceChildren(h('div', { class: 'rm-stack' }, parts));
}

// ---------- boot logo ----------
let bootLogo = { file: null, url: null, busy: false };

function bootLogoPanel() {
  const input = h('input', { type: 'file', accept: 'image/*', hidden: true, onchange: (e) => pickBoot(e.target.files[0]) });
  function pickBoot(file) {
    if (!file) return;
    if (bootLogo.url) URL.revokeObjectURL(bootLogo.url);
    bootLogo = { file, url: URL.createObjectURL(file), busy: false };
    renderSettings();
  }
  async function useDefault() {
    const blob = await (await fetch('boot-default.png')).blob();
    pickBoot(new File([blob], 'gerzys-deck-boot.png', { type: 'image/png' }));
  }
  async function upload() {
    bootLogo.busy = true;
    renderSettings();
    try {
      await invoke('set_boot_logo', new Uint8Array(await bootLogo.file.arrayBuffer()));
      toast('Boot logo je uložené v decku. Ukáže se po příštím zapojení.');
    } catch (e) { toast(e); }
    bootLogo.busy = false;
    renderSettings();
  }
  const preview = h('div', { class: 'boot-prev',
    ondragover: (e) => { e.preventDefault(); },
    ondrop: (e) => { e.preventDefault(); const f = e.dataTransfer.files[0]; if (f?.type.startsWith('image/')) pickBoot(f); } },
  bootLogo.url ? h('img', { src: bootLogo.url, alt: '' }) : h('span', { class: 'rm-meta' }, 'Sem přetáhni obrázek'));
  return h('div', { class: 'rm-panel' },
    h('div', { class: 'rm-panel-head' }, h('h2', {}, 'Boot logo')),
    h('div', { class: 'rm-panel-body rm-stack' },
      h('span', { class: 'rm-meta' }, 'Obrázek, který deck ukáže po zapojení, než se připojí program. Ořízne se na 800 × 480 a uloží přímo do decku.'),
      preview,
      h('div', { class: 'rm-row', style: 'flex-wrap:wrap' },
        h('button', { class: 'rm-btn is-sm', type: 'button', disabled: bootLogo.busy, onclick: () => input.click() }, 'Vybrat obrázek…'),
        h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', disabled: bootLogo.busy, onclick: useDefault }, 'Logo Gerzy\'s deck'),
        h('span', { class: 'rm-grow' }),
        h('button', { class: 'rm-btn is-sm is-primary', type: 'button', disabled: !bootLogo.file || bootLogo.busy || state.connected === false, onclick: upload },
          bootLogo.busy ? 'Nahrávám… (asi 7 s)' : 'Nahrát do decku')),
      bootLogo.busy && h('div', { class: 'rm-note is-info' }, 'Během nahrávání deck neodpojuj.'),
      input));
}

async function startCalibration() {
  try { await invoke('set_calibrating', { on: true }); } catch (e) { toast(e); return; }
  calibration = { step: 1, map: new Array(15).fill(0), used: new Set() };
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
  // the core cancelled calibration (window closed) → drop the half-done wizard, save nothing
  // (only after the core confirmed it started, so an older in-flight state event cannot cancel it)
  if (calibration) {
    if (s.calibrating) calibration.confirmed = true;
    else if (calibration.confirmed) { calibration = null; if (page === 'settings') renderSettings(); }
  }
  if (config && s.activeProfileId) config.activeProfileId = config.profiles.some((p) => p.id === s.activeProfileId) ? s.activeProfileId : config.activeProfileId;
  renderSidebar();
  renderBanners();
  if (!config) return;
  if (page === 'deck') {
    for (let k = 1; k <= 15; k++) if (keyCfg(k).action?.type === 'usage') refreshTile(k);
  }
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
const ui = { h, invoke, toast, svgIcon, save: () => save(), saveNow: () => saveNow(), renderDeck: () => renderDeck() };

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
  // a reload must not leave the core stuck in calibration mode
  invoke('set_calibrating', { on: false }).catch(() => {});
  loadInstalled(ui).then(() => { if (page === 'deck') renderDeck(); });
  invoke('get_autostart').then((v) => { autostart = v; if (page === 'settings') renderSettings(); }).catch(() => {});
  renderSidebar();
  renderBanners();
  renderProfileSelect();
  setPage('deck');
}

boot();
