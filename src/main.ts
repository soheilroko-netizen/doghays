import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import './styles.css';

// ── Constants ────────────────────────────────────────────────
const PING_INTERVAL_MS = 3000;
const STATUS_INTERVAL_MS = 2000;
const UPTIME_INTERVAL_MS = 1000;
/** Ping bars / triangle turn red at or above this latency */
const HIGH_PING_MS = 300;
/** Window sizes for settings panel open/closed (must match create_main_window in Rust) */
const WINDOW_HEIGHT_CLOSED = 780;
const WINDOW_HEIGHT_OPEN = 680;
const WINDOW_WIDTH = 500;
/** flagcdn.com 16x12 flag URL builder */
const FLAG_URL = (code: string) => `https://flagcdn.com/16x12/${code}.png`;
/** H2 bandwidth presets shared by UI and backend (keep in sync with apply_h2_preset) */
const H2_PRESETS: Record<string, { up: number; down: number }> = {
  adsl: { up: 4, down: 16 },
  '4g': { up: 15, down: 30 },
  '5g': { up: 40, down: 80 },
  max: { up: 80, down: 120 },
};

// ── Types ────────────────────────────────────────────────────
interface FullStatus {
  running: boolean;
  mode: string;
  server: string | null;
  uptime_secs: number;
  pid: number | null;
  traffic_up: number;
  traffic_down: number;
  total_up: number;
  total_down: number;
  log_lines: string[];
}

interface Config {
  server_address: string;
  ss_port: number;
  ss_password: string;
  stls_port: number;
  stls_password: string;
  stls_sni: string;
  mtu?: number;
  split_mode?: string;
  wow_apps?: string[];
  mode: string;
  h2_port: number;
  h2_password: string;
  h2_sni: string;
  h2_insecure: boolean;
  h2_obfs: string;
  h2_obfs_password: string;
  h2_up_mbps: number;
  h2_down_mbps: number;
  tun_stack?: string;
}

// ── Elements ─────────────────────────────────────────────
// Header elements
const serverSelectorWrapper = document.getElementById('server-selector-wrapper')!;
const serverSelectorTrigger = document.getElementById('server-selector-trigger')!;
const serverSelectorFlag = document.getElementById('server-selector-flag')!;
const serverSelectorText = document.getElementById('server-selector-text')!;
const serverSelectorOptions = document.getElementById('server-selector-options')!;
const protocolTabs = document.querySelectorAll('.protocol-tabs .tab');
const btnSettingsToggle = document.getElementById('btn-settings-toggle')!;
const btnLog = document.getElementById('btn-main-log')!;

// Split preset selector elements
const splitPresetSelector = document.getElementById('split-preset-selector')!;
const splitPresetCards = document.querySelectorAll('.split-preset-card');

// Status elements
const statusDot = document.getElementById('status-dot')!;
const statusText = document.getElementById('status-text')!;
const statusAddress = document.getElementById('status-address')!;
const statusCard = document.querySelector('.status-card')!;

// Metrics elements
const pingValue = document.getElementById('ping-value')!;
const lossValue = document.getElementById('loss-value')!;
const trafficUpValue = document.getElementById('traffic-up-value')!;
const trafficDownValue = document.getElementById('traffic-down-value')!;
const trafficUpTotal = document.getElementById('traffic-up-total')!;
const trafficDownTotal = document.getElementById('traffic-down-total')!;

const sparklineUp = document.getElementById('sparkline-up') as HTMLCanvasElement;
const sparklineDown = document.getElementById('sparkline-down') as HTMLCanvasElement;

// Controls elements
const btnStart = document.getElementById('btn-start') as HTMLButtonElement;
const btnStartText = document.getElementById('btn-start-text')!;
const btnStop = document.getElementById('btn-stop') as HTMLButtonElement;
const message = document.getElementById('message')!;

// Inline log elements
const logSection = document.getElementById('log-section')!;
const logToggle = document.getElementById('log-toggle')!;
const pingChartWrap = document.getElementById('ping-chart-wrap')!;
const pingHistCanvas = document.getElementById('ping-hist-canvas') as HTMLCanvasElement;
const pingStatAvg = document.getElementById('ping-stat-avg')!;
const pingStatJit = document.getElementById('ping-stat-jit')!;
const pingStatMin = document.getElementById('ping-stat-min')!;
const pingStatMax = document.getElementById('ping-stat-max')!;

// Settings panel
const settingsPanel = document.getElementById('settings-panel')!;

