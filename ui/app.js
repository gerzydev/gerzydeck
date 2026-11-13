// Gerzy's deck — editor. Talks to the Rust core via Tauri invoke/listen.
import { TEMPLATES, sameAction } from './templates.js';
import { iconById, rasterizeIcon } from './icons.js';
import { renderIconsPage, pickIcon } from './iconlib.js';
import { renderPluginsPage, loadInstalled, pluginActionFields, pickPluginAction, closePi, pluginAction } from './plugins.js';
import { t, lang, setLang } from './i18n.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const WOW_NOTE = t("Careful: in WoW, more than one game action per key press breaks Blizzard's rules.");
const TEXT_NOTE = t("Typing text works in regular apps, games may ignore it.");

const ACTION_TYPES = [
  ['none', t("None")], ['keys', t("Key / shortcut")], ['macro', t("Macro")], ['launch', t("Launch program / website")],
  ['sound', t("Sound (soundboard)")], ['media', t("Media and volume")], ['system', t("System")], ['profile', t("Switch profile")], ['usage', t("AI usage (Claude, GPT)")], ['plugin', 'Plugin…'],
];
const MEDIA = [['playpause', t("Play / pause")], ['next', t("Next track")], ['prev', t("Previous track")], ['mute', t("Mute")], ['volup', t("Volume +")], ['voldown', t("Volume −")]];
const SYSTEM = [['lock', t("Lock PC")], ['sleep', t("Sleep PC")]];
const USAGE_PROVIDERS = [['claude', t("Claude (login from Claude Code)")], ['codex', t("ChatGPT (login from Codex)")]];
const USAGE_WINDOWS = [['session', t("5-hour window")], ['weekly', t("Weekly limit")]];
const STEP_TYPES = [['tap', t("Tap")], ['down', t("Hold")], ['up', t("Release")], ['wait', t("Wait")], ['text', t("Type text")]];
const MODS = ['ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight', 'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight'];
const NAMES = {
  ControlLeft: 'Ctrl', ControlRight: t("Right Ctrl"), ShiftLeft: 'Shift', ShiftRight: t("Right Shift"), AltLeft: 'Alt', AltRight: 'AltGr',
  MetaLeft: 'Win', MetaRight: t("Right Win"), Space: t("Space"), Enter: 'Enter', Escape: 'Esc', Tab: 'Tab', Backspace: 'Backspace',
  CapsLock: 'Caps Lock', ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→', PageUp: 'PgUp', PageDown: 'PgDn',
  Insert: 'Insert', Delete: 'Delete', PrintScreen: 'PrtSc', ContextMenu: 'Menu', NumLock: 'Num Lock', ScrollLock: 'Scroll Lock',
  Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'", Backquote: '`',
  Comma: ',', Period: '.', Slash: '/', IntlBackslash: t('\\ (next to Shift)'), NumpadMultiply: 'Num *', NumpadAdd: 'Num +',
  NumpadSubtract: 'Num −', NumpadDivide: 'Num /', NumpadDecimal: 'Num ,', NumpadEnter: 'Num Enter',
};
const PAGES = {
  deck: ['Deck', t("Click a key to set its image and action. Changes are saved right away.")],
  plugins: [t("Plugins"), t("Stream Deck plugins: OBS, Spotify, Discord, PC metrics and hundreds more.")],
  profiles: [t("Profiles"), t("Each profile has its own 15 keys. A profile can switch on its own based on the program in the foreground.")],
  icons: [t("Icons"), t("Open-source icon catalogue, your own images and Stream Deck icon packs.")],
  settings: [t("Settings"), t("Brightness, start with Windows, key calibration and boot logo.")],
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
    case 'macro': return t("Macro ({0} steps)", a.steps.length);
    case 'launch': return t("Launch {0}", a.target);
    case 'sound': return a.mode === 'stopAll' ? t("Stop all sounds") : a.sound ? t("Sound: {0}", a.name || a.sound) : t("Sound (no file)");
    case 'media': return MEDIA.find(([v]) => v === a.command)?.[1] ?? t("Media");
    case 'system': return SYSTEM.find(([v]) => v === a.command)?.[1] ?? t("System");
    case 'plugin': return pluginAction(a.plugin, a.action).action?.name ?? 'Plugin';
    case 'usage': return `${a.provider === 'claude' ? 'Claude' : 'GPT'} – ${a.window === 'weekly' ? t("week") : t("5h window")}`;
    case 'profile': return a.profileId === '__back' ? t("Previous profile") : t("Profile: {0}", config.profiles.find((p) => p.id === a.profileId)?.name ?? '?');
    default: return t("No action");
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
  const soundIcon = cfg.action?.type === 'sound' && !cfg.image;
  if (soundIcon) { const icon = svgIcon(ACTION_ICONS.sound); icon.classList.add('sound-icon'); btn.append(icon); }
  if (pa?.action?.icon) btn.append(h('img', { class: 'act-icon', src: pa.action.icon, alt: '' }));
  if (pa && !pa.plugin) btn.append(h('span', { class: 'lbl center' }, '⚠'));
  if (cfg.label) btn.append(h('span', { class: `lbl ${cfg.image || pa?.action?.icon || soundIcon ? 'bottom' : 'center'}` }, cfg.label));
  btn.append(h('span', { class: 'num' }, k));
}

function resetText(at) {
  const s = Math.round(at - Date.now() / 1000);
  if (s <= 0) return t("now");
  if (s < 60) return '<1 min';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ${String(Math.floor(s % 3600 / 60)).padStart(2, '0')}`;
  return `${Math.floor(s / 86400)} d ${Math.floor(s % 86400 / 3600)} h`;
}

