// i18n.js — Internationalization for modeld Web UI
// Supports: English (en), Chinese Simplified (zh)
// Language detection order: localStorage → browser language → 'en'

const translations = {
  en: {
    // Navigation
    'nav.dashboard':  'Dashboard',
    'nav.dupes':      'Duplicates',
    'nav.library':    'Library',
    'nav.downloads':  'Downloads',
    'nav.refs':       'References',
    'nav.proxy':      'LAN Proxy',
    'nav.settings':   'Settings',
    'nav.brand.sub':  'Model Manager',

    // Connection status
    'ws.connecting':  'Connecting…',
    'ws.connected':   'Connected',
    'ws.disconnected':'Disconnected',
    'ws.error':       'Connection failed',

    // Common
    'common.loading':     'Loading…',
    'common.load_failed': 'Failed to load',
    'common.cancel':      'Cancel',
    'common.confirm':     'Confirm',
    'common.save':        'Save',
    'common.reset':       'Reset',
    'common.copy':        'Copy',
    'common.copied':      'Copied!',
    'common.search':      'Search…',
    'common.all':         'All',
    'common.sort':        'Sort',
    'common.yes':         'Yes',
    'common.no':          'No',
    'common.none':        '—',
    'common.just_now':    'Just now',
    'common.minutes_ago': '{n} min ago',
    'common.hours_ago':   '{n} hr ago',
    'common.days_ago':    '{n} days ago',

    // Dashboard
    'dash.title':           'Dashboard',
    'dash.subtitle':        'Storage overview and quick actions',
    'dash.stat.models':     'Indexed Models',
    'dash.stat.models.sub': 'model files',
    'dash.stat.waste':      'Reclaimable',
    'dash.stat.waste.sub':  'from duplicates',
    'dash.stat.size':       'Total Size',
    'dash.stat.size.sub':   'all models',
    'dash.stat.scan':       'Last Scan',
    'dash.stat.scan.sub':   'scan time',
    'dash.stat.scan.never': 'Never',
    'dash.btn.scan':        '↻ Scan Now',
    'dash.btn.dedup':       '⊕ Deduplicate',
    'dash.btn.gc':          '♺ Garbage Collect',
    'dash.scan.walking':    'Walking',
    'dash.scan.hashing':    'Hashing',
    'dash.scan.indexing':   'Indexing',
    'dash.scan.done':       'Done',
    'dash.scan.scanning':   'Scanning…',
    'dash.scan.started':    'Scan started',
    'dash.scan.start_fail': 'Failed to start scan: {msg}',
    'dash.scan.complete':   'Scan complete',
    'dash.scan.files':      '{n} files',
    'dash.gc.none':         'No reclaimable files',
    'dash.gc.confirm.title':'Confirm Garbage Collection',
    'dash.gc.confirm.body': 'Permanently delete {n} quarantined file(s), reclaiming {size}. This cannot be undone.',
    'dash.gc.done':         'Garbage collection complete',
    'dash.gc.fail':         'Operation failed: {msg}',
    'dash.storage.title':   'Storage Distribution',
    'dash.storage.count':   '{n} frontend(s)',
    'dash.storage.empty':   'No models yet',
    'dash.storage.empty.sub': 'Click "Scan Now" to index model files',
    'dash.storage.col.frontend':  'Frontend',
    'dash.storage.col.models':    'Models',
    'dash.storage.col.size':      'Size',
    'dash.storage.col.waste':     'Duplicate Waste',

    // Dupes
    'dupes.title':          'Duplicates',
    'dupes.subtitle':       'Identify and eliminate duplicate model files to reclaim disk space',
    'dupes.search':         'Search filename…',
    'dupes.sort.waste':     'Sort by wasted space',
    'dupes.sort.copies':    'Sort by copy count',
    'dupes.sort.size':      'Sort by file size',
    'dupes.summary':        '{n} group(s), wasting {size}',
    'dupes.btn.dedup_all':  '⊕ Deduplicate All (Preview)',
    'dupes.group.keep':     'Keep',
    'dupes.group.preview':  'Preview Dedup',
    'dupes.preview.msg':    'Preview: keep {name}, save {size}',
    'dupes.preview.fail':   'Preview failed: {msg}',
    'dupes.confirm.title':  'Confirm Dedup Operation',
    'dupes.confirm.body':   'Deduplicate {n} group(s) via hard links, saving approx. {size}. File content is unchanged.',
    'dupes.notice.wip':     'Dedup execution will be available in the full release',
    'dupes.apply.done':     'Deduplicated {n} group(s), saved {size}',
    'dupes.fail':           'Operation failed: {msg}',
    'dupes.empty':          'No duplicate files found',
    'dupes.empty.sub':      'Storage is already optimal',
    'dupes.copies':         '×{n} copies',

    // Library
    'lib.title':        'Model Library',
    'lib.subtitle':     'All indexed AI model files',
    'lib.search':       'Search name or hash…',
    'lib.type.all':     'All types',
    'lib.type.ckpt':    'Checkpoint',
    'lib.type.lora':    'LoRA',
    'lib.type.vae':     'VAE',
    'lib.type.cn':      'ControlNet',
    'lib.sort.recent':  'Recently seen',
    'lib.sort.size':    'File size',
    'lib.sort.name':    'Name',
    'lib.sort.refs':    'Ref count',
    'lib.orphan':       'Orphans only',
    'lib.count':        '{n} model(s) total',
    'lib.empty':        'No matching models',
    'lib.col.refs':     '{n} ref(s)',
    'lib.orphan.badge': 'Orphan',
    'lib.prev_page':    '← Prev',
    'lib.next_page':    'Next →',
    'lib.detail.title': 'Model Detail',
    'lib.detail.hash':  'BLAKE3',
    'lib.detail.format':'Format',
    'lib.detail.arch':  'Architecture',
    'lib.detail.size':  'Size',
    'lib.detail.refs':  'References',
    'lib.detail.found': 'Discovered',
    'lib.detail.paths': 'All Paths',
    'lib.detail.fail':  'Load failed: {msg}',

    // Downloads
    'dl.title':         'Downloads',
    'dl.subtitle':      'Model download history and progress',
    'dl.filter.all':    'All',
    'dl.filter.active': 'Downloading',
    'dl.filter.done':   'Completed',
    'dl.filter.fail':   'Failed',
    'dl.count':         '{n} record(s)',
    'dl.empty':         'No download records',
    'dl.empty.sub':     'Downloads initiated from frontends will appear here',
    'dl.status.completed':  'Done',
    'dl.status.downloading':'Downloading',
    'dl.status.failed':     'Failed',
    'dl.status.pending':    'Pending',
    'dl.status.paused':     'Paused',

    // References
    'refs.title':       'Reference Graph',
    'refs.subtitle':    'Model × frontend reference relationships',
    'refs.search':      'Search model name or path…',
    'refs.fe.all':      'All frontends',
    'refs.count':       '{n} model(s)',
    'refs.empty':       'No matching references',
    'refs.col.model':   'Model',
    'refs.col.size':    'Size',
    'refs.col.fe':      'Frontend',
    'refs.col.refs':    'Refs',
    'refs.col.orphan':  'Orphan',

    // Proxy
    'proxy.title':      'LAN Proxy',
    'proxy.subtitle':   'Access this Web UI from other devices on the same LAN, or connect external tools',
    'proxy.ui.title':   'Web UI URL',
    'proxy.ui.desc':    'Open directly in a browser — accessible from any device on your network.',
    'proxy.api.title':  'REST API',
    'proxy.api.desc':   'Access all endpoints via HTTP/JSON for script or tool integration.',
    'proxy.ws.title':   'WebSocket Event Stream',
    'proxy.ws.desc':    'Receive real-time events (scan progress, download status) in JSON with a `type` field.',
    'proxy.ref.title':  'API Quick Reference',
    'proxy.col.method': 'Method',
    'proxy.col.path':   'Path',
    'proxy.col.desc':   'Description',
    'proxy.api.stats':  'Storage overview',
    'proxy.api.models': 'Model list (paginated)',
    'proxy.api.model':  'Model detail',
    'proxy.api.dupes':  'Duplicate groups',
    'proxy.api.dedup':  'Dedup (preview/execute)',
    'proxy.api.dls':    'Download history',
    'proxy.api.scan':   'Trigger scan',
    'proxy.api.gcprev': 'GC preview',
    'proxy.api.gc':     'Run GC',
    'proxy.api.set_get':'Read config',
    'proxy.api.set_put':'Update config',

    // Settings
    'set.title':            'Settings',
    'set.subtitle':         'Store path, GC policy, Web UI options',
    'set.store.section':    'Storage',
    'set.store.root':       'Store Root',
    'set.store.root.desc':  'Location of model database and CAS object store',
    'set.store.autoscan':   'Auto-scan on Start',
    'set.store.autoscan.desc': 'Scan store directory each time the daemon starts',
    'set.store.watch':      'File Watch (inotify)',
    'set.store.watch.desc': 'Monitor file changes and auto-update index',
    'set.store.incremental':'Incremental Scan',
    'set.store.incr.desc':  'Only scan new/changed files (faster)',
    'set.gc.section':       'Garbage Collection',
    'set.gc.days':          'Quarantine Retention (days)',
    'set.gc.days.desc':     'Days before permanently deleting quarantined files',
    'set.gc.confirm':       'Confirm before GC',
    'set.gc.confirm.desc':  'Show confirmation dialog before running GC',
    'set.ui.section':       'Web UI',
    'set.ui.port':          'Listen Port',
    'set.ui.port.desc':     'HTTP service port (default 8234)',
    'set.ui.host':          'Listen Address',
    'set.ui.host.desc':     '127.0.0.1 = local only;  0.0.0.0 = allow LAN',
    'set.ui.browser':       'Open Browser on Start',
    'set.ui.lang':          'Interface Language',
    'set.ui.lang.desc':     'Language used throughout the Web UI',
    'set.save':             'Save Settings',
    'set.reset':            'Reset',
    'set.save.ok':          'Settings saved (some changes require restart)',
    'set.save.fail':        'Save failed: {msg}',
    'set.gc.now':           'Run GC Now',
    'set.gc.done':          'Garbage collection complete',
    'set.gc.fail':          'Failed: {msg}',
    'set.reset_db':         'Reset Database (clear all indexes)',
    'set.danger.title':     '⚠ Danger Zone',

    // Dialog
    'dialog.confirm': 'Confirm',
    'dialog.cancel':  'Cancel',
  },

  zh: {
    // Navigation
    'nav.dashboard':  '仪表板',
    'nav.dupes':      '重复文件',
    'nav.library':    '模型库',
    'nav.downloads':  '下载器',
    'nav.refs':       '引用图',
    'nav.proxy':      '局域网代理',
    'nav.settings':   '设置',
    'nav.brand.sub':  '模型管理器',

    // Connection status
    'ws.connecting':  '连接中…',
    'ws.connected':   '已连接',
    'ws.disconnected':'已断开',
    'ws.error':       '连接失败',

    // Common
    'common.loading':     '加载中…',
    'common.load_failed': '加载失败',
    'common.cancel':      '取消',
    'common.confirm':     '确认',
    'common.save':        '保存',
    'common.reset':       '重置',
    'common.copy':        '复制',
    'common.copied':      '已复制！',
    'common.search':      '搜索…',
    'common.all':         '全部',
    'common.sort':        '排序',
    'common.yes':         '是',
    'common.no':          '否',
    'common.none':        '—',
    'common.just_now':    '刚刚',
    'common.minutes_ago': '{n} 分钟前',
    'common.hours_ago':   '{n} 小时前',
    'common.days_ago':    '{n} 天前',

    // Dashboard
    'dash.title':           '仪表板',
    'dash.subtitle':        '存储概览与快捷操作',
    'dash.stat.models':     '已索引模型',
    'dash.stat.models.sub': '个模型文件',
    'dash.stat.waste':      '可节省空间',
    'dash.stat.waste.sub':  '重复文件占用',
    'dash.stat.size':       '总占用空间',
    'dash.stat.size.sub':   '所有模型',
    'dash.stat.scan':       '上次扫描',
    'dash.stat.scan.sub':   '扫描时间',
    'dash.stat.scan.never': '未扫描',
    'dash.btn.scan':        '⟳ 立即扫描',
    'dash.btn.dedup':       '⊕ 去重管理',
    'dash.btn.gc':          '♺ 垃圾回收',
    'dash.scan.walking':    '遍历文件',
    'dash.scan.hashing':    '计算哈希',
    'dash.scan.indexing':   '建立索引',
    'dash.scan.done':       '完成',
    'dash.scan.scanning':   '扫描中…',
    'dash.scan.started':    '扫描已启动',
    'dash.scan.start_fail': '扫描启动失败：{msg}',
    'dash.scan.complete':   '扫描完成',
    'dash.scan.files':      '{n} 个文件',
    'dash.gc.none':         '没有可回收的文件',
    'dash.gc.confirm.title':'确认垃圾回收',
    'dash.gc.confirm.body': '将永久删除 {n} 个隔离文件，释放 {size} 空间。此操作不可逆。',
    'dash.gc.done':         '垃圾回收完成',
    'dash.gc.fail':         '操作失败：{msg}',
    'dash.storage.title':   '存储分布',
    'dash.storage.count':   '{n} 个前端',
    'dash.storage.empty':   '暂无模型',
    'dash.storage.empty.sub': '点击"立即扫描"开始索引模型文件',
    'dash.storage.col.frontend':  '前端',
    'dash.storage.col.models':    '模型数',
    'dash.storage.col.size':      '占用空间',
    'dash.storage.col.waste':     '重复浪费',

    // Dupes
    'dupes.title':          '重复文件',
    'dupes.subtitle':       '发现并消除重复模型文件，释放磁盘空间',
    'dupes.search':         '搜索文件名…',
    'dupes.sort.waste':     '按浪费空间排序',
    'dupes.sort.copies':    '按副本数排序',
    'dupes.sort.size':      '按文件大小排序',
    'dupes.summary':        '{n} 组重复，浪费 {size}',
    'dupes.btn.dedup_all':  '⊕ 一键去重（预览）',
    'dupes.group.keep':     '保留',
    'dupes.group.preview':  '预览去重',
    'dupes.preview.msg':    '预览：保留 {name}，可节省 {size}',
    'dupes.preview.fail':   '预览失败：{msg}',
    'dupes.confirm.title':  '确认去重操作',
    'dupes.confirm.body':   '将对 {n} 组文件执行硬链接去重，预计节省 {size}。操作完成后重复文件将被替换为硬链接（原内容不变）。',
    'dupes.notice.wip':     '去重执行功能将在完整版本中可用',
    'dupes.apply.done':     '已对 {n} 组文件去重，节省 {size}',
    'dupes.fail':           '操作失败：{msg}',
    'dupes.empty':          '未发现重复文件',
    'dupes.empty.sub':      '当前存储空间使用已最优',
    'dupes.copies':         '×{n} 副本',

    // Library
    'lib.title':        '模型库',
    'lib.subtitle':     '全部已索引的 AI 模型文件',
    'lib.search':       '搜索模型名称或哈希…',
    'lib.type.all':     '所有类型',
    'lib.type.ckpt':    'Checkpoint',
    'lib.type.lora':    'LoRA',
    'lib.type.vae':     'VAE',
    'lib.type.cn':      'ControlNet',
    'lib.sort.recent':  '最近发现',
    'lib.sort.size':    '文件大小',
    'lib.sort.name':    '文件名',
    'lib.sort.refs':    '引用数',
    'lib.orphan':       '仅孤儿文件',
    'lib.count':        '共 {n} 个模型',
    'lib.empty':        '没有匹配的模型',
    'lib.col.refs':     '{n} 个引用',
    'lib.orphan.badge': '孤儿',
    'lib.prev_page':    '← 上一页',
    'lib.next_page':    '下一页 →',
    'lib.detail.title': '模型详情',
    'lib.detail.hash':  'BLAKE3',
    'lib.detail.format':'格式',
    'lib.detail.arch':  '架构',
    'lib.detail.size':  '大小',
    'lib.detail.refs':  '引用数',
    'lib.detail.found': '发现时间',
    'lib.detail.paths': '全部路径',
    'lib.detail.fail':  '加载失败：{msg}',

    // Downloads
    'dl.title':         '下载器',
    'dl.subtitle':      '模型下载历史与进度',
    'dl.filter.all':    '全部',
    'dl.filter.active': '下载中',
    'dl.filter.done':   '已完成',
    'dl.filter.fail':   '失败',
    'dl.count':         '共 {n} 条记录',
    'dl.empty':         '暂无下载记录',
    'dl.empty.sub':     '通过前端发起模型下载后将在此显示',
    'dl.status.completed':  '完成',
    'dl.status.downloading':'下载中',
    'dl.status.failed':     '失败',
    'dl.status.pending':    '等待',
    'dl.status.paused':     '暂停',

    // References
    'refs.title':       '引用图',
    'refs.subtitle':    '模型与前端的引用关系',
    'refs.search':      '搜索模型名或路径…',
    'refs.fe.all':      '所有前端',
    'refs.count':       '{n} 个模型',
    'refs.empty':       '没有匹配的引用',
    'refs.col.model':   '模型名',
    'refs.col.size':    '大小',
    'refs.col.fe':      '前端',
    'refs.col.refs':    '引用数',
    'refs.col.orphan':  '孤儿',

    // Proxy
    'proxy.title':      '局域网代理',
    'proxy.subtitle':   '在同一局域网的其他设备上访问此 Web UI，或接入外部工具',
    'proxy.ui.title':   'Web UI 地址',
    'proxy.ui.desc':    '在浏览器中直接打开，可在局域网任意设备访问管理界面。',
    'proxy.api.title':  'REST API',
    'proxy.api.desc':   '通过 HTTP/JSON 访问全部接口，供脚本或外部工具集成使用。',
    'proxy.ws.title':   'WebSocket 事件流',
    'proxy.ws.desc':    '实时接收扫描进度、下载状态等事件推送，JSON 格式，包含 `type` 字段。',
    'proxy.ref.title':  'API 速查',
    'proxy.col.method': '方法',
    'proxy.col.path':   '路径',
    'proxy.col.desc':   '说明',
    'proxy.api.stats':  '存储总览统计',
    'proxy.api.models': '模型列表（分页）',
    'proxy.api.model':  '模型详情',
    'proxy.api.dupes':  '重复文件组',
    'proxy.api.dedup':  '去重操作（预览/执行）',
    'proxy.api.dls':    '下载记录',
    'proxy.api.scan':   '触发扫描',
    'proxy.api.gcprev': 'GC 预览',
    'proxy.api.gc':     '执行垃圾回收',
    'proxy.api.set_get':'读取配置',
    'proxy.api.set_put':'更新配置',

    // Settings
    'set.title':            '设置',
    'set.subtitle':         '存储路径、GC 策略、Web UI 选项',
    'set.store.section':    '存储',
    'set.store.root':       '存储根目录',
    'set.store.root.desc':  '模型数据库与 CAS 对象存储位置',
    'set.store.autoscan':   '启动时自动扫描',
    'set.store.autoscan.desc':'每次启动 daemon 时扫描存储目录',
    'set.store.watch':      '文件监视（inotify）',
    'set.store.watch.desc': '实时监视文件变化并自动更新索引',
    'set.store.incremental':'增量扫描',
    'set.store.incr.desc':  '仅扫描新增或变化的文件（更快）',
    'set.gc.section':       '垃圾回收',
    'set.gc.days':          '隔离保留天数',
    'set.gc.days.desc':     '文件进入隔离区后保留多少天再永久删除',
    'set.gc.confirm':       '回收前确认',
    'set.gc.confirm.desc':  '执行 GC 前弹出确认对话框',
    'set.ui.section':       'Web UI',
    'set.ui.port':          '监听端口',
    'set.ui.port.desc':     'HTTP 服务监听端口（默认 8234）',
    'set.ui.host':          '监听地址',
    'set.ui.host.desc':     '127.0.0.1 仅本机；0.0.0.0 允许局域网访问',
    'set.ui.browser':       '启动后自动打开浏览器',
    'set.ui.lang':          '界面语言',
    'set.ui.lang.desc':     '整个 Web UI 使用的显示语言',
    'set.save':             '保存设置',
    'set.reset':            '重置',
    'set.save.ok':          '设置已保存（部分设置需重启生效）',
    'set.save.fail':        '保存失败：{msg}',
    'set.gc.now':           '立即执行垃圾回收',
    'set.gc.done':          '垃圾回收完成',
    'set.gc.fail':          '失败：{msg}',
    'set.reset_db':         '重置数据库（清除全部索引）',
    'set.danger.title':     '⚠ 危险操作',

    // Dialog
    'dialog.confirm': '确认',
    'dialog.cancel':  '取消',
  },
};

