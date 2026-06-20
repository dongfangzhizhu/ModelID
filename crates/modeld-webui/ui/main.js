// main.js — Router, EventBus, WebSocket manager, global utilities
import { t, buildLangToggle, onLangChange } from './i18n.js';

// ─── Utilities ───────────────────────────────────────────────────────────────

export function fmtBytes(bytes) {
  if (!bytes && bytes !== 0) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0, v = Number(bytes);
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export function fmtRelTime(isoStr) {
  if (!isoStr) return t('common.none');
  const diff = Date.now() - new Date(isoStr).getTime();
  if (diff < 60000)    return t('common.just_now');
  if (diff < 3600000)  return t('common.minutes_ago', { n: Math.floor(diff / 60000) });
  if (diff < 86400000) return t('common.hours_ago',   { n: Math.floor(diff / 3600000) });
  return t('common.days_ago', { n: Math.floor(diff / 86400000) });
}

export function copyText(text) {
  navigator.clipboard.writeText(text).catch(() => {});
}

// Re-export t so pages can import from main.js if desired
export { t } from './i18n.js';

// ─── Toast notifications ─────────────────────────────────────────────────────

export function toast(message, type = 'info', durationMs = 3000) {
  const container = document.getElementById('toast-container');
  const el = document.createElement('div');
  el.className = `toast ${type}`;
  el.textContent = message;
  container.appendChild(el);
  setTimeout(() => el.remove(), durationMs);
}

// ─── Confirm dialog ───────────────────────────────────────────────────────────

export function confirm(title, body) {
  return new Promise((resolve) => {
    const dialog  = document.getElementById('confirm-dialog');
    document.getElementById('dialog-title').textContent   = title;
    document.getElementById('dialog-body').textContent    = body;
    document.getElementById('dialog-confirm').textContent = t('dialog.confirm');
    document.getElementById('dialog-cancel').textContent  = t('dialog.cancel');

    const onConfirm = () => { dialog.close(); resolve(true);  cleanup(); };
    const onCancel  = () => { dialog.close(); resolve(false); cleanup(); };
    const cleanup   = () => {
      document.getElementById('dialog-confirm').removeEventListener('click', onConfirm);
      document.getElementById('dialog-cancel').removeEventListener('click',  onCancel);
    };

    document.getElementById('dialog-confirm').addEventListener('click', onConfirm);
    document.getElementById('dialog-cancel').addEventListener('click',  onCancel);
    dialog.showModal();
  });
}

// ─── EventBus ────────────────────────────────────────────────────────────────

class EventBus {
  constructor() { this._listeners = {}; }

  on(type, fn) {
    (this._listeners[type] ??= []).push(fn);
    return () => this.off(type, fn);
  }

  off(type, fn) {
    this._listeners[type] = (this._listeners[type] ?? []).filter(f => f !== fn);
  }

  emit(type, payload) {
    (this._listeners[type] ?? []).forEach(fn => fn(payload));
  }
}

export const events = new EventBus();

// ─── WebSocket client ─────────────────────────────────────────────────────────

function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(`${proto}//${location.host}/ws`);

  ws.onopen = () => setWsStatus('connected');
  ws.onmessage = (e) => {
    try {
      const msg = JSON.parse(e.data);
      events.emit(msg.type, msg);
    } catch { /* ignore parse errors */ }
  };
  ws.onclose = () => {
    setWsStatus('disconnected');
    setTimeout(connectWs, 5000);
  };
  ws.onerror = () => setWsStatus('error');
}

function setWsStatus(status) {
  const el = document.getElementById('ws-indicator');
  if (!el) return;
  el.className = `ws-indicator ${status === 'connected' ? 'connected' : status === 'error' ? 'error' : ''}`;
  el.querySelector('.ws-text').textContent = t(`ws.${status}`);
}

// ─── Sidebar nav labels ───────────────────────────────────────────────────────