/** Same layout as the key on the deck: name, percentage, bar, reset countdown. */
function usagePreview(cfg) {
  const a = cfg.action;
  const name = cfg.label || `${a.provider === 'claude' ? 'Claude' : 'GPT'} ${a.window === 'weekly' ? t("week") : '5h'}`;
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
      h('span', { class: 'u-reset' }, t("in {0}", resetText(w.resetsAt))));
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
  $('device-sub').textContent = state.connected === undefined ? t("Program not running")
    : `${state.connected ? t("Connected") : t("Not connected")} · ${state.activeProfileName ?? ''}`;
}

function renderProfileSelect() {
  const sel = $('profile-select');
  sel.replaceChildren(...config.profiles.map((p) => option(p.id, p.name, activeId())));
}

function renderBanners() {
  const b = [];
  const warn = (text) => h('div', { class: 'rm-banner' }, svgIcon('<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>'), text);
  if (state.conflicts?.length) b.push(warn(t("Fifine software is running ({0}), close it or it will fight over the deck.", state.conflicts.join(', '))));
  if (state.foregroundElevated) b.push(warn(t("The foreground window runs as administrator, keys from the deck won't reach it.")));
  if (state.connected === false) b.push(warn(t("The deck is not connected. Check the USB cable, the program reconnects on its own.")));
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
  const board = h('div', { class: 'deck-board' }, grid, h('div', { class: 'deck-caption' }, t("Profile “{0}”, laid out like the deck", activeProfile().name)));
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
  const thumb = h('div', { class: 'img-thumb' }, cfg.image ? '' : t("none"));
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
      } }, t("Choose image…")),
      cfg.image && h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', onclick: () => { cfg.image = null; refreshTile(k); save(); renderDeck(); } }, t("Remove"))),
    h('span', { class: 'rm-meta' }, t("Icon catalogue, your icons, packs… or drop an image here."))),
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
      h('h2', {}, t("Key {0}", k)),
      h('div', { class: 'rm-end' },
        h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => { showTemplates = !showTemplates; renderDeck(); } }, showTemplates ? t("Close templates") : t("Templates")),
        h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', onclick: () => {
          activeProfile().keys[String(k)] = { image: null, label: '', bg: '#141618', action: { type: 'none' } };
          save(); renderDeck();
        } }, t("Clear")))),
    h('div', { class: 'rm-panel-body rm-stack' },
      tplList,
      h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Image")), drop,
        ),
      h('div', { class: 'rm-row' },
        h('label', { class: 'rm-field rm-grow' }, h('span', { class: 'rm-label' }, t("Label")),
          h('input', { class: 'rm-input', value: cfg.label, maxLength: 40, placeholder: t("e.g. Fireball"), oninput: (e) => { cfg.label = e.target.value; refreshTile(k); save(); } })),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Background")),
          h('input', { class: 'rm-color', type: 'color', value: cfg.bg, oninput: (e) => { cfg.bg = e.target.value; refreshTile(k); save(); } }))),
      h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Action")),
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
  sound: '<path d="M11 4 6 8H3v8h3l5 4z"/><path d="M15 8a6 6 0 0 1 0 8M18 5a10 10 0 0 1 0 14"/>',
  media: '<path d="M9 18V6l11-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>',
  system: '<path d="M12 3v8"/><path d="M6.3 6.6a8 8 0 1 0 11.4 0"/>',
  profile: '<path d="M7 7h12l-3-3"/><path d="M17 17H5l3 3"/>',
  plugin: '<path d="M9 3v4M15 3v4M7 7h10v4a5 5 0 0 1-10 0z"/><path d="M12 16v5"/>',
  usage: '<path d="M4 15a8 8 0 0 1 16 0"/><path d="M12 15l4-5"/><path d="M4 19h16"/>',
};
const ACTION_SHORT = { none: t("None"), keys: t("Key"), macro: t("Macro"), launch: t("Launch"), sound: t("Sound"), media: t("Media"), system: t("System"), profile: t("Profile"), usage: t("AI usage"), plugin: 'Plugin' };