// ─── Language detection & persistence ────────────────────────────────────────

const STORAGE_KEY = 'modeld_ui_lang';
const SUPPORTED   = ['en', 'zh'];

function detectLang() {
  // 1. User explicit choice
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored && SUPPORTED.includes(stored)) return stored;

  // 2. Browser/system language
  const nav = (navigator.language || navigator.userLanguage || 'en').toLowerCase();
  if (nav.startsWith('zh')) return 'zh';

  // 3. Default
  return 'en';
}

let _lang = detectLang();

export function getLang() { return _lang; }

export function setLang(lang) {
  if (!SUPPORTED.includes(lang)) return;
  _lang = lang;
  localStorage.setItem(STORAGE_KEY, lang);
  // Notify subscribers
  _listeners.forEach(fn => fn(lang));
  // Re-render current page by re-triggering navigation
  window.dispatchEvent(new HashChangeEvent('hashchange'));
}

// ─── Subscription for live lang switch ───────────────────────────────────────

const _listeners = new Set();
export function onLangChange(fn) {
  _listeners.add(fn);
  return () => _listeners.delete(fn);
}

// ─── Translation function ─────────────────────────────────────────────────────

/**
 * Translate a key, with optional variable substitution.
 * @param {string} key   — translation key, e.g. 'dash.title'
 * @param {object} [vars]— substitution map, e.g. {n: 5, size: '1.2 GB'}
 * @returns {string}
 */
export function t(key, vars) {
  const dict = translations[_lang] ?? translations['en'];
  let str = dict[key] ?? translations['en'][key] ?? key;
  if (vars) {
    str = str.replace(/\{(\w+)\}/g, (_, k) => (vars[k] ?? ''));
  }
  return str;
}

// ─── Sidebar language toggle ──────────────────────────────────────────────────

/**
 * Build the lang toggle element to insert into the sidebar.
 * Call once after DOM is ready.
 */
export function buildLangToggle() {
  const el = document.createElement('button');
  el.className = 'lang-toggle';
  el.id = 'lang-toggle';
  el.setAttribute('title', 'Switch language / 切换语言');
  el.onclick = () => setLang(_lang === 'en' ? 'zh' : 'en');
  updateToggleLabel(el);
  _listeners.add(() => updateToggleLabel(el));
  return el;
}

function updateToggleLabel(el) {
  el.textContent = _lang === 'zh' ? '🌐 English' : '🌐 中文';
}
