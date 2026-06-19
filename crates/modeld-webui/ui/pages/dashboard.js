// pages/dashboard.js
import { api } from '../api.js';
import { fmtBytes, fmtRelTime, toast, confirm, events } from '../main.js';

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">仪表板</h1>
      <p class="page-subtitle">存储概览与快捷操作</p>
    </div>
    <div class="page-body">
      <div class="stats-grid" id="stats-grid">
        <div class="stat-card"><div class="stat-label">已索引模型</div><div class="stat-value" id="stat-models">…</div><div class="stat-sub">个模型文件</div></div>
        <div class="stat-card danger"><div class="stat-label">可节省空间</div><div class="stat-value danger" id="stat-waste">…</div><div class="stat-sub">重复文件占用</div></div>
        <div class="stat-card success"><div class="stat-label">总占用空间</div><div class="stat-value" id="stat-size">…</div><div class="stat-sub">所有模型</div></div>
        <div class="stat-card"><div class="stat-label">上次扫描</div><div class="stat-value" style="font-size:18px" id="stat-scan">—</div><div class="stat-sub">扫描时间</div></div>
      </div>

      <div style="display:flex;gap:16px;margin-bottom:24px;flex-wrap:wrap">
        <button class="btn btn-primary" id="btn-scan">⟳ 立即扫描</button>
        <button class="btn btn-ghost" id="btn-dedup" onclick="location.hash='#/dupes'">⊕ 去重管理</button>
        <button class="btn btn-ghost" id="btn-gc">♺ 垃圾回收</button>
      </div>

      <div id="scan-progress-area" style="margin-bottom:20px;display:none">
        <div class="scan-bar">
          <div class="scan-info">
            <div class="scan-phase" id="scan-phase">扫描中...</div>
            <div class="scan-path" id="scan-path"></div>
          </div>
          <div style="min-width:140px">
            <div class="progress-wrap"><div class="progress-bar" id="scan-prog" style="width:0%"></div></div>
            <div style="font-size:11px;color:var(--text-muted);margin-top:4px;text-align:right" id="scan-count"></div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-header"><span class="card-title">存储分布</span><span style="font-size:12px;color:var(--text-muted)" id="frontend-count"></span></div>
        <div id="frontend-table-wrap">
          <div class="loading-state"><div class="spinner"></div></div>
        </div>
      </div>
    </div>
  `;

  // Subscribe to scan progress
  const unsub = events.on('scan_progress', handleScanProgress);

  loadStats();

  let refreshTimer = setInterval(loadStats, 30000);

  document.getElementById('btn-scan').addEventListener('click', triggerScan);
  document.getElementById('btn-gc').addEventListener('click', runGc);

  async function loadStats() {
    try {
      const s = await api.stats();
      document.getElementById('stat-models').textContent = s.total_models.toLocaleString();
      document.getElementById('stat-waste').textContent  = fmtBytes(s.duplicate_waste_bytes);
      document.getElementById('stat-size').textContent   = fmtBytes(s.total_size_bytes);
      document.getElementById('stat-scan').textContent   = fmtRelTime(s.last_scan_at) || '未扫描';
      document.getElementById('frontend-count').textContent = `${s.frontends.length} 个前端`;

      const wrap = document.getElementById('frontend-table-wrap');
      if (s.frontends.length === 0) {
        wrap.innerHTML = `<div class="empty-state"><div class="empty-icon">◫</div><p class="empty-title">暂无模型</p><p class="empty-sub">点击"立即扫描"开始索引模型文件</p></div>`;
        return;
      }

      wrap.innerHTML = `
        <table class="data-table">
          <thead><tr><th>前端</th><th>模型数</th><th>占用空间</th><th>重复浪费</th></tr></thead>
          <tbody>
            ${s.frontends.map(f => `
              <tr>
                <td><span class="badge badge-blue">${f.name}</span></td>
                <td>${f.model_count.toLocaleString()}</td>
                <td>${fmtBytes(f.size_bytes)}</td>
                <td style="color:var(--danger)">${fmtBytes(f.duplicate_bytes)}</td>
              </tr>
            `).join('')}
          </tbody>
        </table>
      `;
    } catch (e) {
      console.error('Stats load failed', e);
    }
  }

  async function triggerScan() {
    try {
      document.getElementById('btn-scan').disabled = true;
      document.getElementById('scan-progress-area').style.display = '';
      await api.scan({});
      toast('扫描已启动', 'success');
    } catch (e) {
      toast('扫描启动失败: ' + e.message, 'error');
      document.getElementById('btn-scan').disabled = false;
    }
  }

  function handleScanProgress(payload) {
    const area = document.getElementById('scan-progress-area');
    if (!area) return;

    area.style.display = '';
    document.getElementById('scan-phase').textContent =
      ({ walking: '遍历文件', hashing: '计算哈希', indexing: '建立索引', done: '完成' })[payload.payload?.phase || payload.phase] || '扫描中';

    const phase = payload.payload?.phase || payload.phase;
    const current = payload.payload?.current_path || payload.current_path || '';
    const scanned = payload.payload?.files_scanned || payload.files_scanned || 0;
    const total   = payload.payload?.files_total   || payload.files_total   || 0;

    document.getElementById('scan-path').textContent = current;
    document.getElementById('scan-count').textContent = total > 0 ? `${scanned} / ${total}` : `${scanned} 个文件`;
    const pct = total > 0 ? Math.round((scanned / total) * 100) : 0;
    document.getElementById('scan-prog').style.width = pct + '%';

    if (phase === 'done') {
      document.getElementById('btn-scan').disabled = false;
      setTimeout(() => { if (area) area.style.display = 'none'; }, 2000);
      loadStats();
      toast('扫描完成', 'success');
    }
  }

  async function runGc() {
    try {
      const preview = await api.gcPreview();
      if (preview.reclaimable.length === 0) {
        toast('没有可回收的文件', 'info');
        return;
      }
      const ok = await confirm(
        '确认垃圾回收',
        `将永久删除 ${preview.reclaimable.length} 个隔离文件，释放 ${fmtBytes(preview.total_reclaimable_bytes)} 空间。此操作不可逆。`
      );
      if (!ok) return;
      await api.gcRun({});
      toast('垃圾回收完成', 'success');
      loadStats();
    } catch (e) {
      toast('操作失败: ' + e.message, 'error');
    }
  }

  // Return cleanup function
  return () => {
    unsub();
    clearInterval(refreshTimer);
  };
}
