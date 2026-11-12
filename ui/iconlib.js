// Icon library: Iconify catalogue, my icons, imported Stream Deck icon packs, built-in icons.
import { ICONS, rasterizeIcon } from './icons.js';
import { t, plural } from './i18n.js';

const ICONIFY = 'https://api.iconify.design';
const SETS = [
  ['', t("All sets")],
  ['simple-icons,logos', t("Brand logos")],
  ['game-icons', t("Game icons")],
  ['mdi', 'Material Design'],
  ['lucide', 'Lucide'],
  ['ph', 'Phosphor'],
  ['tabler', 'Tabler'],
  ['fluent-emoji-flat', 'Emoji'],
];
const TABS = [['catalog', t("Catalogue")], ['library', t("My icons")], ['packs', t("Packs")], ['builtin', t("Built-in")]];

// remembered between openings
const st = { tab: 'catalog', q: '', set: '', color: '#ffffff', bg: '#000000', noBg: false, pack: null };

/** Any image URL (svg/png/…) → 200×200 PNG bytes. Optional bg + scale for catalogue glyphs. */
async function toPng(url, { bg = null, scale = 1 } = {}) {
  const blob = await (await fetch(url)).blob();
  const obj = URL.createObjectURL(blob);
  try {
    const img = new Image();
    img.src = obj;
    await img.decode();
    const size = 200;
    const c = document.createElement('canvas');
    c.width = c.height = size;
    const g = c.getContext('2d');
    if (bg) { g.fillStyle = bg; g.fillRect(0, 0, size, size); }
    const w = img.naturalWidth || size, hgt = img.naturalHeight || size;
    const fit = (size * scale) / Math.max(w, hgt);
    const dw = w * fit, dh = hgt * fit;
    g.drawImage(img, (size - dw) / 2, (size - dh) / 2, dw, dh);
    const out = await new Promise((r) => c.toBlob(r, 'image/png'));
    return new Uint8Array(await out.arrayBuffer());
  } finally { URL.revokeObjectURL(obj); }
}

// icon bodies fetched as JSON (one request per icon set); SVGs are built locally
const bodies = new Map(); // "prefix:name" → { body, width, height, left, top }

async function loadBodies(ids) {
  const byPrefix = {};
  for (const id of ids) {
    if (bodies.has(id)) continue;
    const [prefix, name] = id.split(':');
    (byPrefix[prefix] ||= []).push(name);
  }
  await Promise.all(Object.entries(byPrefix).map(async ([prefix, names]) => {
    const res = await fetch(`${ICONIFY}/${prefix}.json?icons=${names.map(encodeURIComponent).join(',')}`);
    if (!res.ok) return;
    const data = await res.json();
    const resolve = (name, depth = 0) => {
      if (data.icons?.[name]) return { ...data.icons[name] };
      const alias = data.aliases?.[name];
      if (!alias || depth > 5) return null;
      const parent = resolve(alias.parent, depth + 1);
      return parent && { ...parent, ...alias, body: parent.body };
    };
    for (const name of names) {
      const icon = resolve(name);
      if (icon) bodies.set(`${prefix}:${name}`, { left: data.left ?? 0, top: data.top ?? 0, width: data.width ?? 16, height: data.height ?? 16, ...icon });
    }
  }));
}

