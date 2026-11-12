// Plugins: page (installed + catalogues), action picker, property-inspector panel.
// `ui` is supplied by app.js: { h, invoke, toast, svgIcon, getConfig, save, renderDeck }.

const WARNING = 'Pluginy jsou programy třetích stran a mají plný přístup k tvému počítači. Instaluj jen ty, kterým věříš.';
const STATUS = {
  running: ['Běží', 'ok'],
  starting: ['Startuje…', 'warn'],
  crashed: ['Spadl', 'bad'],
  missingNode: ['Chybí Node.js', 'bad'],
  notInstalled: ['Není nainstalovaný', 'bad'],
};

let installed = null; // [{ id, name, version, author, icon, status, actions:[{uuid,name,icon,tooltip}] }]
let catalog = null; // { entries:[{id,name,author,source,iconUrl}], errors:[] }
let query = '';
let busy = new Set();
let pendingRemove = null;

export async function loadInstalled(ui) {
  try { installed = await ui.invoke('plugins_list'); } catch (e) { installed = []; ui.toast(e); }
  return installed;
}

export const installedPlugins = () => installed ?? [];

export function pluginAction(pluginId, actionUuid) {
  const p = installedPlugins().find((x) => x.id === pluginId);
  return { plugin: p, action: p?.actions.find((a) => a.uuid === actionUuid) };
}

function statusChip(ui, status) {
  const [label, tone] = STATUS[status?.kind] ?? ['?', 'warn'];
  const extra = status?.kind === 'crashed' && status.restarts ? ` (${status.restarts}×)` : '';
  return ui.h('span', { class: `chip is-${tone}` }, label + extra);
}

// ---------- page ----------
let poll = null;

export async function renderPluginsPage(ui, root) {
  // keep statuses fresh while the page is visible (Startuje… → Běží)
  clearInterval(poll);
  poll = setInterval(async () => {
    if (root.hidden || !root.isConnected) { clearInterval(poll); poll = null; return; }
    const before = JSON.stringify(installed);
    await loadInstalled(ui);
    const typing = document.activeElement?.tagName === 'INPUT' && root.contains(document.activeElement);
    if (JSON.stringify(installed) !== before && !typing) draw(ui, root);
  }, 2000);
  if (!installed) await loadInstalled(ui);
  draw(ui, root);
  if (!catalog) {
    try { catalog = await ui.invoke('plugins_catalog'); } catch (e) { catalog = { entries: [], errors: [String(e)] }; }
    draw(ui, root);
  }
}

