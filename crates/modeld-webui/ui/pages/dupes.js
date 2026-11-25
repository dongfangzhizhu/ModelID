// pages/dupes.js
import { api } from '../api.js';
import { fmtBytes, toast, confirm, t } from '../main.js';
import { escapeHtml, safeHtml } from '../security.js';

const HEX_HASH = /^[0-9a-f]+$/i;

export function render(container) {
  container.innerHTML = safeHtml`
    <div class="page-header">
      <h1 class="page-title">${t('dupes.title')}</h1>
      <p class="page-subtitle">${t('dupes.subtitle')}</p>
    </div>
    <div class="page-body">
      <div class="toolbar">
        <input class="input" id="dupes-search" aria-label="${t('dupes.search')}" placeholder="${t('dupes.search')}" style="max-width:260px">
        <select class="select" id="dupes-sort" aria-label="${t('common.sort_by')}" style="width:auto">
          <option value="waste">${t('dupes.sort.waste')}</option>
          <option value="copies">${t('dupes.sort.copies')}</option>
          <option value="size">${t('dupes.sort.size')}</option>
        </select>
        <div style="flex:1"></div>
        <span id="dupes-summary" style="font-size:12px;color:var(--text-muted)"></span>
        <button class="btn btn-primary" id="btn-dedup-all">${t('dupes.btn.dedup_all')}</button>
      </div>

      <div id="dupes-list">
        <div class="loading-state"><div class="spinner"></div><p>${t('common.loading')}</p></div>
      </div>
    </div>
  `;

  let allDupes = [];

  async function loadDupes() {
    try {
      const data = await api.dupes();
      allDupes = data.items;
      document.getElementById('dupes-summary').textContent =
        t('dupes.summary', { n: data.items.length, size: fmtBytes(data.total_waste_bytes) });
      renderList();
    } catch (e) {
      document.getElementById('dupes-list').innerHTML = safeHtml`
        <div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">${t('common.load_failed')}</p><p>${e.message}</p></div>`;
    }
  }

  function renderList() {
    const q    = (document.getElementById('dupes-search')?.value || '').toLowerCase();
    const sort = document.getElementById('dupes-sort')?.value || 'waste';

    let items = allDupes.filter(g => !q || g.name.toLowerCase().includes(q));
    if (sort === 'waste')  items.sort((a, b) => b.waste_bytes  - a.waste_bytes);
    if (sort === 'copies') items.sort((a, b) => b.copy_count   - a.copy_count);
    if (sort === 'size')   items.sort((a, b) => b.size_bytes   - a.size_bytes);

    const list = document.getElementById('dupes-list');

    if (items.length === 0) {
      list.innerHTML = safeHtml`<div class="empty-state">
        <div class="empty-icon">✓</div>
        <p class="empty-title">${t('dupes.empty')}</p>
        <p class="empty-sub">${t('dupes.empty.sub')}</p>
      </div>`;
      return;
    }

    list.innerHTML = items.map((g, idx) => `
      <div class="dupe-group" id="dg-${escapeHtml(idx)}">
        <div class="dupe-header" data-dupe-idx="${escapeHtml(idx)}">
          <span class="badge badge-orange">${escapeHtml(t('dupes.copies', { n: g.copy_count }))}</span>
          <span class="dupe-name">${escapeHtml(g.name)}</span>
          ${g.arch ? `<span class="badge badge-blue">${escapeHtml(g.arch)}</span>` : ''}
          <span class="dupe-waste">${escapeHtml(fmtBytes(g.waste_bytes))}</span>
          <span class="mono" style="font-size:10px">${escapeHtml(fmtBytes(g.size_bytes))}</span>
          <span class="dupe-toggle">▼</span>
        </div>
        <div class="dupe-paths">
          ${g.paths.map((p, i) => `
            <div class="dupe-path-row">
              ${i === 0
                ? `<span class="path-keep">${escapeHtml(t('dupes.group.keep'))}</span>`
                : '<span style="width:30px"></span>'}
              <span class="path-text" title="${escapeHtml(p.path)}">${escapeHtml(p.path)}</span>
              <span class="badge badge-gray">${escapeHtml(p.frontend)}</span>
            </div>
          `).join('')}
          <div style="margin-top:10px;display:flex;gap:8px">
            <button class="btn btn-ghost btn-sm dupe-preview" data-hash="${escapeHtml(g.blake3_hash)}">${escapeHtml(t('dupes.group.preview'))}</button>
          </div>
        </div>
      </div>
    `).join('');

    // Bind via addEventListener instead of inline onclick so no hash is ever
    // concatenated into executable JavaScript.
    list.querySelectorAll('.dupe-header').forEach((header) => {
      header.addEventListener('click', () => {
        document.getElementById(`dg-${header.dataset.dupeIdx}`)?.classList.toggle('open');
      });
    });
    list.querySelectorAll('.dupe-preview').forEach((btn) => {
      btn.addEventListener('click', () => window.dryRunGroup(btn.dataset.hash));
    });

    window.dryRunGroup = async (hash) => {
      if (!HEX_HASH.test(hash)) return;
      try {
        const res = await api.dedupPreview();
        const op = res.operations.find(o => o.hash === hash);
        if (!op) return;
        toast(t('dupes.preview.msg', {
          name: op.keep_path.split(/[\\/]/).pop(),
          size: fmtBytes(op.would_save_bytes),
        }), 'info', 5000);
      } catch (e) {
        toast(t('dupes.preview.fail', { msg: e.message }), 'error');
      }
    };
  }

  document.getElementById('dupes-search').addEventListener('input', renderList);
  document.getElementById('dupes-sort').addEventListener('change', renderList);

  document.getElementById('btn-dedup-all').addEventListener('click', async () => {
    try {
      const res = await api.dedupPreview();
      if (!res.operations.length) { toast(t('dupes.empty'), 'info'); return; }
      const ok = await confirm(
        t('dupes.confirm.title'),
        t('dupes.confirm.body', { n: res.operations.length, size: fmtBytes(res.total_would_save_bytes) })
      );
      if (!ok) return;
      const hashes = res.operations.map(op => op.hash);
      const applyRes = await api.dedupApply({ hashes });
      toast(t('dupes.apply.done', {
        n: applyRes.groups_succeeded,
        size: fmtBytes(applyRes.space_saved),
      }), 'success');
      await loadDupes();
    } catch (e) {
      toast(t('dupes.fail', { msg: e.message }), 'error');
    }
  });

  loadDupes();
}

