// pages/settings.js
import { api } from '../api.js';
import { toast } from '../main.js';

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">设置</h1>
      <p class="page-subtitle">存储路径、GC 策略、Web UI 选项</p>
    </div>
    <div class="page-body" style="max-width:680px">
      <div id="settings-form">
        <div class="loading-state"><div class="spinner"></div><p>加载中...</p></div>
      </div>
    </div>
  `;

  let cfg = null;

  async function loadSettings() {
    try {
      cfg = await api.settings();
      renderForm();
    } catch (e) {
      document.getElementById('settings-form').innerHTML =
        `<div class="empty-state"><div class="empty-icon">⚠</div><p>${e.message}</p></div>`;
    }
  }

  function renderForm() {
    document.getElementById('settings-form').innerHTML = `
      <!-- Store -->
      <div class="settings-section">
        <h2 class="settings-title">存储</h2>
        <div class="settings-row">
          <div><div class="settings-label">存储根目录</div><div class="settings-desc">模型数据库与 CAS 对象存储位置</div></div>
          <div class="settings-control"><input class="input" id="s-root" value="${escHtml(cfg.store.root)}" style="width:260px"></div>
        </div>
        <div class="settings-row">
          <div><div class="settings-label">启动时自动扫描</div><div class="settings-desc">每次启动 daemon 时扫描存储目录</div></div>
          <div class="settings-control">
            <label class="toggle"><input type="checkbox" id="s-auto-scan" ${cfg.store.auto_scan_on_start ? 'checked' : ''}><span class="toggle-track"></span></label>
          </div>
        </div>
        <div class="settings-row">
          <div><div class="settings-label">文件监视（inotify）</div><div class="settings-desc">实时监视文件变化并自动更新索引</div></div>
          <div class="settings-control">
            <label class="toggle"><input type="checkbox" id="s-watch" ${cfg.store.watch_enabled ? 'checked' : ''}><span class="toggle-track"></span></label>
          </div>
        </div>
        <div class="settings-row">
          <div><div class="settings-label">增量扫描</div><div class="settings-desc">仅扫描新增或变化的文件（更快）</div></div>
          <div class="settings-control">
            <label class="toggle"><input type="checkbox" id="s-incremental" ${cfg.store.incremental_scan ? 'checked' : ''}><span class="toggle-track"></span></label>
          </div>
        </div>
      </div>

      <!-- GC -->
      <div class="settings-section">
        <h2 class="settings-title">垃圾回收</h2>
        <div class="settings-row">
          <div><div class="settings-label">隔离保留天数</div><div class="settings-desc">文件进入隔离区后保留多少天再永久删除</div></div>
          <div class="settings-control">
            <input class="input" type="number" id="s-gc-days" value="${cfg.gc.quarantine_days}" style="width:80px" min="1" max="365">
          </div>
        </div>
        <div class="settings-row">
          <div><div class="settings-label">回收前确认</div><div class="settings-desc">执行 GC 前弹出确认对话框</div></div>
          <div class="settings-control">
            <label class="toggle"><input type="checkbox" id="s-gc-confirm" ${cfg.gc.confirm_before_gc ? 'checked' : ''}><span class="toggle-track"></span></label>
          </div>
        </div>
      </div>

      <!-- Web UI -->
      <div class="settings-section">
        <h2 class="settings-title">Web UI</h2>
        <div class="settings-row">
          <div><div class="settings-label">监听端口</div><div class="settings-desc">HTTP 服务监听端口（默认 8234）</div></div>
          <div class="settings-control">
            <input class="input" type="number" id="s-port" value="${cfg.ui.port}" style="width:100px" min="1024" max="65535">
          </div>
        </div>
        <div class="settings-row">
          <div><div class="settings-label">监听地址</div><div class="settings-desc">127.0.0.1 仅本机；0.0.0.0 允许局域网访问</div></div>
          <div class="settings-control">
            <select class="select" id="s-host" style="width:160px">
              <option ${cfg.ui.host === '127.0.0.1' ? 'selected' : ''}>127.0.0.1</option>
              <option ${cfg.ui.host === '0.0.0.0'   ? 'selected' : ''}>0.0.0.0</option>
            </select>
          </div>
        </div>
        <div class="settings-row">
          <div><div class="settings-label">启动后自动打开浏览器</div></div>
          <div class="settings-control">
            <label class="toggle"><input type="checkbox" id="s-open-browser" ${cfg.ui.open_browser ? 'checked' : ''}><span class="toggle-track"></span></label>
          </div>
        </div>
      </div>

      <div style="display:flex;gap:10px;margin-top:8px">
        <button class="btn btn-primary" id="btn-save">保存设置</button>
        <button class="btn btn-ghost" id="btn-reset">重置</button>
      </div>

      <div class="danger-zone">
        <div class="danger-zone-title">⚠ 危险操作</div>
        <div style="display:flex;gap:10px;flex-wrap:wrap">
          <button class="btn btn-danger btn-sm" id="btn-gc-now">立即执行垃圾回收</button>
          <button class="btn btn-danger btn-sm" id="btn-reset-db" disabled title="功能规划中">重置数据库（清除全部索引）</button>
        </div>
      </div>
    `;

    document.getElementById('btn-save').addEventListener('click', saveSettings);
    document.getElementById('btn-reset').addEventListener('click', loadSettings);
    document.getElementById('btn-gc-now').addEventListener('click', async () => {
      try {
        await api.gcRun({});
        toast('垃圾回收完成', 'success');
      } catch (e) {
        toast('失败: ' + e.message, 'error');
      }
    });
  }

  async function saveSettings() {
    const body = {
      store: {
        root: document.getElementById('s-root').value,
        auto_scan_on_start: document.getElementById('s-auto-scan').checked,
        watch_enabled: document.getElementById('s-watch').checked,
        incremental_scan: document.getElementById('s-incremental').checked,
      },
      gc: {
        quarantine_days: parseInt(document.getElementById('s-gc-days').value),
        confirm_before_gc: document.getElementById('s-gc-confirm').checked,
      },
      ui: {
        port: parseInt(document.getElementById('s-port').value),
        host: document.getElementById('s-host').value,
        open_browser: document.getElementById('s-open-browser').checked,
      },
    };

    try {
      await api.saveSettings(body);
      cfg = body;
      toast('设置已保存（部分设置需重启生效）', 'success');
    } catch (e) {
      toast('保存失败: ' + e.message, 'error');
    }
  }

  loadSettings();
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
