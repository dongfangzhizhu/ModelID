// pages/library.js
import { api } from '../api.js';
import { fmtBytes, fmtRelTime, t } from '../main.js';

const FORMAT_ICONS = { safetensors: '🟦', gguf: '🟩', ckpt: '🟧', pth: '🟪', pt: '🟪', bin: '⬜' };

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">${t('lib.title')}</h1>
      <p class="page-subtitle">${t('lib.subtitle')}</p>
    </div>
    <div class="page-body">
      <div class="toolbar">
        <input class="input" id="lib-search" placeholder="${t('lib.search')}" style="max-width:280px">
        <select class="select" id="lib-type" style="width:auto">
          <option value="">${t('lib.type.all')}</option>
          <option value="checkpoint">${t('lib.type.ckpt')}</option>
          <option value="lora">${t('lib.type.lora')}</option>
          <option value="vae">${t('lib.type.vae')}</option>
          <option value="controlnet">${t('lib.type.cn')}</option>
        </select>
        <select class="select" id="lib-sort" style="width:auto">
          <option value="last_seen">${t('lib.sort.recent')}</option>
          <option value="size">${t('lib.sort.size')}</option>
          <option value="name">${t('lib.sort.name')}</option>
          <option value="ref_count">${t('lib.sort.refs')}</option>
        </select>
        <label style="display:flex;align-items:center;gap:6px;font-size:13px;color:var(--text-secondary);cursor:pointer">
          <input type="checkbox" id="lib-orphan"> ${t('lib.orphan')}
        </label>
        <div style="flex:1"></div>
        <span id="lib-count" style="font-size:12px;color:var(--text-muted)"></span>
      </div>

      <div class="card" style="overflow:hidden">
        <div id="lib-list">
          <div class="loading-state"><div class="spinner"></div><p>${t('common.loading')}</p></div>
        </div>
        <div id="lib-pagination"
          style="padding:12px 20px;border-top:1px solid var(--border);display:flex;align-items:center;gap:8px;justify-content:center">
        </div>
      </div>
    </div>

    <dialog id="model-dialog" class="modal-dialog" style="max-width:640px">
      <div class="modal-header" style="display:flex;align-items:center;gap:12px">
        <h3 id="model-dialog-title" style="flex:1">${t('lib.detail.title')}</h3>
        <button class="btn btn-ghost btn-sm" onclick="document.getElementById('model-dialog').close()">✕</button>
      </div>
      <div class="modal-body" id="model-dialog-body" style="max-height:60vh;overflow-y:auto"></div>
    </dialog>
  `;

  let page = 1;
  let totalPages = 1;

  async function loadModels() {
    const q      = document.getElementById('lib-search').value;
    const type   = document.getElementById('lib-type').value;
    const sort   = document.getElementById('lib-sort').value;
    const orphan = document.getElementById('lib-orphan').checked;

    const params = { page, per_page: 50, sort };
    if (q)      params.q      = q;
    if (type)   params.type   = type;
    if (orphan) params.orphan = true;

    try {
      const data = await api.models(params);
      totalPages = Math.ceil(data.total / data.per_page) || 1;
      document.getElementById('lib-count').textContent = t('lib.count', { n: data.total.toLocaleString() });

      const list = document.getElementById('lib-list');
      if (data.items.length === 0) {
        list.innerHTML = `<div class="empty-state">
          <div class="empty-icon">◫</div>
          <p class="empty-title">${t('lib.empty')}</p>
        </div>`;
        renderPagination();
        return;
      }

      list.innerHTML = data.items.map(m => `
        <div class="model-row" onclick="showModelDetail('${m.blake3_hash}')">
          <div class="model-icon">${FORMAT_ICONS[m.format] || '📦'}</div>
          <div class="model-main">
            <div class="model-name">${escHtml(m.name)}</div>
            <div class="model-meta">
              ${m.arch       ? `<span class="badge badge-blue" style="margin-right:4px">${m.arch}</span>` : ''}
              ${m.model_type ? `<span class="badge badge-gray" style="margin-right:4px">${m.model_type}</span>` : ''}
              ${m.is_orphan  ? `<span class="badge badge-red"  style="margin-right:4px">${t('lib.orphan.badge')}</span>` : ''}
              ${t('lib.col.refs', { n: m.ref_count })} · ${fmtRelTime(m.last_seen)}
            </div>
          </div>
          <div class="model-size">${fmtBytes(m.size_bytes)}</div>
        </div>
      `).join('');

      renderPagination();
    } catch (e) {
      document.getElementById('lib-list').innerHTML =
        `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">${t('common.load_failed')}</p><p>${e.message}</p></div>`;
    }
  }

  function renderPagination() {
    const pag = document.getElementById('lib-pagination');
    if (totalPages <= 1) { pag.innerHTML = ''; return; }
    pag.innerHTML = `
      <button class="btn btn-ghost btn-sm" ${page <= 1 ? 'disabled' : ''} onclick="goPage(${page - 1})">${t('lib.prev_page')}</button>
      <span style="font-size:12px;color:var(--text-muted)">${page} / ${totalPages}</span>
      <button class="btn btn-ghost btn-sm" ${page >= totalPages ? 'disabled' : ''} onclick="goPage(${page + 1})">${t('lib.next_page')}</button>
    `;
    window.goPage = (p) => { page = p; loadModels(); };
  }

  window.showModelDetail = async (hash) => {
    const dialog = document.getElementById('model-dialog');
    document.getElementById('model-dialog-title').textContent = t('common.loading');
    document.getElementById('model-dialog-body').innerHTML =
      '<div class="loading-state"><div class="spinner"></div></div>';
    dialog.showModal();

    try {
      const m = await api.model(hash);
      document.getElementById('model-dialog-title').textContent = m.name;
      document.getElementById('model-dialog-body').innerHTML = `
        <dl style="display:grid;grid-template-columns:120px 1fr;gap:8px 16px;font-size:13px">
          <dt style="color:var(--text-muted)">${t('lib.detail.hash')}</dt>
          <dd><span class="mono">${m.blake3_hash.slice(0,16)}…</span></dd>
          <dt style="color:var(--text-muted)">${t('lib.detail.format')}</dt>
          <dd>${m.format || '—'}</dd>
          <dt style="color:var(--text-muted)">${t('lib.detail.arch')}</dt>
          <dd>${m.arch || '—'}</dd>
          <dt style="color:var(--text-muted)">${t('lib.detail.size')}</dt>
          <dd>${fmtBytes(m.size_bytes)}</dd>
          <dt style="color:var(--text-muted)">${t('lib.detail.refs')}</dt>
          <dd>${m.ref_count}</dd>
          <dt style="color:var(--text-muted)">${t('lib.detail.found')}</dt>
          <dd>${fmtRelTime(m.created_at)}</dd>
        </dl>
        <div style="margin-top:16px">
          <p style="font-size:12px;font-weight:600;color:var(--text-muted);margin-bottom:8px;text-transform:uppercase;letter-spacing:0.5px">
            ${t('lib.detail.paths')}
          </p>
          ${m.aliases.map(a => `
            <div style="display:flex;align-items:center;gap:8px;padding:8px 0;border-bottom:1px solid var(--border-light);font-size:12px">
              <span class="badge badge-${a.frontend === 'User' ? 'blue' : 'gray'}">${a.frontend}</span>
              <span style="font-family:var(--font-mono);color:var(--text-muted);flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"
                title="${escHtml(a.path)}">${escHtml(a.path)}</span>
              <span class="badge badge-gray">${a.alias_type}</span>
            </div>
          `).join('')}
        </div>
      `;
    } catch (e) {
      document.getElementById('model-dialog-body').innerHTML =
        `<p style="color:var(--danger)">${t('lib.detail.fail', { msg: e.message })}</p>`;
    }
  };

  ['lib-type', 'lib-sort'].forEach(id => {
    document.getElementById(id).addEventListener('change', () => { page = 1; loadModels(); });
  });
  document.getElementById('lib-search').addEventListener('input', debounce(() => { page = 1; loadModels(); }, 400));
  document.getElementById('lib-orphan').addEventListener('change', () => { page = 1; loadModels(); });

  loadModels();
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function debounce(fn, ms) {
  let t;
  return (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
}
