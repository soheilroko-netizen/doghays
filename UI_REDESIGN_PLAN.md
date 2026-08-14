# UI Redesign Plan: shadow-tls-ss2022 (h2 branch)

**Project:** dakal-tls v5 (ShadowTLS + Hysteria2 VPN)  
**Current State:** Tauri 2 + TypeScript + vanilla CSS  
**Target:** Modern, clean, professional VPN client UI

---

## Current Architecture Analysis

### Tech Stack
- **Frontend:** Vanilla TypeScript + HTML + CSS (no framework)
- **Backend:** Tauri 2 (Rust) + sing-box VPN client
- **Window:** 450×520px main view, collapsible panels
- **State:** Hardcoded 4 profiles (2 servers × 2 protocols)

### Current UI Structure
```
Main View:
├── Header (title + subtitle)
├── Profile Bar (dropdown + Settings + Log buttons)
├── Settings Panel (collapsible)
│   ├── Split Tunneling mode
│   ├── Custom rules textarea
│   ├── MTU input
│   └── Save + Update Geofiles buttons
├── H2 Preset Selector (visible when H2 mode)
├── Status Card (dot + text + address)
├── Server Info (4 rows: Ping, Uptime, Current, Total)
└── Controls (Start/Stop buttons)

Log View:
├── Header
├── Controls (Refresh + Back)
└── Pre-formatted log content
```

### Current Features
- 4 hardcoded profiles (Germany/Finland × ShadowTLS/Hysteria2)
- H2 presets (ADSL/4G/5G/Max bandwidth)
- Split tunneling (full/iran/custom)
- Live traffic stats via Clash API (2s polling)
- Real ping (5s interval when connected)
- System tray with connect/disconnect
- Single instance enforcement
- Admin check on Windows
- Dark theme (navy/cyan accent)

---

## Problems to Solve

### 1. **Layout Issues**
- Settings panel collapse creates jarring height jumps
- H2 preset selector appears/disappears based on profile
- 450px width feels cramped for settings textarea
- Log view is separate page (breaks flow)
- No visual hierarchy between critical (status) and secondary (settings)

### 2. **Information Density**
- 4 metrics in server-info card feel redundant when disconnected
- Profile dropdown mixes server + protocol in single select
- Split tunneling + MTU + geofiles in one collapsible panel (unrelated concerns)

### 3. **Interaction Flow**
- Settings require manual "Save" (no auto-apply)
- Profile switch doesn't warn if connected (will disconnect)
- No feedback for geofiles update progress
- Start button doesn't indicate what will connect (relies on dropdown)

### 4. **Visual Design**
- Flat boxes (no depth/elevation)
- Status dot is small (14px)
- Monospace font for values feels technical but inconsistent
- No icon system (text-only buttons)
- Color coding underused (only red/green for status)

---

## Redesign Goals

### Visual
1. **Modern depth:** Subtle shadows, layered cards, glassmorphism hints
2. **Icon language:** Replace text buttons with icons (⚙️ settings, 📋 log, etc.)
3. **Larger status indicator:** Make connection state unmissable
4. **Color-coded modes:** H2 = blue accent, STLS = purple accent
5. **Smooth transitions:** Animate panel expansions, status changes

### Layout
1. **Fixed-height main view:** No layout shifts when toggling panels
2. **Inline log:** Collapsible log panel in main view (no separate page)
3. **Two-column settings:** Use horizontal space better
4. **Server/protocol decoupled:** Separate selectors or tabs

### Interaction
1. **Auto-save settings:** Debounced write on change
2. **Connection-aware profile switch:** Confirm or auto-reconnect
3. **Progressive disclosure:** Hide advanced settings behind "Advanced" toggle
4. **One-click profiles:** Big cards for common profiles (instead of dropdown)

---

## Proposed New Layout

### Option A: Card-Based (Recommended)