/** Grid of action types; the Plugin tile shows the chosen plugin action. */
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
    case 'sound': return { type, sound: '', name: '', me: 80, mic: 80, mode: 'toggle' };
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
    case 'sound': {
      const mode = h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Playback mode")),
        h('select', { class: 'rm-select', onchange: (e) => { a.mode = e.target.value; changed(); redraw(); } },
          [['toggle', t("Play / stop")], ['restart', t("Restart from beginning")], ['stopAll', t("Stop all sounds")]].map(([v, l]) => option(v, l, a.mode))));
      if (a.mode === 'stopAll') return [mode];
      let busy = false;
      const input = h('input', { type: 'file', accept: '.mp3,.wav,.ogg,.flac', hidden: true,
        onchange: (e) => { const file = e.target.files[0]; if (file) importSound(file); e.target.value = ''; } });
      const pick = h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => input.click() }, t("Choose sound…"));
      const drop = h('div', { class: 'img-drop sound-drop',
        ondragover: (e) => { e.preventDefault(); drop.classList.add('drag'); },
        ondragleave: () => drop.classList.remove('drag'),
        ondrop: (e) => { e.preventDefault(); drop.classList.remove('drag'); const file = e.dataTransfer.files[0]; if (file) importSound(file); } },
        h('div', { class: 'rm-stack rm-grow', style: 'gap:6px;min-width:0' },
          h('span', { class: 'sound-name', title: a.name || '' }, a.name || t("No sound selected")),
          h('div', { class: 'rm-row' }, pick),
          h('span', { class: 'rm-meta' }, t("MP3, WAV, OGG or FLAC · up to 50 MB. Drop a sound here."))), input);
      async function importSound(file) {
        if (busy) return;
        busy = true; pick.disabled = true; pick.textContent = t("Importing…");
        try {
          const sound = await invoke('import_sound', new Uint8Array(await file.arrayBuffer()));
          if (cfg.action !== a) return;
          a.sound = sound; a.name = file.name; changed(); redraw();
        } catch (e) { toast(e); }
        finally { busy = false; pick.disabled = false; pick.textContent = t("Choose sound…"); }
      }
      const volume = (field, label) => {
        const value = h('span', { class: 'rm-meta' }, (a[field] ?? 80) + ' %');
        return h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, label),
          h('div', { class: 'rm-row' }, h('input', { class: 'rm-grow', type: 'range', min: 0, max: 100, value: a[field] ?? 80,
            oninput: (e) => { a[field] = +e.target.value; value.textContent = a[field] + ' %'; changed(); } }), value));
      };
      return [mode, drop,
        h('div', { class: 'rm-row' },
          h('button', { class: 'rm-btn is-sm', type: 'button', disabled: !a.sound, onclick: async () => {
            try { await saveNow(); await invoke('preview_sound', { sound: a.sound, volume: a.me ?? 80 }); } catch (e) { toast(e); }
          } }, t("▶ Test")),
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => invoke('stop_sounds').catch(toast) }, t("■ Stop"))),
        volume('me', t("Volume for you")), volume('mic', t("Volume into microphone")),
        h('div', { class: 'rm-meta' }, t("Microphone output needs a virtual microphone (Sonar / VB-Cable). Discord noise suppression (Krisp) may filter the sound."))];
    }
    case 'keys': {
      const recBtn = h('button', { class: 'rm-btn is-sm', type: 'button' }, t("Record shortcut"));
      recBtn.onclick = () => {
        if (recording) { stopRecording(); recBtn.classList.remove('is-recording'); recBtn.textContent = t("Record shortcut"); return; }
        recBtn.classList.add('is-recording');
        recBtn.textContent = t("Press the shortcut on your keyboard…");
        startRecording((codes) => { a.codes = sortCodes(codes).slice(0, 6); changed(); redraw(); });
      };
      const chips = h('div', { class: 'chips' },
        a.codes.length ? a.codes.flatMap((c, i) => [
          i > 0 && h('span', { class: 'rm-plus' }, '+'),
          h('span', { class: 'rm-chip', title: c }, codeName(c),
            h('button', { type: 'button', title: t("Remove"), onclick: () => { a.codes.splice(i, 1); changed(); redraw(); } }, '×')),
        ]) : h('span', { class: 'rm-meta' }, t("No key yet.")));
      return [
        h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Keys (pressed together and held while you hold the deck key)")), chips),
        h('div', { class: 'rm-row' }, recBtn,
          h('div', { class: 'rm-grow' }, codeSelect('', (c) => { if (c && !a.codes.includes(c) && a.codes.length < 6) { a.codes = sortCodes([...a.codes, c]); changed(); redraw(); } }, t("+ add a key from the list")))),
        h('div', { class: 'rm-meta' }, t("Keys are stored by their position on the keyboard. Add shortcuts that Windows catches first (Win+L, Ctrl+Alt+Del) from the list.")),
      ];
    }
    case 'macro': {
      const hasText = a.steps.some((s) => s.type === 'text');
      const rows = a.steps.map((s, i) => {
        let param;
        if (s.type === 'wait') param = h('input', { class: 'rm-input', type: 'number', min: 0, max: 60000, step: 10, value: s.ms, oninput: (e) => { s.ms = Math.max(0, Math.min(60000, parseInt(e.target.value, 10) || 0)); changed(); } });
        else if (s.type === 'text') param = h('input', { class: 'rm-input', value: s.text, placeholder: t("Text to type"), oninput: (e) => { s.text = e.target.value; changed(); } });
        else param = codeSelect(s.code, (c) => { s.code = c; changed(); });
        return h('div', { class: 'step' },
          h('select', { class: 'rm-select', onchange: (e) => {
            const t = e.target.value;
            a.steps[i] = t === 'wait' ? { type: t, ms: 100 } : t === 'text' ? { type: t, text: '' } : { type: t, code: s.code ?? 'Digit1' };
            changed(); redraw();
          } }, STEP_TYPES.map(([v, l]) => option(v, l, s.type))),
          param,
          h('div', { class: 'step-tools' },
            h('button', { class: 'rm-icon-btn', type: 'button', title: t("Up"), disabled: i === 0, onclick: () => { [a.steps[i - 1], a.steps[i]] = [a.steps[i], a.steps[i - 1]]; changed(); redraw(); } }, '↑'),
            h('button', { class: 'rm-icon-btn', type: 'button', title: t("Down"), disabled: i === a.steps.length - 1, onclick: () => { [a.steps[i + 1], a.steps[i]] = [a.steps[i], a.steps[i + 1]]; changed(); redraw(); } }, '↓'),
            h('button', { class: 'rm-icon-btn is-danger', type: 'button', title: t("Delete step"), disabled: a.steps.length === 1, onclick: () => { a.steps.splice(i, 1); changed(); redraw(); } }, '×')));
      });
      return [
        h('div', { class: 'rm-note is-warn' }, WOW_NOTE),
        h('div', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Steps (run once per press)")), h('div', { class: 'steps' }, rows)),
        hasText && h('div', { class: 'rm-note' }, TEXT_NOTE),
        h('div', { class: 'rm-row' },
          h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => { a.steps.push({ type: 'tap', code: 'Digit1' }); changed(); redraw(); } }, t("+ Step")),
          h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => { a.steps.push({ type: 'wait', ms: 100 }); changed(); redraw(); } }, t("+ Wait"))),
      ];
    }
    case 'launch':
      return [
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Program, file, folder or web address")),
          h('input', { class: 'rm-input', value: a.target, placeholder: 'C:\\Program Files\\…\\program.exe  nebo  https://…', oninput: (e) => { a.target = e.target.value; changed(); } })),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Arguments (optional, programs only)")),
          h('input', { class: 'rm-input', value: joinArgs(a.args), oninput: (e) => { a.args = splitArgs(e.target.value); changed(); } })),
      ];
    case 'media':
      return [h('select', { class: 'rm-select', onchange: (e) => { a.command = e.target.value; changed(); } }, MEDIA.map(([v, l]) => option(v, l, a.command)))];
    case 'system':
      return [h('select', { class: 'rm-select', onchange: (e) => { a.command = e.target.value; changed(); } }, SYSTEM.map(([v, l]) => option(v, l, a.command)))];
    case 'usage':
      return [
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, t("Service")),
          h('select', { class: 'rm-select', onchange: (e) => { a.provider = e.target.value; changed(); } }, USAGE_PROVIDERS.map(([v, l]) => option(v, l, a.provider)))),
        h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, 'Limit'),
          h('select', { class: 'rm-select', onchange: (e) => { a.window = e.target.value; changed(); } }, USAGE_WINDOWS.map(([v, l]) => option(v, l, a.window)))),
        h('div', { class: 'rm-meta' }, t("The key shows the percentage and the time until reset, refreshed every minute and right away on press. It uses the login from Claude Code / Codex: they don't have to run, you just need to be logged in. The label replaces the name at the top.")),
      ];
    case 'profile':
      return [h('select', { class: 'rm-select', onchange: (e) => { a.profileId = e.target.value; changed(); } },
        option('__back', t("Previous profile"), a.profileId),
        config.profiles.map((p) => option(p.id, p.name, a.profileId)))];
    default:
      return [h('div', { class: 'rm-meta' }, t("The key does nothing. Pick an action or use a template."))];
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
    const procInput = h('input', { class: 'rm-input', placeholder: t("e.g. wow.exe"), list: 'proc-list' });
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
        isDefault && h('span', { class: 'prof-badge' }, t("Default")),
        isActive && h('span', { class: 'prof-badge' }, t("Active")),
        h('div', { class: 'rm-end' },
          !isDefault && h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => { config.defaultProfileId = p.id; save(); renderProfiles(); } }, t("Set as default")),
          !isActive && h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => switchProfile(p.id) }, t("Show on deck")),
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => {
            const copy = structuredClone(p);
            copy.id = newId(); copy.name = `${p.name} (kopie)`; copy.processes = [];
            config.profiles.splice(config.profiles.indexOf(p) + 1, 0, copy);
            save(); renderProfiles(); renderProfileSelect();
          } }, t("Duplicate")),
          h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', disabled: config.profiles.length === 1, onclick: () => {
            if (!delArmed) { pendingDelete = p.id; renderProfiles(); setTimeout(() => { if (pendingDelete === p.id) { pendingDelete = null; renderProfiles(); } }, 3000); return; }
            pendingDelete = null;
            config.profiles = config.profiles.filter((x) => x.id !== p.id);
            if (config.defaultProfileId === p.id) config.defaultProfileId = config.profiles[0].id;
            if (config.activeProfileId === p.id) config.activeProfileId = config.defaultProfileId;
            if (state.activeProfileId === p.id) state.activeProfileId = config.activeProfileId;
            save(); renderProfiles(); renderProfileSelect();
          } }, delArmed ? t("Really delete?") : t("Delete")))),
      h('div', { class: 'rm-panel-body rm-stack' },
        h('div', { class: 'rm-field' },
          h('span', { class: 'rm-label' }, t("Switch automatically when this program is in the foreground:")),
          h('div', { class: 'chips' },
            p.processes.length ? p.processes.map((proc, i) => h('span', { class: 'rm-chip' }, proc,
              h('button', { type: 'button', title: t("Remove"), onclick: () => { p.processes.splice(i, 1); save(); renderProfiles(); } }, '×')))
              : h('span', { class: 'rm-meta' }, isDefault ? t("The default profile is used when no rule matches.") : t("No rule, the profile is only switched manually.")))),
        h('div', { class: 'rm-row' },
          h('div', { class: 'rm-grow' }, procInput),
          h('button', { class: 'rm-btn is-sm', type: 'button', onclick: addProc }, t("Add"))))));
  }
  $('page-profiles').replaceChildren(
    h('div', { class: 'rm-row', style: 'margin-bottom:12px' },
      h('button', { class: 'rm-btn is-primary', type: 'button', onclick: () => {
        const keys = {};
        for (let k = 1; k <= 15; k++) keys[k] = { image: null, label: '', bg: '#141618', action: { type: 'none' } };
        config.profiles.push({ id: newId(), name: t("New profile"), processes: [], keys });
        save(); renderProfiles(); renderProfileSelect();
      } }, t("+ New profile")),
      h('button', { class: 'rm-btn is-ghost', type: 'button', onclick: loadProcesses }, t("Load running programs")),
      h('span', { class: 'rm-meta' }, processNames.length ? t("The list has {0} running programs.", processNames.length) : t("Tip: start the game and click “Load running programs”."))),
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
    h('div', {}, h('div', { class: 'rm-stack', style: 'gap:2px' }, h('b', {}, t('Language')), h('span', { class: 'rm-meta' }, 'English / Čeština')),
      h('select', { class: 'rm-select', style: 'width:auto;min-width:160px', onchange: async (e) => {
        config.language = e.target.value;
        await saveNow();
        setLang(config.language);
        location.reload();
      } }, option('en', 'English', config.language ?? 'en'), option('cs', 'Čeština', config.language ?? 'en'))),
    h('div', {}, h('b', {}, t("Display brightness")),
      h('div', { class: 'rm-row' }, h('input', { type: 'range', min: 0, max: 100, value: config.brightness, oninput: (e) => { config.brightness = +e.target.value; bright.textContent = `${config.brightness} %`; save(); } }), bright)),
    h('div', {}, h('div', { class: 'rm-stack', style: 'gap:2px' }, h('b', {}, t("Start with Windows")), h('span', { class: 'rm-meta' }, t("The program starts hidden in the notification area."))), sw),
    h('div', {}, h('div', { class: 'rm-stack', style: 'gap:2px' }, h('b', {}, t("Key calibration")), h('span', { class: 'rm-meta' }, config.keyMap ? t("Custom calibration in use.") : t("Default layout (verified on your deck)."))),
      h('div', { class: 'rm-row' },
        config.keyMap && h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => { config.keyMap = null; save(); renderSettings(); } }, t("Reset to default")),
        h('button', { class: 'rm-btn is-sm', type: 'button', onclick: startCalibration }, t("Start calibration")))),
    h('div', {}, h('b', {}, 'Data'), h('code', {}, '%APPDATA%\\GerzysDeck')));

  const parts = [h('div', { class: 'rm-panel' }, h('div', { class: 'rm-panel-body', style: 'padding-top:8px' }, rows)), soundboardPanel(), bootLogoPanel()];
  if (calibration) parts.push(calibrationPanel());
  $('page-settings').replaceChildren(h('div', { class: 'rm-stack' }, parts));
}