// Views
const mainView = document.getElementById('main-view')!;
const logView = document.getElementById('log-view')!;
const logContent = document.getElementById('log-content')!;
const btnRefreshLog = document.getElementById('btn-refresh-log')!;
const btnBackFromLog = document.getElementById('btn-back-from-log')!;

// Settings inputs
const wowInfoContainer = document.getElementById('wow-info-container')!;
const wowAppDiscord = document.getElementById('wow-app-discord') as HTMLInputElement;
const wowAppChrome = document.getElementById('wow-app-chrome') as HTMLInputElement;
const wowAppTelegram = document.getElementById('wow-app-telegram') as HTMLInputElement;
const settingMtu = document.getElementById('setting-mtu') as HTMLInputElement;
const settingTunStack = document.getElementById('setting-tun-stack') as HTMLSelectElement;
const btnSaveSettings = document.getElementById('btn-save-settings')!;
const btnDoh = document.getElementById('btn-doh') as HTMLButtonElement;

// ── Helpers ──────────────────────────────────────────────────
const SERVER_FLAGS: Record<string, string> = {
  netherlands: 'nl',
  germany: 'de',
  finland: 'fi',
};

function getServerFlag(server: string): string {
  const code = SERVER_FLAGS[server.split('-')[0]] || '';
  return code ? `<img src="${FLAG_URL(code)}" style="margin-right:4px;vertical-align:middle;" alt="${code.toUpperCase()}" />` : '';
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`;
}

function formatUptime(secs: number): string {
  if (!secs || secs < 1) return '-';
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}h ${m}m`;
}

function showMessage(msg: string, isError = false) {
  message.textContent = msg;
  message.className = `message ${isError ? 'error' : 'success'}`;
}

function clearMessage() {
  message.textContent = '';
  message.className = 'message';
}

// ── Backend state-change events ──────────────────────────────
// The Rust monitor emits "vpn-state" when the VPN transitions (e.g. the
// sing-box process exits unexpectedly). Show any message and refresh status.
listen<{ state: string; message?: string }>('vpn-state', (event) => {
  const payload = event.payload;
  if (payload.message) {
    showMessage(payload.message, true);
  }
  updateStatus();
}).catch((e) => console.error('failed to listen vpn-state', e));

// ── Views ────────────────────────────────────────────────────
function showView(view: 'main' | 'log') {
  mainView.style.display = view === 'main' ? 'block' : 'none';
  logView.style.display = view === 'log' ? 'block' : 'none';
  if (view === 'log') refreshLog();
}

// ── Sparkline rendering ──────────────────────────────────────
const SPARKLINE_POINTS = 30;
const upHistory: number[] = [];
const downHistory: number[] = [];

// Initialize with zeros
for (let i = 0; i < SPARKLINE_POINTS; i++) {
  upHistory.push(0);
  downHistory.push(0);
}

