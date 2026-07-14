// pages/dashboard.js
import { api } from '../api.js';
import { fmtBytes, fmtRelTime, toast, confirm, events, t } from '../main.js';
import {
  resolvePhaseKey,
  computeScanProgressPercent,
  computeScanButtonDisabled,
} from './dashboard-progress.js';
import { decideButtonToRestore } from './dashboard-operation-complete.js';

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">${t('dash.title')}</h1>
      <p class="page-subtitle">${t('dash.subtitle')}</p>
    </div>
    <div class="page-body">
      <div class="stats-grid">
        <div class="stat-card">
          <div class="stat-label">${t('dash.stat.models')}</div>
          <div class="stat-value" id="stat-models">…</div>
          <div class="stat-sub">${t('dash.stat.models.sub')}</div>
        </div>
        <div class="stat-card danger">
          <div class="stat-label">${t('dash.stat.waste')}</div>
          <div class="stat-value danger" id="stat-waste">…</div>
          <div class="stat-sub">${t('dash.stat.waste.sub')}</div>
        </div>
        <div class="stat-card success">
          <div class="stat-label">${t('dash.stat.size')}</div>
          <div class="stat-value" id="stat-size">…</div>
          <div class="stat-sub">${t('dash.stat.size.sub')}</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">${t('dash.stat.scan')}</div>
          <div class="stat-value" style="font-size:18px" id="stat-scan">—</div>
          <div class="stat-sub">${t('dash.stat.scan.sub')}</div>
        </div>
      </div>

      <div style="display:flex;gap:16px;margin-bottom:24px;flex-wrap:wrap">
        <button class="btn btn-primary" id="btn-scan">${t('dash.btn.scan')}</button>
        <button class="btn btn-ghost" onclick="location.hash='#/dupes'">${t('dash.btn.dedup')}</button>
        <button class="btn btn-ghost" id="btn-gc">${t('dash.btn.gc')}</button>
      </div>

      <div id="scan-progress-area" style="margin-bottom:20px;display:none">
        <div class="scan-bar">
          <div class="scan-info">
            <div class="scan-phase" id="scan-phase">${t('dash.scan.scanning')}</div>
            <div class="scan-path" id="scan-path"></div>
          </div>
          <div style="min-width:140px">
            <div class="progress-wrap"><div class="progress-bar" id="scan-prog" style="width:0%"></div></div>
            <div style="font-size:11px;color:var(--text-muted);margin-top:4px;text-align:right" id="scan-count"></div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-header">
          <span class="card-title">${t('dash.storage.title')}</span>
          <span style="font-size:12px;color:var(--text-muted)" id="frontend-count"></span>
        </div>
        <div id="frontend-table-wrap">
          <div class="loading-state"><div class="spinner"></div></div>
        </div>
      </div>
    </div>
  `;

  const unsubs = [
    events.on('scan_progress', handleScanProgress),
    events.on('operation_complete', handleOperationComplete),
  ];
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
      document.getElementById('stat-scan').textContent   = s.last_scan_at ? fmtRelTime(s.last_scan_at) : t('dash.stat.scan.never');
      document.getElementById('frontend-count').textContent = t('dash.storage.count', { n: s.frontends.length });

      const wrap = document.getElementById('frontend-table-wrap');
      if (s.frontends.length === 0) {
        wrap.innerHTML = `<div class="empty-state">
          <div class="empty-icon">◫</div>
          <p class="empty-title">${t('dash.storage.empty')}</p>
          <p class="empty-sub">${t('dash.storage.empty.sub')}</p>
        </div>`;
        return;
      }

      wrap.innerHTML = `
        <table class="data-table">
          <thead><tr>
            <th>${t('dash.storage.col.frontend')}</th>
            <th>${t('dash.storage.col.models')}</th>
            <th>${t('dash.storage.col.size')}</th>
            <th>${t('dash.storage.col.waste')}</th>
          </tr></thead>
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
      toast(t('dash.scan.started'), 'success');
    } catch (e) {
      toast(t('dash.scan.start_fail', { msg: e.message }), 'error');
      document.getElementById('btn-scan').disabled = false;
    }
  }

  function handleScanProgress(payload) {
    const area = document.getElementById('scan-progress-area');
    if (!area) return;
    area.style.display = '';

    const p = payload.payload || payload;
    const phaseKey = resolvePhaseKey(p.phase);
    document.getElementById('scan-phase').textContent = t(phaseKey);
    document.getElementById('scan-path').textContent  = p.current_path || '';
    document.getElementById('scan-count').textContent = t('dash.scan.files', { n: p.files_scanned || 0 });

    const pct = computeScanProgressPercent(p);
    document.getElementById('scan-prog').style.width = pct + '%';

    document.getElementById('btn-scan').disabled = computeScanButtonDisabled(p.phase);

    if (p.phase === 'done') {
      setTimeout(() => { if (area) area.style.display = 'none'; }, 2000);
      loadStats();
      toast(t('dash.scan.complete'), 'success');
    }
  }

  function handleOperationComplete(payload) {
    const p = payload.payload || payload;
    const buttonToRestore = decideButtonToRestore(p.operation);
    
    if (buttonToRestore === 'gc') {
      document.getElementById('btn-gc').disabled = false;
      if (!p.success) toast(t('dash.gc.fail', { msg: p.message }), 'error');
    } else if (buttonToRestore === 'scan') {
      document.getElementById('btn-scan').disabled = false;
    }
  }

  async function runGc() {
    try {
      const preview = await api.gcPreview();
      if (preview.would_quarantine.length === 0) {
        toast(t('dash.gc.none'), 'info');
        return;
      }
      const ok = await confirm(
        t('dash.gc.confirm.title'),
        t('dash.gc.confirm.body', { n: preview.would_quarantine.length, size: fmtBytes(preview.total_reclaimable_bytes) })
      );
      if (!ok) return;
      document.getElementById('btn-gc').disabled = true;
      await api.gcRun({});
      toast(t('dash.gc.done'), 'success');
      loadStats();
    } catch (e) {
      toast(t('dash.gc.fail', { msg: e.message }), 'error');
      document.getElementById('btn-gc').disabled = false;
    }
  }

  return () => { unsubs.forEach((fn) => fn()); clearInterval(refreshTimer); };
}