// Audio devices load when Settings opens; saved disconnected outputs stay selectable.
let soundOutputs = null;
let soundAutoMic = null;
let soundOutputsLoading = false;
function soundboardPanel() {
  const deviceSelect = (field, label, fallback, initial) => {
    const selected = config[field] ?? initial;
    const devices = soundOutputs ?? [];
    return h('label', { class: 'rm-field' }, h('span', { class: 'rm-label' }, label),
      h('select', { class: 'rm-select', onchange: (e) => { config[field] = e.target.value; save(); } },
        fallback.map(([v, l]) => option(v, l, selected)),
        devices.filter((d) => !fallback.some(([v]) => v === d)).map((d) => option(d, d, selected)),
        !fallback.some(([v]) => v === selected) && !devices.includes(selected) && option(selected, soundOutputs === null ? selected : t("{0} (not connected)", selected), selected)));
  };
  if (soundOutputs === null && !soundOutputsLoading) {
    soundOutputsLoading = true;
    Promise.allSettled([invoke('list_audio_outputs'), invoke('auto_mic_device')]).then(([outputs, mic]) => {
      soundOutputs = outputs.status === 'fulfilled' ? [...new Set(outputs.value)] : [];
      soundAutoMic = mic.status === 'fulfilled' ? mic.value : null;
      soundOutputsLoading = false;
      if (outputs.status === 'rejected') toast(outputs.reason);
      if (mic.status === 'rejected') toast(mic.reason);
      if (page === 'settings') renderSettings();
    });
  }
  return h('div', { class: 'rm-panel' }, h('div', { class: 'rm-panel-head' }, h('h2', {}, t("Soundboard"))),
    h('div', { class: 'rm-panel-body rm-stack' },
      deviceSelect('soundMeDevice', t("Output for you"), [['', t("Windows default")]], ''),
      deviceSelect('soundMicDevice', t("Output into microphone"), [['auto', t("Auto ({0})", soundOutputsLoading ? t("Loading…") : soundAutoMic || t("not found"))], ['none', t("Off")]], 'auto')));
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
      toast(t("The boot logo is stored in the deck. It shows the next time you plug it in."));
    } catch (e) { toast(e); }
    bootLogo.busy = false;
    renderSettings();
  }
  const preview = h('div', { class: 'boot-prev',
    ondragover: (e) => { e.preventDefault(); },
    ondrop: (e) => { e.preventDefault(); const f = e.dataTransfer.files[0]; if (f?.type.startsWith('image/')) pickBoot(f); } },
  bootLogo.url ? h('img', { src: bootLogo.url, alt: '' }) : h('span', { class: 'rm-meta' }, t("Drop an image here")));
  return h('div', { class: 'rm-panel' },
    h('div', { class: 'rm-panel-head' }, h('h2', {}, 'Boot logo')),
    h('div', { class: 'rm-panel-body rm-stack' },
      h('span', { class: 'rm-meta' }, t("The image the deck shows when plugged in, before the program connects. It is cropped to 800 × 480 and stored in the deck itself.")),
      preview,
      h('div', { class: 'rm-row', style: 'flex-wrap:wrap' },
        h('button', { class: 'rm-btn is-sm', type: 'button', disabled: bootLogo.busy, onclick: () => input.click() }, t("Choose image…")),
        h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', disabled: bootLogo.busy, onclick: useDefault }, t("Gerzy's deck logo")),
        h('span', { class: 'rm-grow' }),
        h('button', { class: 'rm-btn is-sm is-primary', type: 'button', disabled: !bootLogo.file || bootLogo.busy || state.connected === false, onclick: upload },
          bootLogo.busy ? t("Uploading… (about 7 s)") : t("Upload to deck"))),
      bootLogo.busy && h('div', { class: 'rm-note is-info' }, t("Don't unplug the deck while uploading.")),
      input));
}

async function startCalibration() {
  try { await invoke('set_calibrating', { on: true }); } catch (e) { toast(e); return; }
  calibration = { step: 1, map: new Array(15).fill(0), used: new Set() };
  renderSettings();
}

function endCalibration(saveIt) {
  if (saveIt) { config.keyMap = calibration.map; save(); toast(t("Calibration saved.")); }
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
      h('div', { class: 'rm-end' }, h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => endCalibration(false) }, t("Cancel")))),
    h('div', { class: 'rm-panel-body rm-stack' },
      h('div', { class: 'rm-note is-info' }, t("Press the key on the deck that is highlighted here. Keys don't run anything during calibration.")),
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
  for (const el of document.querySelectorAll('[data-i18n]')) el.textContent = t(el.dataset.i18n);
  document.documentElement.lang = lang;
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
    $('page-deck').replaceChildren(h('div', { class: 'rm-note is-warn' }, t("Could not connect to the program core: {0}", e)));
    return;
  }
  // the config is the source of truth for the language; modules translated with the cached one
  if ((config.language ?? 'en') !== lang) { setLang(config.language ?? 'en'); location.reload(); return; }
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
