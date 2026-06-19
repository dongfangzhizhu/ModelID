// pages/proxy.js — LAN proxy endpoint info
import { copyText, toast } from '../main.js';

export function render(container) {
  const host  = location.hostname;
  const port  = location.port || '8234';
  const wsUrl = `ws://${host}:${port}/ws`;
  const apiUrl = `http://${host}:${port}/api/v1`;
  const uiUrl  = `http://${host}:${port}/`;

  container.innerHTML = `
    <div class="page-header">
      <h1 class="page-title">局域网代理</h1>
      <p class="page-subtitle">在同一局域网的其他设备上访问此 Web UI，或接入外部工具</p>
    </div>
    <div class="page-body">
      <div style="max-width:700px">
        <div class="proxy-card">
          <h3 style="font-size:14px;font-weight:700;margin-bottom:8px">Web UI 地址</h3>
          <div class="proxy-endpoint">
            <span>${escHtml(uiUrl)}</span>
            <button class="copy-btn" title="复制" onclick="copyAndToast('${escHtml(uiUrl)}')">⎘</button>
          </div>
          <p style="font-size:12px;color:var(--text-muted)">在浏览器中直接打开，可在局域网任意设备访问管理界面。</p>
        </div>

        <div class="proxy-card">
          <h3 style="font-size:14px;font-weight:700;margin-bottom:8px">REST API</h3>
          <div class="proxy-endpoint">
            <span>${escHtml(apiUrl)}</span>
            <button class="copy-btn" title="复制" onclick="copyAndToast('${escHtml(apiUrl)}')">⎘</button>
          </div>
          <p style="font-size:12px;color:var(--text-muted)">通过 HTTP/JSON 访问全部接口，供脚本或外部工具集成使用。</p>
        </div>

        <div class="proxy-card">
          <h3 style="font-size:14px;font-weight:700;margin-bottom:8px">WebSocket 事件流</h3>
          <div class="proxy-endpoint">
            <span>${escHtml(wsUrl)}</span>
            <button class="copy-btn" title="复制" onclick="copyAndToast('${escHtml(wsUrl)}')">⎘</button>
          </div>
          <p style="font-size:12px;color:var(--text-muted)">实时接收扫描进度、下载状态等事件推送，JSON 格式，包含 <code>type</code> 字段。</p>
        </div>

        <div class="card" style="margin-top:20px">
          <div class="card-header"><span class="card-title">API 速查</span></div>
          <div class="card-body">
            <table class="data-table" style="font-size:12px">
              <thead><tr><th>方法</th><th>路径</th><th>说明</th></tr></thead>
              <tbody>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/stats</td>      <td>存储总览统计</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/models</td>     <td>模型列表（分页）</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/models/:hash</td><td>模型详情</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/dupes</td>      <td>重复文件组</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/dupes/dedup</td><td>去重操作（预览/执行）</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/downloads</td>  <td>下载记录</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/scan/trigger</td><td>触发扫描</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/gc/preview</td> <td>GC 预览</td></tr>
                <tr><td><span class="badge badge-blue">POST</span></td>  <td class="mono">/api/v1/gc/run</td>     <td>执行垃圾回收</td></tr>
                <tr><td><span class="badge badge-green">GET</span></td>  <td class="mono">/api/v1/settings</td>   <td>读取配置</td></tr>
                <tr><td><span class="badge badge-orange">PUT</span></td> <td class="mono">/api/v1/settings</td>   <td>更新配置</td></tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </div>
  `;

  window.copyAndToast = (url) => {
    copyText(url);
    toast('已复制到剪贴板', 'success');
  };
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}