const NAV_ITEMS = [
  { route: '/dashboard', labelKey: 'nav.dashboard', icon: '◈', id: 'nav-dashboard' },
  { route: '/dupes',     labelKey: 'nav.dupes',     icon: '⊕', id: 'nav-dupes'    },
  { route: '/library',   labelKey: 'nav.library',   icon: '◫', id: 'nav-library'  },
  { route: '/downloads', labelKey: 'nav.downloads', icon: '⬇', id: 'nav-downloads'},
  { route: '/refs',      labelKey: 'nav.refs',       icon: '◉', id: 'nav-refs'    },
  { route: '/proxy',     labelKey: 'nav.proxy',      icon: '⟳', id: 'nav-proxy'  },
  { route: '/settings',  labelKey: 'nav.settings',   icon: '⚙', id: 'nav-settings'},
];

function buildSidebar() {
  const brand = document.querySelector('.brand-sub');
  if (brand) brand.textContent = t('nav.brand.sub');

  const nav = document.querySelector('.sidebar-nav');
  nav.innerHTML = '';

  for (const item of NAV_ITEMS) {
    const a = document.createElement('a');
    a.href = `#${item.route}`;
    a.className = 'nav-item';
    a.dataset.route = item.route;
    a.id = item.id;
    a.innerHTML = `
      <span class="nav-icon">${item.icon}</span>
      <span class="nav-label">${t(item.labelKey)}</span>
      ${item.route === '/dupes' ? '<span class="nav-badge" id="badge-dupes" style="display:none"></span>' : ''}
    `;
    nav.appendChild(a);
  }

  // Re-apply active state
  const hash = location.hash.replace('#', '') || '/dashboard';
  document.querySelectorAll('.nav-item').forEach(el => {
    el.classList.toggle('active', el.dataset.route === hash);
  });
}

function buildSidebarFooter() {
  const footer = document.querySelector('.sidebar-footer');
  footer.innerHTML = `
    <div class="ws-indicator" id="ws-indicator" title="WebSocket">
      <span class="ws-dot"></span>
      <span class="ws-text">${t('ws.connecting')}</span>
    </div>
  `;
  footer.appendChild(buildLangToggle());
}

// ─── Hash Router ─────────────────────────────────────────────────────────────

const routes = {
  '/dashboard': () => import('./pages/dashboard.js'),
  '/dupes':     () => import('./pages/dupes.js'),
  '/library':   () => import('./pages/library.js'),
  '/downloads': () => import('./pages/downloads.js'),
  '/refs':      () => import('./pages/refs.js'),
  '/proxy':     () => import('./pages/proxy.js'),
  '/settings':  () => import('./pages/settings.js'),
};

let currentCleanup = null;

async function navigate() {
  const hash = location.hash.replace('#', '') || '/dashboard';
  const path = hash.startsWith('/') ? hash : '/' + hash;
  const loader = routes[path];

  if (!loader) { location.hash = '#/dashboard'; return; }

  if (currentCleanup) { currentCleanup(); currentCleanup = null; }

  document.querySelectorAll('.nav-item').forEach(el => {
    el.classList.toggle('active', el.dataset.route === path);
  });

  const container = document.getElementById('page-content');
  container.innerHTML = `<div class="loading-state"><div class="spinner"></div><p>${t('common.loading')}</p></div>`;

  try {
    const mod = await loader();
    container.innerHTML = '';
    currentCleanup = mod.render(container) || null;
  } catch (err) {
    container.innerHTML = `
      <div class="page-body">
        <div class="empty-state">
          <div class="empty-icon">⚠</div>
          <p class="empty-title">${t('common.load_failed')}</p>
          <p class="empty-sub">${err.message}</p>
        </div>
      </div>`;
  }
}

// ─── Init ────────────────────────────────────────────────────────────────────

window.addEventListener('hashchange', navigate);

document.addEventListener('DOMContentLoaded', () => {
  buildSidebar();
  buildSidebarFooter();
  connectWs();
  navigate();

  // When lang changes, rebuild sidebar labels + WS status text + re-navigate
  onLangChange(() => {
    buildSidebar();
    buildSidebarFooter();
    // Re-navigate to re-render current page in new language
    navigate();
  });
});
