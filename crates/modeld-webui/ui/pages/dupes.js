// pages/dupes.js
import { api } from '../api.js';
import { fmtBytes, toast, confirm } from '../main.js';

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">重复文件</h1>
      <p class="page-subtitle">发现并消除重复模型文件，释放磁盘空间</p>
    </div>
    <div class="page-body">
      <div class="toolbar">
        <input class="input" id="dupes-search" placeholder="搜索文件名..." style="max-width:260px">
        <select class="select" id="dupes-sort" style="width:auto">
          <option value="waste">按浪费空间排序</option>
          <option value="copies">按副本数排序</option>
          <option value="size">按文件大小排序</option>
        </select>
        <div style="flex:1"></div>
        <span id="dupes-summary" style="font-size:12px;color:var(--text-muted)"></span>
        <button class="btn btn-primary" id="btn-dedup-all">⊕ 一键去重（预览）</button>
      </div>

      <div id="dupes-list">
        <div class="loading-state"><div class="spinner"></div><p>加载中...</p></div>
      </div>
    </div>
  `;

  let allDupes = [];

  async function loadDupes() {
    try {
      const data = await api.dupes();
      allDupes = data.items;
      document.getElementById('dupes-summary').textContent =
        `${data.items.length} 组重复，浪费 ${fmtBytes(data.total_waste_bytes)}`;
      renderList();
    } catch (e) {
      document.getElementById('dupes-list').innerHTML =
        `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">加载失败</p><p>${e.message}</p></div>`;
    }
  }

  function renderList() {
    const q = (document.getElementById('dupes-search')?.value || '').toLowerCase();
    const sort = document.getElementById('dupes-sort')?.value || 'waste';

    let items = allDupes.filter(g => !q || g.name.toLowerCase().includes(q));
    if (sort === 'waste')  items.sort((a, b) => b.waste_bytes  - a.waste_bytes);
    if (sort === 'copies') items.sort((a, b) => b.copy_count   - a.copy_count);
    if (sort === 'size')   items.sort((a, b) => b.size_bytes   - a.size_bytes);

    const list = document.getElementById('dupes-list');

    if (items.length === 0) {
      list.innerHTML = `<div class="empty-state"><div class="empty-icon">✓</div><p class="empty-title">未发现重复文件</p><p class="empty-sub">当前存储空间使用已最优</p></div>`;
      return;
    }

    list.innerHTML = items.map((g, idx) => `
      <div class="dupe-group" id="dg-${idx}">
        <div class="dupe-header" onclick="toggleDupe(${idx})">
          <span class="badge badge-orange">×${g.copy_count} 副本</span>
          <span class="dupe-name">${escHtml(g.name)}</span>
          ${g.arch ? `<span class="badge badge-blue">${escHtml(g.arch)}</span>` : ''}
          <span class="dupe-waste">浪费 ${fmtBytes(g.waste_bytes)}</span>
          <span class="mono" style="font-size:10px">${fmtBytes(g.size_bytes)}</span>
          <span class="dupe-toggle">▼</span>
        </div>
        <div class="dupe-paths">
          ${g.paths.map((p, i) => `
            <div class="dupe-path-row">
              ${i === 0 ? '<span class="path-keep">保留</span>' : '<span style="width:30px"></span>'}
              <span class="path-text" title="${escHtml(p.path)}">${escHtml(p.path)}</span>
              <span class="badge badge-gray">${p.frontend}</span>
            </div>
          `).join('')}
          <div style="margin-top:10px;display:flex;gap:8px">
            <button class="btn btn-ghost btn-sm" onclick="dryRunGroup('${g.blake3_hash}')">预览去重</button>
          </div>
        </div>
      </div>
    `).join('');

    // Attach global handlers
    window.toggleDupe = (idx) => {
      const el = document.getElementById(`dg-${idx}`);
      el?.classList.toggle('open');
    };

    window.dryRunGroup = async (hash) => {
      try {
        const res = await api.dedup({ dry_run: true, hashes: [hash] });
        if (res.operations.length === 0) return;
        const op = res.operations[0];
        toast(`预览: 保留 ${op.keep_path.split(/[\\/]/).pop()}，可节省 ${fmtBytes(op.would_save_bytes)}`, 'info', 5000);
      } catch (e) {
        toast('预览失败: ' + e.message, 'error');
      }
    };
  }

  document.getElementById('dupes-search').addEventListener('input', renderList);
  document.getElementById('dupes-sort').addEventListener('change', renderList);

  document.getElementById('btn-dedup-all').addEventListener('click', async () => {
    try {
      const res = await api.dedup({ dry_run: true });
      if (res.operations.length === 0) { toast('没有可去重的文件', 'info'); return; }
      const ok = await confirm(
        '确认去重操作',
        `将对 ${res.operations.length} 组文件执行硬链接去重，预计节省 ${fmtBytes(res.total_would_save_bytes)}。操作完成后重复文件将被替换为硬链接（原内容不变）。`
      );
      if (!ok) return;
      toast('去重功能将在完整版本中可用', 'info');
    } catch (e) {
      toast('操作失败: ' + e.message, 'error');
    }
  });

  loadDupes();
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
