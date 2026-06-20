// pages/settings.js
import { api } from '../api.js';
import { toast, t } from '../main.js';
import { getLang, setLang } from '../i18n.js';

export function render(container) {
  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">${t('set.title')}</h1>
      <p class="page-subtitle">${t('set.subtitle')}</p>
    </div>
    <div class="page-body" style="max-width:680px">
      <div id="settings-form">
        <div class="loading-state"><div class="spinner"></div><p>${t('common.loading')}</p></div>
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
    const currentLang = getLang();

    document.getElementById('settings-form').innerHTML = `

      <!-- Storage -->
      <div class="settings-section">
        <h2 class="settings-title">${t('set.store.section')}</h2>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.store.root')}</div>
            <div class="settings-desc">${t('set.store.root.desc')}</div>
          </div>
          <div class="settings-control">
            <input class="input" id="s-root" value="${escHtml(cfg.store.root)}" style="width:260px">
          </div>
        </div>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.store.autoscan')}</div>
            <div class="settings-desc">${t('set.store.autoscan.desc')}</div>
          </div>
          <div class="settings-control">
            <label class="toggle">
              <input type="checkbox" id="s-auto-scan" ${cfg.store.auto_scan_on_start ? 'checked' : ''}>
              <span class="toggle-track"></span>
            </label>
          </div>
        </div>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.store.watch')}</div>
            <div class="settings-desc">${t('set.store.watch.desc')}</div>
          </div>
          <div class="settings-control">
            <label class="toggle">
              <input type="checkbox" id="s-watch" ${cfg.store.watch_enabled ? 'checked' : ''}>
              <span class="toggle-track"></span>
            </label>
          </div>
        </div>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.store.incremental')}</div>
            <div class="settings-desc">${t('set.store.incr.desc')}</div>
          </div>
          <div class="settings-control">
            <label class="toggle">
              <input type="checkbox" id="s-incremental" ${cfg.store.incremental_scan ? 'checked' : ''}>
              <span class="toggle-track"></span>
            </label>
          </div>
        </div>
      </div>

      <!-- GC -->
      <div class="settings-section">
        <h2 class="settings-title">${t('set.gc.section')}</h2>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.gc.days')}</div>
            <div class="settings-desc">${t('set.gc.days.desc')}</div>
          </div>
          <div class="settings-control">
            <input class="input" type="number" id="s-gc-days"
              value="${cfg.gc.quarantine_days}" style="width:80px" min="1" max="365">
          </div>
        </div>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.gc.confirm')}</div>
            <div class="settings-desc">${t('set.gc.confirm.desc')}</div>
          </div>
          <div class="settings-control">
            <label class="toggle">
              <input type="checkbox" id="s-gc-confirm" ${cfg.gc.confirm_before_gc ? 'checked' : ''}>
              <span class="toggle-track"></span>
            </label>
          </div>
        </div>
      </div>

      <!-- Web UI -->
      <div class="settings-section">
        <h2 class="settings-title">${t('set.ui.section')}</h2>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.ui.port')}</div>
            <div class="settings-desc">${t('set.ui.port.desc')}</div>
          </div>
          <div class="settings-control">
            <input class="input" type="number" id="s-port"
              value="${cfg.ui.port}" style="width:100px" min="1024" max="65535">
          </div>
        </div>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.ui.host')}</div>
            <div class="settings-desc">${t('set.ui.host.desc')}</div>
          </div>
          <div class="settings-control">
            <select class="select" id="s-host" style="width:160px">
              <option ${cfg.ui.host === '127.0.0.1' ? 'selected' : ''}>127.0.0.1</option>
              <option ${cfg.ui.host === '0.0.0.0'   ? 'selected' : ''}>0.0.0.0</option>
            </select>
          </div>
        </div>

        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.ui.browser')}</div>
          </div>
          <div class="settings-control">
            <label class="toggle">
              <input type="checkbox" id="s-open-browser" ${cfg.ui.open_browser ? 'checked' : ''}>
              <span class="toggle-track"></span>
            </label>
          </div>
        </div>

        <!-- Language selector -->
        <div class="settings-row">
          <div>
            <div class="settings-label">${t('set.ui.lang')}</div>
            <div class="settings-desc">${t('set.ui.lang.desc')}</div>
          </div>
          <div class="settings-control">
            <select class="select" id="s-lang" style="width:160px">
              <option value="en" ${currentLang === 'en' ? 'selected' : ''}>English</option>
              <option value="zh" ${currentLang === 'zh' ? 'selected' : ''}>中文</option>
            </select>
          </div>
        </div>
      </div>

      <div style="display:flex;gap:10px;margin-top:8px">
        <button class="btn btn-primary" id="btn-save">${t('set.save')}</button>
        <button class="btn btn-ghost"   id="btn-reset">${t('set.reset')}</button>
      </div>

      <!-- Danger zone -->
      <div class="danger-zone">
        <div class="danger-zone-title">${t('set.danger.title')}</div>
        <div style="display:flex;gap:10px;flex-wrap:wrap">
          <button class="btn btn-danger btn-sm" id="btn-gc-now">${t('set.gc.now')}</button>
          <button class="btn btn-danger btn-sm" disabled title="Coming soon">${t('set.reset_db')}</button>
        </div>
      </div>
    `;

    // Language switcher — instant, no save needed (persisted in localStorage)
    document.getElementById('s-lang').addEventListener('change', (e) => {
      setLang(e.target.value);
      // Page will re-render automatically via hashchange triggered by setLang()
    });

    document.getElementById('btn-save').addEventListener('click',  saveSettings);
    document.getElementById('btn-reset').addEventListener('click', loadSettings);
    document.getElementById('btn-gc-now').addEventListener('click', async () => {
      try {
        await api.gcRun({});
        toast(t('set.gc.done'), 'success');
      } catch (e) {
        toast(t('set.gc.fail', { msg: e.message }), 'error');
      }
    });
  }

  async function saveSettings() {
    const body = {
      store: {
        root:               document.getElementById('s-root').value,
        auto_scan_on_start: document.getElementById('s-auto-scan').checked,
        watch_enabled:      document.getElementById('s-watch').checked,
        incremental_scan:   document.getElementById('s-incremental').checked,
      },
      gc: {
        quarantine_days:    parseInt(document.getElementById('s-gc-days').value),
        confirm_before_gc:  document.getElementById('s-gc-confirm').checked,
      },
      ui: {
        port:         parseInt(document.getElementById('s-port').value),
        host:         document.getElementById('s-host').value,
        open_browser: document.getElementById('s-open-browser').checked,
      },
    };

    try {
      await api.saveSettings(body);
      cfg = body;
      toast(t('set.save.ok'), 'success');
    } catch (e) {
      toast(t('set.save.fail', { msg: e.message }), 'error');
    }
  }

  loadSettings();
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
