import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import './styles.css';

// ── Constants ────────────────────────────────────────────────
const PING_INTERVAL_MS = 5000;
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

interface SplitRule {
  pattern: string;
  process_names?: string[];
  folder_paths?: string[];
}

interface Config {
  server_address: string;
  ss_port: number;
  ss_password: string;
  stls_port: number;
  stls_password: string;
  stls_sni: string;
  socks5_port: number;
  mtu?: number;
  split_mode?: string;
  split_rules?: { pattern: string }[];
  mode: string;
  h2_port: number;
  h2_password: string;
  h2_sni: string;
  h2_insecure: boolean;
  h2_obfs: string;
  h2_obfs_password: string;
  h2_mport: string;
  h2_up_mbps: number;
  h2_down_mbps: number;
  h2_auto: boolean;
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
const inlineLogContent = document.getElementById('inline-log-content')!;

// Settings panel
const settingsPanel = document.getElementById('settings-panel')!;

// Views
const mainView = document.getElementById('main-view')!;
const logView = document.getElementById('log-view')!;
const logContent = document.getElementById('log-content')!;
const btnRefreshLog = document.getElementById('btn-refresh-log')!;
const btnBackFromLog = document.getElementById('btn-back-from-log')!;

// Settings inputs
const customRulesContainer = document.getElementById('custom-rules-container')!;
const wowInfoContainer = document.getElementById('wow-info-container')!;
const settingMtu = document.getElementById('setting-mtu') as HTMLInputElement;
const settingSplitRules = document.getElementById('setting-split-rules') as HTMLTextAreaElement;
const btnSaveSettings = document.getElementById('btn-save-settings')!;
const btnUpdateGeofiles = document.getElementById('btn-update-geofiles')!;
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

async function doPing() {
  try {
    const result = await invoke<string>('real_ping');
    pingValue.textContent = result;
    
    // Mark that we have a ping response
    hasPingResponse = true;
    
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
  }
}

function startPingLoop() {
  stopPingLoop();
  doPing();
  pingTimer = setInterval(doPing, PING_INTERVAL_MS);
}

function stopPingLoop() {
  if (pingTimer) clearInterval(pingTimer);
  pingTimer = null;
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
    trafficUpTotal.textContent = `Total: ${formatBytes(s.total_up)}`;
    trafficDownTotal.textContent = `Total: ${formatBytes(s.total_down)}`;
    
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
    'germany-1': 'de',
    'germany-3': 'de',
    'finland-1': 'fi'
  };
  const displayMap: Record<string, string> = {
    'netherlands-1': 'Netherlands 1',
    'germany-1': 'Germany 1',
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
    
    // Load split settings
    const splitSettings = await invoke<{ split_mode: string; split_rules: SplitRule[] }>('get_split_settings');
    const mode = splitSettings.split_mode || 'full';
    settingSplitRules.value = splitSettings.split_rules?.map(r => r.pattern).join('\n') || '';
    
    // Update split preset UI
    updateSplitPresetUI(mode);
  } catch (e) {
    console.error('Failed to load settings:', e);
  }
}

// Check geofiles cooldown and toggle button
async function checkGeofilesCooldown() {
  try {
    const canDownload = await invoke<boolean>('can_download_geofiles');
    btnUpdateGeofiles.style.display = canDownload ? 'inline-block' : 'none';
  } catch {
    btnUpdateGeofiles.style.display = 'inline-block';
  }
}

function updateSplitPresetUI(preset: string) {
  splitPresetCards.forEach(card => {
    card.classList.toggle('active', (card as HTMLElement).dataset.preset === preset);
  });

  customRulesContainer.style.display = preset === 'custom' ? 'block' : 'none';
  wowInfoContainer.style.display = preset === 'wow' ? 'block' : 'none';

  // Geofiles button hidden for all modes
  btnUpdateGeofiles.style.display = 'none';
}

// Split preset card handlers
splitPresetCards.forEach(card => {
  card.addEventListener('click', async () => {
    const preset = (card as HTMLElement).dataset.preset || 'full';
    updateSplitPresetUI(preset);

    // No custom rules in remaining modes — WoW domains hardcoded in backend
    const splitRules: string[] = [];

    try {
      const running = await invoke('get_status');
      await invoke('update_settings', {
        mtu: settingMtu.value ? parseInt(settingMtu.value, 10) : null,
        splitMode: preset,
        splitRules,
        reconnect: running
      });
      showMessage('Settings saved', false);
      if (running) {
        showMessage('Reconnecting with new split settings...', false);
      }
    } catch (e) {
      showMessage(`Failed: ${e}`, true);
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
    const splitRules: string[] = [];  // No custom rules

    const running = await invoke('get_status');
    await invoke('update_settings', { mtu, splitMode, splitRules, reconnect: running });
    showMessage('Settings saved', false);
    if (running) showMessage('Reconnecting...', false);
  } catch (e) {
    showMessage(`Failed: ${e}`, true);
  }
});

btnUpdateGeofiles.addEventListener('click', async () => {
  try {
    // Check cooldown
    const canDownload = await invoke<boolean>('can_download_geofiles');
    if (!canDownload) {
      showMessage('Geofiles updated recently. Try again in a week.', true);
      return;
    }

    showMessage('Downloading geofiles...', false);
    await invoke('update_geofiles');
    showMessage('Geofiles updated', false);
    // Hide button after successful update
    btnUpdateGeofiles.style.display = 'none';
  } catch (e) {
    showMessage(`Failed: ${e}`, true);
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
  // Update inline log if expanded
  if (logSection.classList.contains('expanded')) {
    inlineLogContent.textContent += `\n${event.payload}`;
    inlineLogContent.scrollTop = inlineLogContent.scrollHeight;
  }
  
  // Update separate log view if visible
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
    showMessage(String(e), true);
  }
});

btnStop.addEventListener('click', async () => {
  clearMessage();
  try {
    await invoke('stop_proxy');
    showMessage('Stopped');
    stopPingLoop();
    lastPid = null;
    pingValue.textContent = '-';
  } catch (e: any) {
    showMessage(String(e), true);
  }
});

btnLog.addEventListener('click', () => showView('log'));
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
    const splitSettings = await invoke<{ split_mode: string }>('get_split_settings');
    updateSplitPresetUI(splitSettings.split_mode || 'full');
  } catch {
    updateSplitPresetUI('full');
  }

  await updateDohButton();
})();

setInterval(updateStatus, STATUS_INTERVAL_MS);