```
┌─────────────────────────────────────────────┐
│ dakal-tls                      [⚙️] [📋] [✕] │ ← Window chrome
├─────────────────────────────────────────────┤
│ ╔═══════════════════════════════════════╗   │
│ ║  ●  Connected                         ║   │ ← Large status card
│ ║     ns.baft.uk • Hysteria2            ║   │   (60px height)
│ ╚═══════════════════════════════════════╝   │
│                                             │
│ Quick Profiles:                             │
│ ┌──────────┐ ┌──────────┐                   │
│ │ Germany  │ │ Finland  │                   │ ← Profile cards
│ │ Hysteria2│ │ Hysteria2│                   │   (80×80 each)
│ │   12ms   │ │   45ms   │                   │
│ └──────────┘ └──────────┘                   │
│ ┌──────────┐ ┌──────────┐                   │
│ │ Germany  │ │ Finland  │                   │
│ │ShadowTLS │ │ShadowTLS │                   │
│ │   14ms   │ │   48ms   │                   │
│ └──────────┘ └──────────┘                   │
│                                             │
│ Stats:                                      │
│ ┌─────────────┬──────────────┬────────────┐ │
│ │ ↑ 1.2 MB/s  │ ↓ 3.4 MB/s   │ ⏱ 2h 14m   │ │ ← Inline stats
│ └─────────────┴──────────────┴────────────┘ │
│                                             │
│ ┌─────────────────────────────────────────┐ │
│ │         [Disconnect]                    │ │ ← Action button
│ └─────────────────────────────────────────┘ │
│                                             │
│ ▼ Advanced Settings                         │ ← Collapsible
│ ┌─────────────────────────────────────────┐ │
│ │ Split Tunneling: [Full Tunnel ▾]       │ │
│ │ MTU: [Auto]                             │ │
│ └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

**Advantages:**
- Profile selection is visual (card grid)
- Ping values shown directly on cards
- Larger touch targets (better for accessibility)
- Status card dominates (main info at-a-glance)

**Disadvantages:**
- Needs 480×600px window (60px taller)
- 4-profile limit (won't scale to 10+ profiles)

---

### Option B: Hybrid (Dropdown + Tabs)

```
┌─────────────────────────────────────────────┐
│ ┌─────────────────────────────┐ [⚙️] [📋]   │
│ │ Germany #1          ▾       │             │ ← Server dropdown
│ └─────────────────────────────┘             │
│ ┌───────────────────┬──────────────────────┐│
│ │  Hysteria2       │  ShadowTLS           ││ ← Protocol tabs
│ └───────────────────┴──────────────────────┘│
│                                             │
│ ╔═══════════════════════════════════════╗   │
│ ║ ●  Connected        ns.baft.uk:40001  ║   │ ← Status banner
│ ╚═══════════════════════════════════════╝   │
│                                             │
│ ┌─────────┬─────────┬──────────┬─────────┐ │
│ │  12ms   │ 2h 14m  │ ↑1.2 MB/s│↓3.4 MB/s│ │ ← Metric tiles
│ │  Ping   │ Uptime  │  Upload  │Download │ │
│ └─────────┴─────────┴──────────┴─────────┘ │
│                                             │
│ [H2 Preset: 5G/Fiber (40↑/80↓) ▾]          │ ← Inline H2 control
│                                             │
│ ┌─────────────────────────────────────────┐ │
│ │         [Start Connection]              │ │
│ └─────────────────────────────────────────┘ │
│                                             │
│ ▼ Advanced                                  │
│ ┌─────────────────────────────────────────┐ │
│ │ Split Tunneling: [Full ▾]    MTU: [Auto]│ │
│ └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

**Advantages:**
- Compact (fits 450×540)
- Scales to more servers (dropdown)
- Protocol switch is fast (tabs)
- Familiar VPN client pattern

**Disadvantages:**
- Less visual than cards
- Tabs + dropdown = 2 interaction layers

---

### Option C: Single-Column Minimalist