function draw(ui, root) {
  const { h } = ui;
  const installedIds = new Set(installedPlugins().map((p) => p.id.replace(/\.sdPlugin$/i, '')));

  const fileInput = h('input', { type: 'file', accept: '.streamDeckPlugin,.zip', hidden: true, onchange: async (e) => {
    const f = e.target.files[0];
    if (!f) return;
    await withBusy(ui, root, 'file', async () => {
      await ui.invoke('plugins_install_file', new Uint8Array(await f.arrayBuffer()));
      ui.toast(`Plugin ${f.name} nainstalován.`);
    });
  } });

  const installedGrid = h('div', { class: 'plug-grid' },
    installedPlugins().length ? installedPlugins().map((p) => {
      const removing = pendingRemove === p.id;
      return h('div', { class: 'plug-card' },
        h('img', { class: 'plug-icon', src: p.icon, alt: '' }),
        h('div', { class: 'plug-text' },
          h('b', {}, p.name),
          h('span', { class: 'rm-meta' }, `${p.author || ''} · ${p.version || ''}`),
          h('div', { class: 'rm-row', style: 'gap:6px' }, statusChip(ui, p.status),
            h('span', { class: 'rm-meta' }, `${p.actions.length} akcí`))),
        h('div', { class: 'plug-actions' },
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => ui.invoke('open_plugin_log', { id: p.id }).catch(ui.toast) }, 'Log'),
          h('button', { class: 'rm-btn is-sm is-ghost is-danger', type: 'button', disabled: busy.has(p.id), onclick: async () => {
            if (!removing) { pendingRemove = p.id; draw(ui, root); setTimeout(() => { if (pendingRemove === p.id) { pendingRemove = null; draw(ui, root); } }, 3000); return; }
            pendingRemove = null;
            await withBusy(ui, root, p.id, () => ui.invoke('plugins_uninstall', { id: p.id }));
          } }, removing ? 'Opravdu?' : 'Odinstalovat')));
    }) : h('div', { class: 'rm-meta' }, 'Zatím žádný plugin. Vyber si z katalogu níž.'));

  const q = query.trim().toLowerCase();
  const matches = (e) => !q || e.name.toLowerCase().includes(q) || (e.author ?? '').toLowerCase().includes(q) || e.id.toLowerCase().includes(q);
  const section = (title, source) => {
    const list = (catalog?.entries ?? []).filter((e) => e.source === source && matches(e));
    return h('div', { class: 'rm-stack', style: 'gap:8px' },
      h('h3', { class: 'plug-h' }, title, h('span', { class: 'rm-meta' }, ` ${list.length}`)),
      h('div', { class: 'plug-grid' }, list.slice(0, q ? 200 : 60).map((e) => {
        const isIn = installedIds.has(e.id);
        return h('div', { class: 'plug-card' },
          h('img', { class: 'plug-icon', src: e.iconUrl, alt: '', loading: 'lazy', onerror: (ev) => { ev.target.style.visibility = 'hidden'; } }),
          h('div', { class: 'plug-text' }, h('b', {}, e.name), h('span', { class: 'rm-meta' }, e.author || '')),
          h('div', { class: 'plug-actions' },
            h('button', { class: `rm-btn is-sm ${isIn ? 'is-ghost' : ''}`, type: 'button', disabled: busy.has(e.id), onclick: () => install(ui, root, e) },
              busy.has(e.id) ? 'Instaluji…' : isIn ? 'Přeinstalovat' : 'Instalovat')));
      })),
      !q && list.length > 60 && h('div', { class: 'rm-meta' }, `…a dalších ${list.length - 60}. Použij hledání.`));
  };

  const search = h('input', { class: 'rm-input', type: 'search', placeholder: 'Hledat plugin (OBS, Spotify, Discord, CPU…)', value: query,
    oninput: (e) => { query = e.target.value; draw(ui, root); const s = root.querySelector('input[type=search]'); s.focus(); s.setSelectionRange(query.length, query.length); } });

  root.replaceChildren(h('div', { class: 'rm-stack' },
    h('div', { class: 'rm-panel' },
      h('div', { class: 'rm-panel-head' }, h('h2', {}, 'Nainstalované'),
        h('div', { class: 'rm-end' },
          h('button', { class: 'rm-btn is-sm', type: 'button', disabled: busy.has('file'), onclick: () => fileInput.click() }, 'Instalovat ze souboru…'),
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: async () => { await loadInstalled(ui); draw(ui, root); } }, 'Obnovit'))),
      h('div', { class: 'rm-panel-body' }, installedGrid, fileInput)),
    h('div', { class: 'rm-note is-warn' }, WARNING),
    search,
    catalog ? [
      catalog.errors?.length ? h('div', { class: 'rm-note is-warn' }, `Katalog se nepodařilo načíst celý: ${catalog.errors.join('; ')}`) : null,
      section('Open-source pluginy (OpenAction)', 'openAction'),
      section('Archiv obchodu Elgato', 'elgatoArchive'),
    ] : h('div', { class: 'rm-meta' }, 'Načítám katalogy…')));
}

async function withBusy(ui, root, id, fn) {
  busy.add(id);
  draw(ui, root);
  try {
    await fn();
    await loadInstalled(ui);
  } catch (e) {
    ui.toast(e);
  } finally {
    busy.delete(id);
    draw(ui, root);
  }
}

async function install(ui, root, entry) {
  await withBusy(ui, root, entry.id, async () => {
    let res = await ui.invoke('plugins_install', { id: entry.id, source: entry.source, assetUrl: null });
    if (res?.choose?.length) {
      const asset = await chooseAsset(ui, res.choose);
      if (!asset) return;
      res = await ui.invoke('plugins_install', { id: entry.id, source: entry.source, assetUrl: asset.url });
    }
    ui.toast(`Plugin ${entry.name} nainstalován.`);
  });
}

function chooseAsset(ui, assets) {
  const { h } = ui;
  return new Promise((resolve) => {
    const close = (v) => { overlay.remove(); resolve(v); };
    const overlay = h('div', { class: 'modal-back', onclick: (e) => { if (e.target === overlay) close(null); } },
      h('div', { class: 'rm-panel modal' },
        h('div', { class: 'rm-panel-head' }, h('h2', {}, 'Vyber soubor pluginu')),
        h('div', { class: 'rm-panel-body rm-stack', style: 'gap:6px' },
          assets.map((a) => h('button', { class: 'tpl-item', type: 'button', onclick: () => close(a) }, a.name)),
          h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => close(null) }, 'Zrušit'))));
    document.body.append(overlay);
  });
}

