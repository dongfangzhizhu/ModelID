// pages/downloads.js
import { api } from '../api.js';
import { fmtBytes, fmtRelTime, events } from '../main.js';

const STATUS_COLORS = { completed: 'green', downloading: 'blue', failed: 'red', pending: 'gray', paused: 'orange' };
const STATUS_LABELS = { completed: '完成', downloading: '下载中', failed: '失败', pending: '等待', paused: '暂停' };

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">下载器</h1>
      <p class="page-subtitle">模型下载历史与进度</p>
    </div>
    <div class="page-body">
      <div class="toolbar">
        <select class="select" id="dl-filter" style="width:auto">
          <option value="">全部</option>
          <option value="downloading">下载中</option>
          <option value="completed">已完成</option>
          <option value="failed">失败</option>
        </select>
        <div style="flex:1"></div>
        <span id="dl-count" style="font-size:12px;color:var(--text-muted)"></span>
      </div>
      <div id="dl-list">
        <div class="loading-state"><div class="spinner"></div><p>加载中...</p></div>
      </div>
    </div>
  `;

  let allDownloads = [];

  const unsub = events.on('download_progress', () => loadDownloads());

  async function loadDownloads() {
    try {
      const data = await api.downloads();
      allDownloads = data.items;
      renderList();
    } catch (e) {
      document.getElementById('dl-list').innerHTML =
        `<div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">加载失败</p><p>${e.message}</p></div>`;
    }
  }

  function renderList() {
    const filter = document.getElementById('dl-filter')?.value || '';
    const items = filter ? allDownloads.filter(d => d.status === filter) : allDownloads;
    document.getElementById('dl-count').textContent = `共 ${items.length} 条记录`;

    const list = document.getElementById('dl-list');

    if (items.length === 0) {
      list.innerHTML = `<div class="empty-state"><div class="empty-icon">⬇</div><p class="empty-title">暂无下载记录</p><p class="empty-sub">通过前端发起模型下载后将在此显示</p></div>`;
      return;
    }

    list.innerHTML = items.map(d => {
      const pct = d.bytes_total > 0 ? Math.round((d.bytes_done / d.bytes_total) * 100) : 0;
      const color = STATUS_COLORS[d.status] || 'gray';
      const label = STATUS_LABELS[d.status] || d.status;

      return `
        <div class="download-card">
          <div class="download-header">
            <span class="download-name" title="${escHtml(d.source_url)}">${escHtml(d.name)}</span>
            <span class="badge badge-${color} download-status">${label}</span>
          </div>
          ${d.status === 'downloading' ? `
            <div class="download-progress">
              <div class="progress-wrap"><div class="progress-bar" style="width:${pct}%"></div></div>
            </div>
          ` : ''}
          <div class="download-meta">
            <span>${d.bytes_total > 0 ? `${fmtBytes(d.bytes_done)} / ${fmtBytes(d.bytes_total)} (${pct}%)` : fmtBytes(d.bytes_done)}</span>
            <span>${fmtRelTime(d.finished_at || d.started_at)}</span>
          </div>
          ${d.error ? `<div style="font-size:11px;color:var(--danger);margin-top:6px">⚠ ${escHtml(d.error)}</div>` : ''}
          ${d.blake3_hash ? `<div style="font-size:11px;color:var(--text-muted);margin-top:4px">BLAKE3: <span class="mono">${d.blake3_hash.slice(0,16)}…</span></div>` : ''}
        </div>
      `;
    }).join('');
  }

  document.getElementById('dl-filter').addEventListener('change', renderList);

  // Refresh active downloads every 3s
  const timer = setInterval(() => {
    if (allDownloads.some(d => d.status === 'downloading')) loadDownloads();
  }, 3000);

  loadDownloads();

  return () => {
    unsub();
    clearInterval(timer);
  };
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