```
┌────────────────────────────────┐
│  dakal-tls         [⚙️] [📋] [✕]│
├────────────────────────────────┤
│                                │
│       ●  Disconnected          │ ← Centered status
│                                │
│  ┌────────────────────────┐   │
│  │ Germany #1 · Hysteria2 │   │ ← Selected profile chip
│  └────────────────────────┘   │
│                                │
│  ┌──────────────────────────┐ │
│  │      Start VPN           │ │ ← Primary action
│  └──────────────────────────┘ │
│                                │
│  ─────────────────────────────│
│                                │
│  Ping: 12ms     Uptime: 2h 14m│ ← Stats (2-column)
│  ↑ 1.2 MB/s     ↓ 3.4 MB/s    │
│                                │
│  [Change Profile]              │ ← Opens modal/drawer
│                                │
└────────────────────────────────┘
```

**Advantages:**
- Minimal cognitive load
- Fastest to implement (least change)
- Extremely clear primary action

**Disadvantages:**
- Profile selection hidden (extra click)
- Wastes horizontal space

---

## Recommended Approach: **Option B (Hybrid)**

**Rationale:**
- Balances familiarity + efficiency
- Fits existing 450px width (just +20px height)
- Server dropdown + protocol tabs = clear mental model
- Metric tiles are scannable
- Advanced settings collapse keeps power-user features accessible

---

## Implementation Phases

### Phase 1: Layout Refactor (No Visual Changes)
**Goal:** Restructure HTML/CSS for new layout without changing appearance.

1. Split `index.html` into logical sections:
   - `<div class="app-header">` (server + protocol selectors)
   - `<div class="status-banner">` (status card)
   - `<div class="metrics-grid">` (ping/uptime/traffic tiles)
   - `<div class="controls-section">` (H2 preset + start/stop)
   - `<div class="advanced-section">` (collapsible settings)
   - `<div class="log-section">` (collapsible log)

2. Extract CSS into modules:
   - `styles/base.css` (resets, vars)
   - `styles/components.css` (buttons, inputs, tiles)
   - `styles/layout.css` (grid, flex, spacing)

3. Update `main.ts`:
   - Group DOM refs by section
   - Extract update functions (`updateMetrics`, `updateStatus`, etc.)

**Deliverable:** Same UI, cleaner code structure.

---

### Phase 2: Protocol Tabs + Server Dropdown
**Goal:** Replace single profile dropdown with server selector + protocol tabs.

1. Add tab component:
   ```html
   <div class="protocol-tabs">
     <button class="tab active" data-protocol="h2">Hysteria2</button>
     <button class="tab" data-protocol="stls">ShadowTLS</button>
   </div>
   ```

2. Server dropdown (2 options):
   ```html
   <select id="server-selector">
     <option value="germany-1">Germany #1</option>
     <option value="finland-1">Finland #1</option>
   </select>
   ```

3. Update `main.ts`:
   - Combine server + protocol → profile name
   - Sync tabs with active profile (e.g., `germany-1-h2` → server=germany-1, protocol=h2)

4. Add Tauri command (optional):
   ```rust
   #[tauri::command]
   fn get_available_servers() -> Vec<ServerInfo> { ... }
   ```

**Deliverable:** New selection UI, same functionality.

---

### Phase 3: Metric Tiles + Status Banner
**Goal:** Replace flat info rows with tile grid + prominent status banner.

1. Status banner (full-width, colored background):
   ```html
   <div class="status-banner connected">
     <div class="status-dot"></div>
     <span class="status-text">Connected</span>
     <span class="status-server">ns.baft.uk:40001</span>
   </div>
   ```
   CSS: 48px height, green gradient when connected, gray when disconnected.

2. Metric tiles (4-column grid):
   ```html
   <div class="metrics-grid">
     <div class="metric-tile">
       <div class="metric-value" id="ping-value">12ms</div>
       <div class="metric-label">Ping</div>
     </div>
     <!-- repeat for uptime, upload, download -->
   </div>
   ```
   CSS: Equal-width columns, centered text, 64px height.