// ---------- action picker ----------
export function pickPluginAction(ui) {
  const { h } = ui;
  return new Promise((resolve) => {
    let q = '';
    const close = (v) => { overlay.remove(); resolve(v); };
    const open = new Set();
    const list = h('div', { class: 'rm-stack', style: 'gap:10px;max-height:60vh;overflow:auto' });
    const draw = () => {
      const qq = q.trim().toLowerCase();
      const groups = installedPlugins().map((p) => ({
        p,
        actions: p.actions.filter((a) => !qq || a.name.toLowerCase().includes(qq) || p.name.toLowerCase().includes(qq)),
      })).filter((g) => g.actions.length);
      // plugins start collapsed; a search expands every plugin with a match
      list.replaceChildren(...(groups.length ? groups.map(({ p, actions }) => {
        const isOpen = Boolean(qq) || open.has(p.id);
        return h('div', { class: 'rm-stack', style: 'gap:4px' },
          h('button', { class: `plug-group${isOpen ? ' open' : ''}`, type: 'button', 'aria-expanded': String(isOpen),
            onclick: () => { if (open.has(p.id)) open.delete(p.id); else open.add(p.id); draw(); } },
            h('img', { class: 'tpl-icon', src: p.icon, alt: '' }),
            h('span', { class: 'plug-group-name' }, p.name),
            h('span', { class: 'rm-meta' }, `${actions.length}`),
            h('span', { class: 'plug-chev' }, '›')),
          isOpen && actions.map((a) => h('button', { class: 'tpl-item', type: 'button', title: a.tooltip || '', onclick: () => close({ plugin: p.id, action: a.uuid }) },
            h('span', { class: 'tpl-name' }, h('img', { class: 'tpl-icon', src: a.icon, alt: '' }), a.name))));
      }) : [
        h('div', { class: 'rm-meta' }, installedPlugins().length ? 'Nic nenalezeno.' : 'Nemáš nainstalovaný žádný plugin. Najdeš je na stránce Pluginy.')]));
    };
    const search = h('input', { class: 'rm-input', type: 'search', placeholder: 'Hledat akci…', oninput: (e) => { q = e.target.value; draw(); } });
    const overlay = h('div', { class: 'modal-back', onclick: (e) => { if (e.target === overlay) close(null); } },
      h('div', { class: 'rm-panel modal' },
        h('div', { class: 'rm-panel-head' }, h('h2', {}, 'Akce z pluginu'),
          h('div', { class: 'rm-end' }, h('button', { class: 'rm-btn is-sm is-ghost', type: 'button', onclick: () => close(null) }, 'Zavřít'))),
        h('div', { class: 'rm-panel-body rm-stack' }, search, list)));
    document.body.append(overlay);
    draw();
    search.focus();
  });
}

// ---------- key panel for a plugin action ----------
let openPi = null; // context of the open property inspector

export function closePi(ui) {
  if (openPi) {
    ui.invoke('pi_close', { context: openPi }).catch(() => {});
    openPi = null;
  }
}

export function pluginActionFields(ui, profileId, key, cfg, onChange) {
  const { h } = ui;
  const a = cfg.action;
  const { plugin, action } = pluginAction(a.plugin, a.action);
  const head = h('div', { class: 'plug-current' },
    action?.icon ? h('img', { class: 'plug-icon', src: action.icon, alt: '' }) : null,
    h('div', { class: 'plug-text' },
      h('b', {}, action?.name ?? a.action),
      h('span', { class: 'rm-meta' }, plugin ? plugin.name : `Plugin ${a.plugin} není nainstalovaný`)),
    h('button', { class: 'rm-btn is-sm', type: 'button', onclick: async () => {
      const pick = await pickPluginAction(ui);
      if (pick) { cfg.action = { type: 'plugin', plugin: pick.plugin, action: pick.action, settings: {}, state: 0 }; onChange(true); }
    } }, 'Změnit'));
  if (!plugin) return [head, h('div', { class: 'rm-note is-warn' }, 'Plugin chybí. Nainstaluj ho znovu na stránce Pluginy a akce i nastavení se vrátí.')];

  const alignRow = h('label', { class: 'rm-field' },
    h('span', { class: 'rm-label' }, 'Pozice hodnoty na tlačítku'),
    h('select', { class: 'rm-select', onchange: (e) => { cfg.titleAlign = e.target.value || null; onChange(false); } },
      [['', 'Podle pluginu'], ['top', 'Nahoře'], ['middle', 'Uprostřed'], ['bottom', 'Dole']]
        .map(([v, l]) => h('option', { value: v, selected: (cfg.titleAlign ?? '') === v }, l))));
  const frame = h('iframe', { class: 'pi-frame', title: 'Nastavení akce pluginu' });
  const note = h('div', { class: 'rm-meta' }, 'Načítám nastavení pluginu…');
  closePi(ui);
  // the core must know this key's current action first, or it opens the previous action's page
  ui.saveNow().then(() => ui.invoke('pi_open', { profileId, key })).then((pi) => {
    if (!pi) { note.textContent = 'Tahle akce nemá žádné nastavení.'; frame.remove(); return; }
    openPi = pi.context ?? `${profileId}:${key}`;
    frame.src = pi.url;
    frame.onload = () => note.remove();
  }).catch((e) => { note.textContent = String(e); frame.remove(); });
  return [head, alignRow, note, frame];
}

// property inspectors report their height via postMessage (bootstrap injected by the scheme handler)
window.addEventListener('message', (e) => {
  if (e.data?.type !== 'gd-pi-height') return;
  const f = document.querySelector('iframe.pi-frame');
  if (f && e.source === f.contentWindow) f.style.height = `${Math.min(Math.max(e.data.height, 120), 900)}px`;
});