const catalogSvg = (id) => {
  const icon = bodies.get(id);
  if (!icon) return null;
  const { left = 0, top = 0, width = 16, height = 16 } = icon;
  const body = icon.body.replace(/currentColor/g, st.color);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="${left} ${top} ${width} ${height}" width="${width * 8}" height="${height * 8}">${body}</svg>`;
  return 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);
};

const ikon = (n) => plural(n, 'icon', 'icons', 'ikona', 'ikony', 'ikon');
const b64 = (text) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));

/**
 * The icon browser. mode 'pick' → onPick(pngBytes) when an icon is chosen;
 * mode 'page' → management (upload, import packs, delete, save catalogue icons to t("My icons")).
 */
export function iconBrowser(ui, { mode, onPick }) {
  const { h, invoke, toast } = ui;
  const root = h('div', { class: 'rm-stack ilib' });
  let searchTimer = 0;
  let searchSeq = 0;

  const pick = async (fn) => {
    try { const bytes = await fn(); if (bytes) await onPick(bytes); } catch (e) { toast(e?.message || e); }
  };

  const tabs = () => h('div', { class: 'rm-theme-switch ilib-tabs' },
    TABS.map(([id, label]) => h('button', { type: 'button', 'aria-pressed': String(st.tab === id), onclick: () => { st.tab = id; draw(); } }, label)));

  const tile = (src, title, onclick, extra) => h('div', { class: 'ilib-cell' },
    h('button', { class: 'icon-pick ilib-tile', type: 'button', title, onclick }, h('img', { src, alt: title, loading: 'lazy' })),
    extra || '');

  const del = (onclick) => h('button', { class: 'ilib-del', type: 'button', title: t("Delete"), onclick }, '×');

  // ---- catalogue ----
  function catalog() {
    const results = h('div', { class: 'icon-grid ilib-grid' });
    const meta = h('div', { class: 'rm-meta' });
    const run = async () => {
      const q = st.q.trim();
      const seq = ++searchSeq;
      if (!q) { results.replaceChildren(); meta.textContent = t("Type what you're looking for, e.g. “discord”, “microphone”, “sword” or “fire”. Search is in English."); return; }
      meta.textContent = t("Searching…");
      try {
        const params = new URLSearchParams({ query: q, limit: '120' });
        if (st.set) params.set('prefixes', st.set);
        const res = await fetch(`${ICONIFY}/search?${params}`);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        await loadBodies(data.icons);
        if (seq !== searchSeq) return;
        meta.textContent = data.icons.length ? `${data.total > data.icons.length ? t("First {0} of {1}", data.icons.length, data.total) : ikon(data.icons.length)}` : t("Nothing found.");
        results.replaceChildren(...data.icons.filter((id) => bodies.has(id)).map((id) => {
          const src = catalogSvg(id);
          const name = id.split(':')[1].replace(/-/g, ' ');
          if (mode === 'pick') return tile(src, id, () => pick(() => toPng(src, { bg: st.noBg ? null : st.bg, scale: 0.62 })));
          return tile(src, id, async () => {
            try {
              await invoke('icons_upload', await toPng(src, { bg: st.noBg ? null : st.bg, scale: 0.62 }), { headers: { 'x-name': b64(name) } });
              toast(t("Saved to My icons: {0}", name));
            } catch (e) { toast(e?.message || e); }
          });
        }));
      } catch (e) {
        if (seq === searchSeq) meta.textContent = t("Catalogue unavailable ({0}). Are you connected to the internet?", e.message || e);
      }
    };
    const search = h('input', { class: 'rm-input rm-grow', type: 'search', placeholder: t("Search icons…"), value: st.q,
      oninput: (e) => { st.q = e.target.value; clearTimeout(searchTimer); searchTimer = setTimeout(run, 300); } });
    const set = h('select', { class: 'rm-select ilib-set', onchange: (e) => { st.set = e.target.value; run(); } },
      SETS.map(([v, l]) => h('option', { value: v, selected: v === st.set }, l)));
    const color = h('input', { class: 'rm-color', type: 'color', value: st.color, title: t("Symbol colour"),
      onchange: (e) => { st.color = e.target.value; run(); } });
    const bg = h('input', { class: 'rm-color', type: 'color', value: st.bg, title: t("Key background"),
      disabled: st.noBg, onchange: (e) => { st.bg = e.target.value; } });
    const noBg = h('input', { type: 'checkbox', checked: st.noBg, onchange: (e) => { st.noBg = e.target.checked; bg.disabled = st.noBg; } });
    setTimeout(run);
    setTimeout(() => search.focus());
    return [
      h('div', { class: 'rm-row ilib-bar' }, search, set),
      h('div', { class: 'rm-row ilib-bar' },
        h('label', { class: 'rm-row ilib-opt' }, color, h('span', { class: 'rm-meta' }, 'Symbol')),
        h('label', { class: 'rm-row ilib-opt' }, bg, h('span', { class: 'rm-meta' }, t("Background"))),
        h('label', { class: 'rm-row ilib-opt', title: t("Transparent icon: the key's background colour shows behind it") }, noBg, h('span', { class: 'rm-meta' }, t("No background"))),
        h('span', { class: 'rm-meta rm-grow', style: 'text-align:end' }, mode === 'pick' ? t("Click an icon to put it on the key.") : t("Click an icon to save it to My icons."))),
      meta, results,
      h('div', { class: 'rm-meta ilib-credit' }, t('Icons from '), h('a', { href: 'https://iconify.design', target: '_blank' }, 'Iconify'), t(", open-source sets, each with its own license.")),
    ];
  }

  // ---- my icons ----
  function library() {
    const grid = h('div', { class: 'icon-grid ilib-grid' });
    const meta = h('div', { class: 'rm-meta' }, t("Loading…"));
    const upload = h('input', { type: 'file', accept: 'image/*', multiple: true, hidden: true, onchange: (e) => addFiles([...e.target.files]) });
    async function addFiles(files) {
      for (const f of files) {
        try { await invoke('icons_upload', new Uint8Array(await f.arrayBuffer()), { headers: { 'x-name': b64(f.name.replace(/\.[^.]+$/, '')) } }); } catch (e) { toast(`${f.name}: ${e}`); }
      }
      load();
    }
    async function load() {
      try {
        const icons = await invoke('icons_library');
        meta.textContent = icons.length ? ikon(icons.length) : t("Nothing here yet. Upload images or save them from the catalogue.");
        grid.replaceChildren(...icons.map((i) => tile(i.url, i.name,
          mode === 'pick' ? () => pick(() => toPng(i.url)) : () => {},
          mode === 'page' && del(async () => { try { await invoke('icons_delete', { id: i.id }); load(); } catch (e) { toast(e); } }))));
      } catch (e) { meta.textContent = String(e); }
    }
    load();
    const drop = h('div', { class: 'img-drop ilib-drop',
      ondragover: (e) => { e.preventDefault(); drop.classList.add('drag'); },
      ondragleave: () => drop.classList.remove('drag'),
      ondrop: (e) => { e.preventDefault(); drop.classList.remove('drag'); addFiles([...e.dataTransfer.files].filter((f) => f.type.startsWith('image/'))); } },
    h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => upload.click() }, t("Upload images…")),
    h('span', { class: 'rm-meta' }, t("or drop them here (several at once)")), upload);
    return [drop, meta, grid];
  }

  // ---- packs ----
  function packs() {
    const body = h('div', { class: 'rm-stack' });
    const input = h('input', { type: 'file', accept: '.streamDeckIconPack,.zip', hidden: true, onchange: (e) => importPack(e.target.files[0]) });
    async function importPack(file) {
      if (!file) return;
      try {
        const p = await invoke('icon_pack_import', new Uint8Array(await file.arrayBuffer()));
        toast(t("Pack “{0}” added ({1})", p.name, ikon(p.count)));
        st.pack = p.id;
        draw();
      } catch (e) { toast(e); }
    }
    async function load() {
      try {
        const list = await invoke('icon_packs');
        if (st.pack && list.some((p) => p.id === st.pack)) return openPack(list.find((p) => p.id === st.pack));
        st.pack = null;
        body.replaceChildren(list.length
          ? h('div', { class: 'plug-grid' }, list.map((p) => h('div', { class: 'rm-panel plug-card ilib-pack', role: 'button', tabindex: '0', onclick: () => { st.pack = p.id; openPack(p); } },
            p.iconUrl ? h('img', { class: 'plug-icon', src: p.iconUrl, alt: '' }) : h('div', { class: 'plug-icon' }),
            h('div', { class: 'plug-text' }, h('strong', {}, p.name), h('span', { class: 'rm-meta' }, `${ikon(p.count)}${p.author ? ` · ${p.author}` : ''}`)))))
          : h('div', { class: 'rm-meta' }, t("No packs. Stream Deck icon packs (.streamDeckIconPack files) are available on the Elgato Marketplace or from their authors.")));
      } catch (e) { body.replaceChildren(h('div', { class: 'rm-meta' }, String(e))); }
    }
    async function openPack(p) {
      let q = '';
      const grid = h('div', { class: 'icon-grid ilib-grid' });
      const icons = await invoke('icon_pack_icons', { id: p.id }).catch((e) => { toast(e); return []; });
      const drawIcons = () => {
        const qq = q.trim().toLowerCase();
        const shown = icons.filter((i) => !qq || i.name.toLowerCase().includes(qq) || i.tags.some((t) => t.toLowerCase().includes(qq)));
        grid.replaceChildren(...shown.slice(0, 600).map((i) => tile(i.url, i.name, mode === 'pick' ? () => pick(() => toPng(i.url)) : () => {})));
      };
      body.replaceChildren(
        h('div', { class: 'rm-row ilib-bar' },
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => { st.pack = null; load(); } }, t("‹ Packs")),
          h('strong', { class: 'rm-grow' }, p.name),
          mode === 'page' && h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', onclick: async () => {
            if (!confirm(t("Delete pack “{0}”?", p.name))) return;
            try { await invoke('icon_pack_delete', { id: p.id }); st.pack = null; load(); } catch (e) { toast(e); }
          } }, t("Delete pack"))),
        h('input', { class: 'rm-input', type: 'search', placeholder: t("Search {0} icons…", icons.length), oninput: (e) => { q = e.target.value; drawIcons(); } }),
        grid);
      drawIcons();
    }
    load();
    return [
      h('div', { class: 'rm-row ilib-bar' },
        h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => input.click() }, t("Import pack…")),
        h('span', { class: 'rm-meta' }, '.streamDeckIconPack'), input),
      body,
    ];
  }

  // ---- built-in ----
  function builtin() {
    return [h('div', { class: 'icon-grid ilib-grid' }, ICONS.map((icon) => tile(
      'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(icon.svg), icon.name,
      mode === 'pick' ? () => pick(() => rasterizeIcon(icon)) : () => {})))];
  }

  function draw() {
    const view = { catalog, library, packs, builtin }[st.tab]();
    root.replaceChildren(tabs(), ...view);
  }
  draw();
  return root;
}

/** Modal icon chooser for a key; resolves with PNG bytes, or 'file' for "from disk", or null. */
export function pickIcon(ui) {
  const { h } = ui;
  return new Promise((resolve) => {
    const close = (v) => { overlay.remove(); resolve(v); };
    const overlay = h('div', { class: 'modal-back', onclick: (e) => { if (e.target === overlay) close(null); } },
      h('div', { class: 'rm-panel modal ilib-modal' },
        h('div', { class: 'rm-panel-head' }, h('h2', {}, t("Choose image")),
          h('div', { class: 'rm-end' },
            h('button', { class: 'rm-btn is-sm', type: 'button', onclick: () => close('file') }, t("From file…")),
            h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => close(null) }, t("Close")))),
        h('div', { class: 'rm-panel-body' }, iconBrowser(ui, { mode: 'pick', onPick: (bytes) => close(bytes) }))));
    document.body.append(overlay);
  });
}

export function renderIconsPage(ui, el) {
  el.replaceChildren(ui.h('div', { class: 'rm-panel' }, ui.h('div', { class: 'rm-panel-body' }, iconBrowser(ui, { mode: 'page' }))));
}