3. Update polling logic:
   - Keep 2s interval for traffic
   - Add smooth number transitions (CSS `transition: opacity 0.2s`)

**Deliverable:** Modern metric display.

---

### Phase 4: Inline Log Panel
**Goal:** Remove separate log view, add collapsible log in main window.

1. Replace log view navigation with inline panel:
   ```html
   <div class="log-section collapsed">
     <button class="log-toggle">
       <span>Show Logs</span>
       <svg class="chevron">...</svg>
     </button>
     <pre class="log-content"></pre>
   </div>
   ```

2. CSS:
   - Collapsed: 36px height (just toggle button)
   - Expanded: 240px height (fixed, scrollable log)
   - Smooth `max-height` transition

3. Update `main.ts`:
   - Remove `showView('log')` logic
   - Keep live log streaming (existing `listen('proxy-log')`)

**Deliverable:** Log accessible without leaving main view.

---

### Phase 5: Visual Polish
**Goal:** Add depth, icons, animations.

1. **Depth:**
   - Add `box-shadow` to cards:
     ```css
     .status-banner { box-shadow: 0 2px 8px rgba(0,0,0,0.3); }
     .metric-tile { box-shadow: 0 1px 4px rgba(0,0,0,0.2); }
     ```
   - Glassmorphism for settings panel:
     ```css
     .advanced-section { 
       background: rgba(22, 33, 62, 0.6);
       backdrop-filter: blur(12px);
     }
     ```

2. **Icons:**
   - Use SVG icons (inline or sprite sheet):
     - ⚙️ Settings → `<svg>...</svg>`
     - 📋 Logs → `<svg>...</svg>`
     - ✕ Close → `<svg>...</svg>`
   - Replace text buttons with icon buttons (32×32px)

3. **Animations:**
   - Status dot pulse when connected:
     ```css
     @keyframes pulse {
       0%, 100% { box-shadow: 0 0 8px var(--success); }
       50% { box-shadow: 0 0 16px var(--success); }
     }
     .status-dot.connected { animation: pulse 2s infinite; }
     ```
   - Metric value fade on update:
     ```ts
     element.style.opacity = '0.5';
     setTimeout(() => { element.textContent = newValue; element.style.opacity = '1'; }, 100);
     ```

4. **Color coding:**
   - Hysteria2 tab active → blue accent (`#00b4d8`)
   - ShadowTLS tab active → purple accent (`#a855f7`)
   - Update status banner border to match protocol

**Deliverable:** Polished, modern UI.

---

### Phase 6: Advanced Features (Optional)
**Goal:** Add power-user features without cluttering main view.

1. **Profile management:**
   - Add "Manage Profiles" in settings panel
   - Modal for adding custom servers (replace hardcoded profiles)
   - Store in `profiles.json` (read via Tauri command)

2. **Traffic graph:**
   - Replace current traffic text with mini sparkline (last 30s)
   - Use Canvas API or simple SVG path

3. **Connection history:**
   - Log each connect/disconnect event
   - Show "Last connected: 2h ago" in profile cards

4. **Notifications:**
   - Desktop notification on connect/disconnect
   - Tauri API: `notification.requestPermission()` + `new Notification()`

**Deliverable:** Enhanced UX for power users.

---

## Design System

