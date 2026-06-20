// pages/refs.js — Reference graph: models × frontends
import { api } from '../api.js';
import { fmtBytes, t } from '../main.js';

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">${t('refs.title')}</h1>
      <p class="page-subtitle">${t('refs.subtitle')}</p>
    </div>
    <div class="page-body">
      <div class="toolbar">
        <input class="input" id="refs-search" placeholder="${t('refs.search')}" style="max-width:280px">
        <select class="select" id="refs-frontend" style="width:auto">
          <option value="">${t('refs.fe.all')}</option>
        </select>
        <div style="flex:1"></div>
        <span id="refs-count" style="font-size:12px;color:var(--text-muted)"></span>
      </div>

      <div class="card" style="overflow:hidden">
        <div id="refs-table-wrap">
          <div class="loading-state"><div class="spinner"></div><p>${t('common.loading')}</p></div>
        </div>
      </div>
    </div>
  `;

  let allModels = [];

  async function loadRefs() {
    try {
      const data = await api.models({ per_page: 200 });
      allModels = data.items;

      const frontends = [...new Set(allModels.flatMap(m => m.frontends))].sort();
      const select = document.getElementById('refs-frontend');
      frontends.forEach(f => {
        const opt = document.createElement('option');
        opt.value = f;
        opt.textContent = f;
        select.appendChild(opt);
      });

      renderTable();
    } catch (e) {
      document.getElementById('refs-table-wrap').innerHTML =
        `<div class="empty-state"><div class="empty-icon">⚠</div><p>${e.message}</p></div>`;
    }
  }

  function renderTable() {
    const q  = (document.getElementById('refs-search')?.value || '').toLowerCase();
    const fe = document.getElementById('refs-frontend')?.value || '';

    let items = allModels;
    if (q)  items = items.filter(m => m.name.toLowerCase().includes(q) || m.paths.some(p => p.toLowerCase().includes(q)));
    if (fe) items = items.filter(m => m.frontends.includes(fe));

    document.getElementById('refs-count').textContent = t('refs.count', { n: items.length });

    const wrap = document.getElementById('refs-table-wrap');

    if (items.length === 0) {
      wrap.innerHTML = `<div class="empty-state">
        <div class="empty-icon">◉</div>
        <p class="empty-title">${t('refs.empty')}</p>
      </div>`;
      return;
    }

    wrap.innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th>${t('refs.col.model')}</th>
            <th>${t('refs.col.size')}</th>
            <th>${t('refs.col.fe')}</th>
            <th>${t('refs.col.refs')}</th>
            <th>${t('refs.col.orphan')}</th>
          </tr>
        </thead>
        <tbody>
          ${items.map(m => `
            <tr title="${escHtml(m.paths.join('\n'))}">
              <td>
                <div style="font-weight:600;color:var(--text-primary)">${escHtml(m.name)}</div>
                <div style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted)">${m.blake3_hash.slice(0,12)}…</div>
              </td>
              <td>${fmtBytes(m.size_bytes)}</td>
              <td>
                ${[...new Set(m.frontends)].map(f =>
                  `<span class="badge badge-blue" style="margin-right:3px">${escHtml(f)}</span>`
                ).join('')}
              </td>
              <td>${m.ref_count}</td>
              <td>${m.is_orphan
                ? `<span class="badge badge-red">${t('common.yes')}</span>`
                : `<span class="badge badge-green">${t('common.no')}</span>`}
              </td>
            </tr>
          `).join('')}
        </tbody>
      </table>
    `;
  }

  document.getElementById('refs-search').addEventListener('input', debounce(renderTable, 300));
  document.getElementById('refs-frontend').addEventListener('change', renderTable);

  loadRefs();
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function debounce(fn, ms) {
  let t;
  return (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
}
