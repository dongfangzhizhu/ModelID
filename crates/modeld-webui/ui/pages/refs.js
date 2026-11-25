// pages/refs.js — Reference graph: models × frontends
import { api } from '../api.js';
import { fmtBytes, t } from '../main.js';
import { escapeHtml, safeHtml } from '../security.js';

export function render(container) {
  container.innerHTML = safeHtml`
    <div class="page-header">
      <h1 class="page-title">${t('refs.title')}</h1>
      <p class="page-subtitle">${t('refs.subtitle')}</p>
    </div>
    <div class="page-body">
      <div class="toolbar">
        <input class="input" id="refs-search" aria-label="${t('refs.search')}" placeholder="${t('refs.search')}" style="max-width:280px">
        <select class="select" id="refs-frontend" aria-label="${t('common.frontend')}" style="width:auto">
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
      document.getElementById('refs-table-wrap').innerHTML = safeHtml`
        <div class="empty-state"><div class="empty-icon">⚠</div><p>${e.message}</p></div>`;
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
      wrap.innerHTML = safeHtml`<div class="empty-state">
        <div class="empty-icon">◉</div>
        <p class="empty-title">${t('refs.empty')}</p>
      </div>`;
      return;
    }

    wrap.innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th>${escapeHtml(t('refs.col.model'))}</th>
            <th>${escapeHtml(t('refs.col.size'))}</th>
            <th>${escapeHtml(t('refs.col.fe'))}</th>
            <th>${escapeHtml(t('refs.col.refs'))}</th>
            <th>${escapeHtml(t('refs.col.orphan'))}</th>
          </tr>
        </thead>
        <tbody>
          ${items.map(m => `
            <tr title="${escapeHtml(m.paths.join('\n'))}">
              <td>
                <div style="font-weight:600;color:var(--text-primary)">${escapeHtml(m.name)}</div>
                <div style="font-family:var(--font-mono);font-size:10px;color:var(--text-muted)">${escapeHtml(m.blake3_hash.slice(0,12))}…</div>
              </td>
              <td>${escapeHtml(fmtBytes(m.size_bytes))}</td>
              <td>
                ${[...new Set(m.frontends)].map(f =>
                  `<span class="badge badge-blue" style="margin-right:3px">${escapeHtml(f)}</span>`
                ).join('')}
              </td>
              <td>${escapeHtml(m.ref_count)}</td>
              <td>${m.is_orphan
                ? `<span class="badge badge-red">${escapeHtml(t('common.yes'))}</span>`
                : `<span class="badge badge-green">${escapeHtml(t('common.no'))}</span>`}
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

function debounce(fn, ms) {
  let t;
  return (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
}