### Colors (Updated)
```css
:root {
  /* Base */
  --bg-primary: #0a0e1a;     /* Darker background */
  --bg-secondary: #16213e;   /* Card background */
  --bg-tertiary: #1a2744;    /* Elevated surface */
  
  /* Text */
  --text-primary: #f0f4f8;
  --text-secondary: #94a3b8;
  --text-tertiary: #64748b;
  
  /* Accents */
  --accent-h2: #00b4d8;      /* Hysteria2 blue */
  --accent-stls: #a855f7;    /* ShadowTLS purple */
  --success: #10b981;
  --error: #ef4444;
  --warning: #f59e0b;
  
  /* UI */
  --border: #334155;
  --shadow-sm: 0 1px 3px rgba(0,0,0,0.3);
  --shadow-md: 0 4px 12px rgba(0,0,0,0.4);
  --shadow-lg: 0 8px 24px rgba(0,0,0,0.5);
}
```

### Typography
```css
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 
               'Roboto', 'Oxygen', 'Ubuntu', sans-serif;
  font-size: 14px;
  line-height: 1.5;
}

.metric-value { font-size: 20px; font-weight: 600; }
.metric-label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.5px; }
.status-text { font-size: 16px; font-weight: 500; }
```

### Spacing Scale
```css
--spacing-xs: 4px;
--spacing-sm: 8px;
--spacing-md: 12px;
--spacing-lg: 16px;
--spacing-xl: 24px;
```

---

## Window Sizing

### Current
- Main: 450×520 (as CSS, not window size)
- Settings: 460×700 (unused in h2 branch)

### Proposed
- Main: **450×560** (add 40px for tabs + better spacing)
- Min: 450×560 (prevent shrinking)
- Max: 450×700 (allow vertical expansion for log panel)

Update `tauri.conf.json`:
```json
{
  "app": {
    "windows": [
      {
        "label": "main",
        "width": 450,
        "height": 560,
        "minWidth": 450,
        "minHeight": 560,
        "maxWidth": 450,
        "maxHeight": 700,
        "resizable": true,
        "center": true
      }
    ]
  }
}
```

---

## Migration Strategy

### Keep Working
- Don't break existing functionality during phases 1-3
- Each phase should be deployable (user can test incrementally)

### Feature Flags (Optional)
If you want to A/B test:
```rust
#[tauri::command]
fn get_ui_version() -> String {
  std::env::var("UI_VERSION").unwrap_or("v1".to_string())
}
```
Then in TypeScript:
```ts
const uiVersion = await invoke('get_ui_version');
if (uiVersion === 'v2') { /* load new layout */ }
```

---

## Risks & Mitigations

### Risk 1: Window height increase breaks existing users
**Mitigation:** Auto-migrate window size on first launch:
```rust
if stored_height < 560 {
  window.set_size(450, 560)?;
}
```

### Risk 2: Tab UI confuses users who expect single dropdown
**Mitigation:** Add tooltip on first launch: "Select server above, then choose protocol"

### Risk 3: Performance regression (too many animations)
**Mitigation:**
- Use CSS `will-change` sparingly
- Test on low-end hardware
- Add `prefers-reduced-motion` media query

---

## Success Metrics

1. **Visual clarity:** Status visible in <1s (user study)
2. **Efficiency:** Profile switch in <2 clicks
3. **Performance:** UI responsive (<16ms frame time)
4. **Accessibility:** Keyboard navigation works
5. **Code health:** <600 LOC in `main.ts` (down from 361)

---

## Next Steps

1. **Approve design:** Choose Option A/B/C (recommend B)
2. **Phase 1:** Refactor layout structure (2-3h)
3. **Phase 2:** Protocol tabs (1-2h)
4. **Phase 3:** Metric tiles (1-2h)
5. **Phase 4:** Inline log (1h)
6. **Phase 5:** Visual polish (2-3h)
7. **QA + deploy**

**Total Estimate:** 8-12 hours (spread over 3-5 sessions)

---

## Open Questions

1. Do you want profile management UI (add/edit servers), or keep hardcoded?
2. Should H2 preset selector stay visible, or move to Advanced?
3. Traffic graph (sparkline) in Phase 6, or skip?
4. Desktop notifications on connect/disconnect?
5. Keep system tray menu as-is, or update to match new protocol/server split?

**Awaiting your input to finalize the plan.**
