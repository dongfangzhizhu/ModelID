// pages/proxy.js — LAN proxy endpoint info
import { copyText, toast, t } from '../main.js';

export function render(container) {
  const host   = location.hostname;
  const port   = location.port || '8234';
  const wsUrl  = `ws://${host}:${port}/ws`;
  const apiUrl = `http://${host}:${port}/api/v1`;
  const uiUrl  = `http://${host}:${port}/`;

  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">${t('proxy.title')}</h1>
      <p class="page-subtitle">${t('proxy.subtitle')}</p>
    </div>
    <div class="page-body">
      <div style="max-width:700px">

        <div class="proxy-card">
          <h3 style="font-size:14px;font-weight:700;margin-bottom:8px">${t('proxy.ui.title')}</h3>
          <div class="proxy-endpoint">
            <span>${escHtml(uiUrl)}</span>
            <button class="copy-btn" title="${t('common.copy')}" id="copy-ui">⎘</button>
          </div>
          <p style="font-size:12px;color:var(--text-muted)">${t('proxy.ui.desc')}</p>
        </div>

        <div class="proxy-card">
          <h3 style="font-size:14px;font-weight:700;margin-bottom:8px">${t('proxy.api.title')}</h3>
          <div class="proxy-endpoint">
            <span>${escHtml(apiUrl)}</span>
            <button class="copy-btn" title="${t('common.copy')}" id="copy-api">⎘</button>
          </div>
          <p style="font-size:12px;color:var(--text-muted)">${t('proxy.api.desc')}</p>
        </div>

        <div class="proxy-card">
          <h3 style="font-size:14px;font-weight:700;margin-bottom:8px">${t('proxy.ws.title')}</h3>
          <div class="proxy-endpoint">
            <span>${escHtml(wsUrl)}</span>
            <button class="copy-btn" title="${t('common.copy')}" id="copy-ws">⎘</button>
          </div>
          <p style="font-size:12px;color:var(--text-muted)">${t('proxy.ws.desc')}</p>
        </div>

        <div class="card" style="margin-top:20px">
          <div class="card-header"><span class="card-title">${t('proxy.ref.title')}</span></div>
          <div class="card-body">
            <table class="data-table" style="font-size:12px">
              <thead>
                <tr>
                  <th>${t('proxy.col.method')}</th>
                  <th>${t('proxy.col.path')}</th>
                  <th>${t('proxy.col.desc')}</th>
                </tr>
              </thead>
              <tbody>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/stats</td>           <td>${t('proxy.api.stats')}</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/models</td>          <td>${t('proxy.api.models')}</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/models/:hash</td>    <td>${t('proxy.api.model')}</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/dupes</td>           <td>${t('proxy.api.dupes')}</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/dupes/dedup</td>     <td>${t('proxy.api.dedup')}</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/downloads</td>       <td>${t('proxy.api.dls')}</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/scan/trigger</td>   <td>${t('proxy.api.scan')}</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/gc/preview</td>     <td>${t('proxy.api.gcprev')}</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/gc/run</td>         <td>${t('proxy.api.gc')}</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/settings</td>       <td>${t('proxy.api.set_get')}</td></tr>
                <tr><td><span class="badge badge-orange">PUT</span></td> <td class="mono">/api/v1/settings</td>       <td>${t('proxy.api.set_put')}</td></tr>
              </tbody>
            </table>
          </div>
        </div>

      </div>
    </div>
  `;

  // Bind copy buttons after DOM inserted
  const copies = { 'copy-ui': uiUrl, 'copy-api': apiUrl, 'copy-ws': wsUrl };
  for (const [id, url] of Object.entries(copies)) {
    document.getElementById(id)?.addEventListener('click', () => {
      copyText(url);
      toast(t('common.copied'), 'success');
    });
  }
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
