// main.js — Router, EventBus, WebSocket manager, global utilities

// ─── Utilities ───────────────────────────────────────────────────────────────

export function fmtBytes(bytes) {
  if (!bytes && bytes !== 0) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0, v = Number(bytes);
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export function fmtRelTime(isoStr) {
  if (!isoStr) return '—';
  const diff = Date.now() - new Date(isoStr).getTime();
  if (diff < 60000)   return '刚刚';
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`;
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`;
  return `${Math.floor(diff / 86400000)} 天前`;
}

export function fmtSpeed(bps) { return fmtBytes(bps) + '/s'; }

export function copyText(text) {
  navigator.clipboard.writeText(text).catch(() => {});
}

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
    document.getElementById('dialog-title').textContent = title;
    document.getElementById('dialog-body').textContent  = body;

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

const wsIndicator = () => document.getElementById('ws-indicator');

function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(`${proto}//${location.host}/ws`);

  ws.onopen = () => {
    const el = wsIndicator();
    if (el) { el.className = 'ws-indicator connected'; el.querySelector('.ws-text').textContent = '已连接'; }
  };

  ws.onmessage = (e) => {
    try {
      const msg = JSON.parse(e.data);
      events.emit(msg.type, msg);
    } catch { /* ignore parse errors */ }
  };

  ws.onclose = () => {
    const el = wsIndicator();
    if (el) { el.className = 'ws-indicator error'; el.querySelector('.ws-text').textContent = '已断开'; }
    // Reconnect with backoff
    setTimeout(connectWs, 5000);
  };

  ws.onerror = () => {
    const el = wsIndicator();
    if (el) { el.className = 'ws-indicator error'; el.querySelector('.ws-text').textContent = '连接失败'; }
  };
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

  if (!loader) {
    location.hash = '#/dashboard';
    return;
  }

  // Cleanup previous page
  if (currentCleanup) { currentCleanup(); currentCleanup = null; }

  // Update active nav item
  document.querySelectorAll('.nav-item').forEach(el => {
    el.classList.toggle('active', el.dataset.route === path);
  });

  // Show loading state
  const container = document.getElementById('page-content');
  container.innerHTML = '<div class="loading-state"><div class="spinner"></div><p>加载中...</p></div>';

  try {
    const mod = await loader();
    container.innerHTML = '';
    currentCleanup = mod.render(container) || null;
  } catch (err) {
    container.innerHTML = `<div class="page-body"><div class="empty-state"><div class="empty-icon">⚠</div><p class="empty-title">页面加载失败</p><p class="empty-sub">${err.message}</p></div></div>`;
  }
}

// ─── Init ────────────────────────────────────────────────────────────────────

window.addEventListener('hashchange', navigate);
document.addEventListener('DOMContentLoaded', () => {
  connectWs();
  navigate();
});