function drawSparkline(canvas: HTMLCanvasElement, data: number[], color: string) {
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  
  const width = canvas.width;
  const height = canvas.height;
  const max = Math.max(...data, 1);
  
  ctx.clearRect(0, 0, width, height);
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  
  data.forEach((value, i) => {
    const x = (i / (SPARKLINE_POINTS - 1)) * width;
    const y = height - (value / max) * height;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  
  ctx.stroke();
}

function updateSparklines(upSpeed: number, downSpeed: number) {
  upHistory.shift();
  upHistory.push(upSpeed);
  downHistory.shift();
  downHistory.push(downSpeed);
  
  drawSparkline(sparklineUp, upHistory, '#4ade80');
  drawSparkline(sparklineDown, downHistory, '#60a5fa');
}
let pingTimer: ReturnType<typeof setInterval> | null = null;
let lossTimer: ReturnType<typeof setInterval> | null = null;

// Packet-loss tracking: ring buffer of last 30 samples (~past minute at 2s)
const LOSS_SAMPLES = 30;
const LOSS_TIMEOUT_MS = 1000; // latency above this counts as loss (gaming: 1s is an eternity)
const lossRing: boolean[] = []; // true = lost

// Ping history for the chart in the expandable "Ping" panel.
// Time-based 20s window (exact regardless of poll rate). Each entry: { ts, ms }.
// ms = -1 means lost / no response.
const PING_WINDOW_MS = 20000;
const pingHistory: { ts: number; ms: number }[] = [];

function drawPingChart(hoverX?: number) {
  const ctx = pingHistCanvas.getContext('2d');
  if (!ctx) return;
  const W = pingHistCanvas.width, H = pingHistCanvas.height, pad = 4;
  // Window spans [now - WINDOW, now] so the newest sample sits at the right edge.
  const now = Date.now();
  const t0 = now - PING_WINDOW_MS;
  ctx.clearRect(0, 0, W, H);

  // Baseline gridlines
  let max = 0, min = 0;
  for (const s of pingHistory) { if (s.ms > 0) { if (s.ms > max) max = s.ms; if (min === 0 || s.ms < min) min = s.ms; } }
  if (max === 0) max = 300; // nothing yet — show full 0..300 scale
  if (min > max) min = 0;
  const range = (max - min) || 1;
  const yOf = (v: number) => H - pad - ((Math.min(v, max) - min) / range) * (H - 2 * pad);
  const xOf = (ts: number) => ((ts - t0) / PING_WINDOW_MS) * W;

  ctx.strokeStyle = 'rgba(255,255,255,0.08)';
  ctx.lineWidth = 1;
  for (const g of [100, 200, 300]) {
    if (g >= min && g <= max) {
      const y = yOf(g);
      ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(W, y); ctx.stroke();
    }
  }

  const pts = pingHistory.filter(s => s.ts >= t0);
  if (pts.length >= 2) {
    ctx.lineWidth = 1.6;
    for (let i = 1; i < pts.length; i++) {
      const v0 = pts[i - 1], v1 = pts[i];
      const x0 = xOf(v0.ts), x1 = xOf(v1.ts);
      const y0 = v0.ms > 0 ? yOf(v0.ms) : H - pad;
      const y1 = v1.ms > 0 ? yOf(v1.ms) : H - pad;
      const v = v1.ms > 0 ? v1.ms : v0.ms;
      ctx.strokeStyle = v > 300 ? '#ff3366' : v > 200 ? '#ff6b35' : v > 145 ? '#ffcc00' : '#00ff88';
      ctx.beginPath(); ctx.moveTo(x0, y0); ctx.lineTo(x1, y1); ctx.stroke();
    }
    // current dot
    const last = pts[pts.length - 1];
    if (last.ms > 0) {
      const lx = xOf(last.ts), ly = yOf(last.ms);
      ctx.fillStyle = '#00ff88';
      ctx.beginPath(); ctx.arc(lx, ly, 2.5, 0, Math.PI * 2); ctx.fill();
    }
  }

  // Hover marker + nearest-sample highlight
  if (hoverX != null && pts.length) {
    let best = pts[0], bestD = Infinity;
    for (const s of pts) { const d = Math.abs(xOf(s.ts) - hoverX); if (d < bestD) { bestD = d; best = s; } }
    const hx = xOf(best.ts), hy = best.ms > 0 ? yOf(best.ms) : H - pad;
    ctx.strokeStyle = 'rgba(255,255,255,0.25)';
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(hx, 0); ctx.lineTo(hx, H); ctx.stroke();
    if (best.ms > 0) {
      ctx.fillStyle = '#fff';
      ctx.beginPath(); ctx.arc(hx, hy, 3, 0, Math.PI * 2); ctx.fill();
    }
  }
}

function updatePingStats() {
  const now = Date.now();
  const valid = pingHistory.filter(s => s.ms > 0 && now - s.ts <= PING_WINDOW_MS);
  if (!valid.length) {
    pingStatAvg.textContent = '–ms'; pingStatJit.textContent = '±–ms';
    pingStatMin.textContent = '–ms'; pingStatMax.textContent = '–ms';
    return;
  }
  const sum = valid.reduce((a, b) => a + b.ms, 0);
  const avg = Math.round(sum / valid.length);
  const mn = Math.min(...valid.map(s => s.ms)), mx = Math.max(...valid.map(s => s.ms));
  // jitter ~= mean absolute deviation from the average (simple, stable)
  const jit = Math.round(valid.reduce((a, b) => a + Math.abs(b.ms - avg), 0) / valid.length);
  pingStatAvg.textContent = `${avg}ms`;
  pingStatJit.textContent = `±${jit}ms`;
  pingStatMin.textContent = `${mn}ms`;
  pingStatMax.textContent = `${mx}ms`;
}

async function sampleLoss() {
  let lost = false;
  let ms: number | null = null;
  try {
    const result = await invoke<string>('real_ping');
    ms = parseInt(result.replace('ms', ''));
    if (isNaN(ms) || ms > LOSS_TIMEOUT_MS) lost = true;
  } catch {
    lost = true;
  }
  lossRing.push(lost);
  if (lossRing.length > LOSS_SAMPLES) lossRing.shift();
  const lostCount = lossRing.filter(Boolean).length;
  const pct = Math.round((lostCount / lossRing.length) * 100);
  lossValue.textContent = `${pct}%`;

  // Push a time-stamped sample; prune anything older than the visible window.
  pingHistory.push({ ts: Date.now(), ms: ms ?? -1 });
  const cutoff = Date.now() - PING_WINDOW_MS - 1000;
  while (pingHistory.length && pingHistory[0].ts < cutoff) pingHistory.shift();
  if (logSection.classList.contains('expanded')) { drawPingChart(); updatePingStats(); }
}

// ── Chart hover tooltip ────────────────────────────────────
// Overlaid div; shows the ping value + age at the cursor's x position.
const pingTip = document.createElement('div');
pingTip.id = 'ping-tip';
pingTip.style.cssText =
  'position:absolute;pointer-events:none;display:none;z-index:20;' +
  'background:rgba(0,0,0,0.85);border:1px solid var(--border);border-radius:4px;' +
  'padding:3px 6px;font:11px/1.3 monospace;color:#fff;white-space:nowrap;transform:translate(-50%,-120%)';
pingChartWrap.style.position = 'relative';
pingChartWrap.appendChild(pingTip);

pingHistCanvas.addEventListener('mousemove', (e: MouseEvent) => {
  const rect = pingHistCanvas.getBoundingClientRect();
  const x = ((e.clientX - rect.left) / rect.width) * pingHistCanvas.width;
  drawPingChart(x);
  // nearest sample by x
  const now = Date.now();
  const t0 = now - PING_WINDOW_MS;
  const pts = pingHistory.filter(s => s.ts >= t0);
  if (pts.length) {
    let best = pts[0], bestD = Infinity;
    const xOf = (ts: number) => ((ts - t0) / PING_WINDOW_MS) * pingHistCanvas.width;
    for (const s of pts) { const d = Math.abs(xOf(s.ts) - x); if (d < bestD) { bestD = d; best = s; } }
    const ageSec = Math.max(0, Math.round((now - best.ts) / 1000));
    pingTip.textContent = best.ms > 0 ? `${best.ms}ms · ${ageSec}s ago` : `lost · ${ageSec}s ago`;
    pingTip.style.left = `${(best.ts - t0) / PING_WINDOW_MS * 100}%`;
    pingTip.style.top = '0px';
    pingTip.style.display = 'block';
  }
});
pingHistCanvas.addEventListener('mouseleave', () => {
  pingTip.style.display = 'none';
  if (logSection.classList.contains('expanded')) drawPingChart();
});

async function doPing() {
  // Overlap guard: if the previous ping is still in flight (e.g. a stalled
  // backend call), skip this tick instead of stacking calls on the channel.
  if (pingInFlight) return;
  pingInFlight = true;
  try {
    const result = await invoke<string>('real_ping');
    pingValue.textContent = result;

    // Mark that we have a ping response
    hasPingResponse = true;
    lastGoodPingTs = Date.now(); // watchdog: record last good ping

    // Update ping bars based on latency
    const pingMs = parseInt(result.replace('ms', ''));
    const bars = document.querySelectorAll('.ping-bar');

    bars.forEach(bar => {
      const threshold = parseInt((bar as HTMLElement).dataset.threshold || '0');
      if (pingMs >= threshold) {
        bar.classList.add('active');
      } else {
        bar.classList.remove('active');
      }
    });

    // Update triangle for 300+ ms
    const triangle = document.getElementById('ping-triangle');
    if (triangle) {
      triangle.classList.toggle('active', pingMs >= HIGH_PING_MS);
    }
  } catch {
    pingValue.textContent = '-';
    hasPingResponse = false;
    // Clear all bars on error
    document.querySelectorAll('.ping-bar').forEach(bar => bar.classList.remove('active'));
    const triangle = document.getElementById('ping-triangle');
    if (triangle) triangle.classList.remove('active');
  } finally {
    pingInFlight = false;
  }
}

// ── Auto-disconnect watchdog ─────────────────────────────────
async function watchdogCheck() {
  if (lastGoodPingTs === 0 || watchdogTripped) return; // not yet connected, or already tripped
  let running = false;
  try { running = await invoke<boolean>('get_status'); } catch { return; }
  if (!running) { lastGoodPingTs = 0; return; } // user disconnected / already down
  if (Date.now() - lastGoodPingTs > NO_PING_DISCONNECT_MS) {
    watchdogTripped = true;
    try { await invoke('stop_proxy'); } catch { /* ignore */ }
    showMessage('VPN auto-disconnected: no ping for 10s — you are no longer protected.', true);
  }
}

function startPingLoop() {
  stopPingLoop();
  watchdogTripped = false;
  lastGoodPingTs = 0;
  doPing();
  sampleLoss();
  pingTimer = setInterval(doPing, PING_INTERVAL_MS);
  lossTimer = setInterval(sampleLoss, 2000);
  watchTimer = setInterval(watchdogCheck, 1000);
}

function stopPingLoop() {
  if (pingTimer) clearInterval(pingTimer);
  if (lossTimer) clearInterval(lossTimer);
  if (watchTimer) clearInterval(watchTimer);
  pingTimer = null;
  lossTimer = null;
  watchTimer = null;
}

// ── Status update (every 2s) ─────────────────────────────────
let lastPid: number | null = null;
let uptimeStartSecs: number | null = null;
let uptimeTimer: ReturnType<typeof setInterval> | null = null;

function startUptimeTimer() {
  stopUptimeTimer();
  uptimeTimer = setInterval(() => {
    if (uptimeStartSecs !== null) {
      const elapsed = uptimeStartSecs + Math.floor((Date.now() - uptimeRefresh) / 1000);
      btnStartText.textContent = formatUptime(elapsed);
    }
  }, UPTIME_INTERVAL_MS);
}

function stopUptimeTimer() {
  if (uptimeTimer) clearInterval(uptimeTimer);
  uptimeTimer = null;
  uptimeStartSecs = null;
  btnStartText.textContent = 'Start';
}

let uptimeRefresh = Date.now();

// ── State: connection status tracking ───────────────────────
let isConnecting = false;
let hasPingResponse = false;

// Auto-disconnect watchdog: armed only after we've seen at least one good ping.
// If the link then goes silent (no successful ping) for this long, tear the
// tunnel down so the UI never hangs waiting on a dead/stalled connection.
const NO_PING_DISCONNECT_MS = 10000;
let lastGoodPingTs = 0;   // timestamp of last successful ping (0 = not yet connected)
let watchdogTripped = false;
let watchTimer: ReturnType<typeof setInterval> | null = null;
let pingInFlight = false; // overlap guard: skip a tick if the previous one is still running

async function updateStatus() {
  try {
    const s = await invoke<FullStatus>('get_full_status');
    uptimeRefresh = Date.now();

    // Handle connecting state
    if (s.running && !hasPingResponse) {
      statusText.textContent = 'Connecting...';
      statusDot.classList.remove('connected');
      statusDot.style.background = 'var(--warning)';
    } else if (s.running && hasPingResponse) {
      statusText.textContent = 'Connected';
      statusDot.classList.add('connected');
      statusDot.style.background = '';
    } else {
      statusText.textContent = 'Disconnected';
      statusDot.classList.remove('connected');
      statusDot.style.background = '';
      hasPingResponse = false;
      lastGoodPingTs = 0; // watchdog disarms when not running
    }
    
    statusAddress.textContent = s.running && s.server ? s.server : '';
    if (s.running && s.server) {
      statusAddress.innerHTML = getServerFlag(s.server) + s.server;
    } else {
      statusAddress.textContent = '';
    }

    // TCP/UDP indicator
    const protocolIndicator = document.getElementById('protocol-indicator');
    if (s.running && protocolIndicator) {
      const isH2 = s.mode === 'hysteria2';
      protocolIndicator.textContent = isH2 ? 'UDP' : 'TCP';
      protocolIndicator.classList.toggle('protocol-udp', isH2);
      protocolIndicator.classList.toggle('protocol-tcp', !isH2);
      protocolIndicator.style.display = 'inline-block';
    } else if (protocolIndicator) {
      protocolIndicator.style.display = 'none';
      protocolIndicator.classList.remove('protocol-udp', 'protocol-tcp');
    }

    if (!s.running) pingValue.textContent = '-';

    uptimeStartSecs = s.uptime_secs;
    btnStartText.textContent = s.running ? formatUptime(s.uptime_secs) : 'Start';

    // Toggle connected class on start button
    btnStart.classList.toggle('connected', s.running);

    trafficUpValue.textContent = s.running ? formatSpeed(s.traffic_up) : '0 B/s';
    trafficDownValue.textContent = s.running ? formatSpeed(s.traffic_down) : '0 B/s';
    trafficUpTotal.textContent = `${formatBytes(s.total_up)}`;
    trafficDownTotal.textContent = `${formatBytes(s.total_down)}`;
    
    // Update sparklines
    if (s.running) {
      updateSparklines(s.traffic_up, s.traffic_down);
    }

    btnStart.disabled = s.running;
    btnStop.disabled = !s.running;

    if (s.running && s.pid !== lastPid) {
      startPingLoop();
      startUptimeTimer();
    } else if (!s.running) {
      stopPingLoop();
      stopUptimeTimer();
    }
    lastPid = s.pid ?? null;

    if (s.running) clearMessage();
  } catch { /* silent */ }
}

// ── Log ──────────────────────────────────────────────────────
async function renderLog(el: HTMLElement) {
  try {
    const s = await invoke<FullStatus>('get_full_status');
    el.textContent = s.log_lines.join('\n') || 'No log available';
    el.scrollTop = el.scrollHeight;
  } catch {
    el.textContent = 'Failed to load log.';
  }
}

const refreshLog = () => renderLog(logContent);
const refreshInlineLog = () => renderLog(inlineLogContent);

// ── Inline log toggle ────────────────────────────────────────
logToggle.addEventListener('click', () => {
  const isExpanded = logSection.classList.toggle('expanded');
  if (isExpanded) refreshInlineLog();
});

// ── Profile management (Phase 2: Server + Protocol) ─────────
// State: current server and protocol
let currentServer = 'netherlands-1';
let currentProtocol: 'h2' | 'stls' = 'h2';

function getProfileName(): string {
  return `${currentServer}-${currentProtocol}`;
}

function parseProfile(profile: string): { server: string; protocol: 'h2' | 'stls' } {
  // Parse "netherlands-1-h2" -> { server: "netherlands-1", protocol: "h2" }
  const parts = profile.split('-');
  const protocol = parts[parts.length - 1] as 'h2' | 'stls';
  const server = parts.slice(0, -1).join('-');
  return { server, protocol };
}

async function loadProfile() {
  try {
    const profile = await invoke<string>('get_profile');
    const parsed = parseProfile(profile);
    currentServer = parsed.server;
    currentProtocol = parsed.protocol;
    
    // Update UI
    updateServerSelectorUI(currentServer);
    updateProtocolTabs(currentProtocol);
    updateH2PresetVisibility(currentProtocol);
    
    if (currentProtocol === 'h2') loadH2PresetSelection();
  } catch (e) {
    console.error('Failed to load profile:', e);
  }
}

function updateServerSelectorUI(server: string) {
  const flagMap: Record<string, string> = {
    'netherlands-1': 'nl',
    'germany-3': 'de',
    'finland-1': 'fi'
  };
  const displayMap: Record<string, string> = {
    'netherlands-1': 'Netherlands 1',
    'germany-3': 'Germany 3',
    'finland-1': 'Finland 1'
  };
  
  const flag = flagMap[server] || 'de';
  const display = displayMap[server] || 'Germany 1';
  
  serverSelectorFlag.innerHTML = `<img src="https://flagcdn.com/16x12/${flag}.png" alt="${flag.toUpperCase()}" />`;
  serverSelectorText.textContent = display;
  
  // Update active option
  serverSelectorOptions.querySelectorAll<HTMLElement>('.custom-select-option').forEach(opt => {
    opt.classList.toggle('active', opt.dataset.value === server);
  });
}

// ── Server selector handler ──────────────────────────────────
serverSelectorTrigger.addEventListener('click', () => {
  serverSelectorWrapper.classList.toggle('open');
});

// Close on click outside
document.addEventListener('click', (e) => {
  if (!serverSelectorWrapper.contains(e.target as Node)) {
    serverSelectorWrapper.classList.remove('open');
  }
});

serverSelectorOptions.querySelectorAll<HTMLElement>('.custom-select-option').forEach(opt => {
  opt.addEventListener('click', async () => {
    currentServer = opt.dataset.value || 'netherlands-1';
    serverSelectorWrapper.classList.remove('open');
    updateServerSelectorUI(currentServer);
    
    try {
      await invoke('set_profile', { profile: getProfileName() });
      await updateStatus();
      showMessage('Server changed', false);
    } catch (e) {
      showMessage(`Failed: ${e}`, true);
    }
  });
});

function updateProtocolTabs(protocol: 'h2' | 'stls') {
      protocolTabs.forEach(tab => {
        const tabProtocol = (tab as HTMLElement).dataset.protocol;
        tab.classList.toggle('active', tabProtocol === protocol);
      });
  
      // Update status card border color
      statusCard.classList.remove('protocol-h2', 'protocol-stls');
      statusCard.classList.add(`protocol-${protocol}`);
    }

function updateH2PresetVisibility(protocol: 'h2' | 'stls') {
  const h2Sel = document.getElementById('h2-preset-selector');
  if (h2Sel) h2Sel.style.display = protocol === 'h2' ? 'block' : 'none';
  
  // Split preset selector always visible
  const splitSel = document.getElementById('split-preset-selector');
  if (splitSel) splitSel.style.display = 'block';
}

async function loadH2PresetSelection() {
  try {
    const s = await invoke<{ up_mbps: number; down_mbps: number }>('get_h2_speeds');
    const cards = document.querySelectorAll('.h2-preset-card');
    if (!cards.length) return;
    const { up_mbps, down_mbps } = s;
    
    // Remove active class from all cards
    cards.forEach(card => card.classList.remove('active'));
    
    // Set active card based on speeds (default '5g')
    let activePreset = '5g';
    for (const [name, { up, down }] of Object.entries(H2_PRESETS)) {
      if (up_mbps === up && down_mbps === down) { activePreset = name; break; }
    }
    
    const activeCard = document.querySelector(`.h2-preset-card[data-preset="${activePreset}"]`);
    if (activeCard) activeCard.classList.add('active');
  } catch (e) { /* silent */ }
}

// ── Settings panel ───────────────────────────────────────────
async function loadSettings() {
  try {
    const cfg = await invoke<Config>('get_config');
    settingMtu.value = cfg.mtu ? String(cfg.mtu) : '';
    settingTunStack.value = cfg.tun_stack || 'system';

    // Load split mode
    const splitSettings = await invoke<{ split_mode: string; wow_apps?: string[] }>('get_split_settings');
    const mode = splitSettings.split_mode || 'full';

    // Set WoW app checkboxes (default all checked)
    const apps = splitSettings.wow_apps || ['discord', 'chrome', 'telegram'];
    wowAppDiscord.checked = apps.includes('discord');
    wowAppChrome.checked = apps.includes('chrome');
    wowAppTelegram.checked = apps.includes('telegram');

    // Update split preset UI
    updateSplitPresetUI(mode);
  } catch (e) {
    console.error('Failed to load settings:', e);
  }
}

function updateSplitPresetUI(preset: string) {
  splitPresetCards.forEach(card => {
    card.classList.toggle('active', (card as HTMLElement).dataset.preset === preset);
  });

  wowInfoContainer.style.display = preset === 'wow' ? 'block' : 'none';
}

// Gather currently-checked WoW app ids
function getWowApps(): string[] {
  const apps: string[] = [];
  if (wowAppDiscord.checked) apps.push('discord');
  if (wowAppChrome.checked) apps.push('chrome');
  if (wowAppTelegram.checked) apps.push('telegram');
  return apps;
}

// Split preset card handlers
splitPresetCards.forEach(card => {
  card.addEventListener('click', async () => {
    const preset = (card as HTMLElement).dataset.preset || 'full';
    updateSplitPresetUI(preset);

    try {
      const running = await invoke('get_status');
      await invoke('update_settings', {
        mtu: settingMtu.value ? parseInt(settingMtu.value, 10) : null,
        splitMode: preset,
        tunStack: settingTunStack.value,
        wowApps: preset === 'wow' ? getWowApps() : null,
        reconnect: running
      });
      showMessage('Settings saved', false);
      if (running) {
        // Reconnect happens inside update_settings and may fail — it now returns
        // an error on failure (no silent dead tunnel). If we reach here it succeeded.
        showMessage('Reconnected with new split settings', false);
      }
    } catch (e) {
      // e may be "Reconnect failed: <reason>" from the backend.
      showMessage(`${e}`, true);
    }
  });
});

// ── Settings panel toggle ─────────────
btnSettingsToggle.addEventListener('click', async () => {
  const visible = settingsPanel.style.display !== 'none';
  settingsPanel.style.display = visible ? 'none' : 'block';

  const appWindow = getCurrentWindow();
  if (visible) {
    await appWindow.setSize(new LogicalSize(WINDOW_WIDTH, WINDOW_HEIGHT_OPEN));
  } else {
    await appWindow.setSize(new LogicalSize(WINDOW_WIDTH, WINDOW_HEIGHT_CLOSED));
    loadSettings();
  }
});

btnSaveSettings.addEventListener('click', async () => {
  try {
    const mtu = settingMtu.value ? parseInt(settingMtu.value, 10) : null;
    // Get current split mode from active preset card
    const activeCard = document.querySelector('.split-preset-card.active') as HTMLElement;
    const splitMode = activeCard ? activeCard.dataset.preset || 'full' : 'full';

    const running = await invoke('get_status');
    await invoke('update_settings', { mtu, splitMode, tunStack: settingTunStack.value, wowApps: splitMode === 'wow' ? getWowApps() : null, reconnect: running });
    showMessage('Settings saved', false);
    if (running) showMessage('Reconnected with new settings', false);
  } catch (e) {
    showMessage(`${e}`, true);
  }
});

// ── DoH DNS toggle ───────────────────────────────────────────
let dohEnabled = false;

async function updateDohButton() {
  try {
    dohEnabled = await invoke<boolean>('doh_active');
  } catch {
    dohEnabled = false;
  }
  btnDoh.classList.toggle('active', dohEnabled);
  btnDoh.title = dohEnabled
    ? 'DoH DNS is active — click to restore DHCP DNS'
    : 'Set DNS to private DoH servers';
}

btnDoh.addEventListener('click', async () => {
  try {
    if (dohEnabled) {
      await invoke('doh_clear');
      showMessage('DNS restored to DHCP', false);
    } else {
      await invoke('doh_set');
      showMessage('DoH DNS set', false);
    }
    await updateDohButton();
  } catch (e) {
    showMessage(`DoH DNS failed: ${e}`, true);
  }
});

// ── Events ───────────────────────────────────────────────────
listen('proxy-log', (event: { payload: string }) => {
  // Separate full log view only (inline panel now shows the ping chart).
  if (logView.style.display !== 'none') {
    logContent.textContent += `\n${event.payload}`;
    logContent.scrollTop = logContent.scrollHeight;
  }
});

// ── Button handlers ──────────────────────────────────────────
btnStart.addEventListener('click', async () => {
  clearMessage();
  showMessage('Starting...', false);
  try {
    await invoke('start_proxy');
    showMessage('Started');
    startPingLoop();
    lastPid = null;
  } catch (e: any) {
    // e is already a classified, non-technical message from the backend.
    showMessage(String(e), true);
  }
  // Reflect authoritative state (handles immediate failure -> Stopped).
  updateStatus();
});

btnStop.addEventListener('click', async () => {
  clearMessage();
  showMessage('Stopping...', false);
  try {
    await invoke('stop_proxy');
    showMessage('Stopped');
    stopPingLoop();
    lastPid = null;
    pingValue.textContent = '-';
  } catch (e: any) {
    showMessage(String(e), true);
  }
  updateStatus();
});

// Header "Ping" button now toggles the expandable ping-chart panel (was the inline log).
btnLog.addEventListener('click', () => {
  const isExpanded = logSection.classList.toggle('expanded');
  if (isExpanded) { drawPingChart(); updatePingStats(); }
});
btnBackFromLog.addEventListener('click', () => showView('main'));
btnRefreshLog.addEventListener('click', refreshLog);

// ── Protocol tabs handler ────────────────────────────────────
protocolTabs.forEach(tab => {
  tab.addEventListener('click', async () => {
    const protocol = (tab as HTMLElement).dataset.protocol as 'h2' | 'stls';
    if (protocol === currentProtocol) return;
    
    currentProtocol = protocol;
    updateProtocolTabs(protocol);
    updateH2PresetVisibility(protocol);
    
    try {
      await invoke('set_profile', { profile: getProfileName() });
      if (protocol === 'h2') loadH2PresetSelection();
      await updateStatus();
      showMessage('Protocol changed', false);
    } catch (e) {
      showMessage(`Failed: ${e}`, true);
    }
  });
});

// ── H2 Preset Cards ──────────────────────────────────────────
document.querySelectorAll('.h2-preset-card').forEach(card => {
  card.addEventListener('click', async (e) => {
    const target = e.currentTarget as HTMLElement;
    const preset = target.dataset.preset;
    if (!preset) return;
    
    // Update active state
    document.querySelectorAll('.h2-preset-card').forEach(c => c.classList.remove('active'));
    target.classList.add('active');
    
    try {
      await invoke('apply_h2_preset', { name: preset });
      showMessage('Preset applied', false);
    } catch (e) {
      showMessage(`Failed: ${e}`, true);
    }
  });
});

// ── Init ─────────────────────────────────────────────────────
(async () => {
  await loadProfile();
  await updateStatus();

  // Highlight the active split preset card on launch (full tunnel by default)
  try {
    const splitSettings = await invoke<{ split_mode: string; wow_apps?: string[] }>('get_split_settings');
    updateSplitPresetUI(splitSettings.split_mode || 'full');
    const apps = splitSettings.wow_apps || ['discord', 'chrome', 'telegram'];
    wowAppDiscord.checked = apps.includes('discord');
    wowAppChrome.checked = apps.includes('chrome');
    wowAppTelegram.checked = apps.includes('telegram');
  } catch {
    updateSplitPresetUI('full');
  }

  await updateDohButton();
})();

setInterval(updateStatus, STATUS_INTERVAL_MS);
