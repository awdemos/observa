(function () {
  'use strict';

  const MAX_HISTORY = 60;
  const METRIC_HISTORY = [];
  window.OBSERVA_METRIC_HISTORY = METRIC_HISTORY;
  const WIDGET_STORAGE_KEY = 'observa-widget-session';
  const CHAT_HISTORY_KEY = 'observa-chat-history';
  const WIDGET_OPEN_KEY = 'observa-widget-open';
  const PREFS_KEY = 'observa-preferences';
  const DEFAULT_REFRESH_MS = 2000;
  let refreshIntervalId = null;
  let currentRefreshMs = DEFAULT_REFRESH_MS;

  async function init() {
    initPreferences();
    initTheme();
    initChart();
    initMeters();
    loadHistory();
    loadInsights();
    loadStatus();
    await wireChat();
    wireWidget();
    wireMobileNav();
    wireSse();
    wireSecurityPage();
    initThreeViz();
    startRefreshLoop();
    document.body.addEventListener('htmx:afterSwap', (evt) => {
      if (evt.detail.elt?.id === 'metrics-summary') {
        refreshMeters();
      }
      if (evt.detail.elt?.id === 'chat-messages') {
        scrollChat();
      }
      if (evt.detail.elt?.id === 'logs-panel') {
        restoreLogExplanations(evt.detail.elt);
        scrollLogs();
      }
      if (evt.detail.elt?.id === 'security-rows') {
        restoreLogExplanations(evt.detail.elt);
        updateSecurityCount();
      }
      if (window.ObservaThreeViz && document.querySelector('[data-three-viz]')) {
        setTimeout(() => window.ObservaThreeViz.init(), 0);
      }
      if (!window.ObservaThreeViz || !window.ObservaThreeViz.isAvailable()) {
        enableChartFallback();
      }
    });

    document.body.addEventListener('htmx:beforeSwap', (evt) => {
      if (evt.detail.target?.id === 'logs-panel' || evt.detail.target?.id === 'security-rows') {
        saveLogExplanations(evt.detail.target);
      }
    });
  }

  function wireSse() {
    let reconnectDelay = 1000;
    let source = createSource();

    function createSource() {
      const s = new EventSource('/events');
      s.addEventListener('open', () => {
        setSseStatus('live');
        reconnectDelay = 1000;
      });
      s.addEventListener('metric', (evt) => {
        const envelope = JSON.parse(evt.data);
        const data = envelope.Metric || envelope;
        pushHistory(data);
        updateMetersFromSnapshot(data);
        refreshMetricsPanels();
        refreshMetricsGrids(data);
        if (window.ObservaThreeViz) window.ObservaThreeViz.update('metric', data);
      });
      s.addEventListener('log', () => {
        refreshLogsPanel();
        refreshSecurityPanel();
        if (window.ObservaThreeViz) window.ObservaThreeViz.update('log');
      });
      s.addEventListener('chat', (evt) => {
        const envelope = JSON.parse(evt.data);
        appendChatMessage(envelope.Chat || envelope);
      });
      s.addEventListener('heartbeat', (evt) => {
        const envelope = JSON.parse(evt.data);
        const data = envelope.Heartbeat || envelope;
        updateSystemStatus(data);
      });
      s.addEventListener('alert', async (evt) => {
        const envelope = JSON.parse(evt.data);
        const data = envelope.Alert || envelope;
        await showAlertToast(data);
        refreshSecurityPanel();
      });
      s.addEventListener('error', () => {
        setSseStatus('disconnected');
        s.close();
        scheduleReconnect();
      });
      return s;
    }

    function scheduleReconnect() {
      if (document.visibilityState !== 'visible') return;
      setSseStatus('reconnecting');
      setTimeout(() => {
        source = createSource();
      }, reconnectDelay);
      reconnectDelay = Math.min(reconnectDelay * 2, 30000);
    }

    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible' && source.readyState === EventSource.CLOSED) {
        source.close();
        source = createSource();
      }
    });
  }

  function setSseStatus(state) {
    const statusEl = document.getElementById('sse-status');
    if (!statusEl) return;
    statusEl.dataset.state = state;
    statusEl.textContent = state === 'live' ? 'live' : 'reconnecting…';
  }

  function wirePreferencesPanel() {
    const panel = document.getElementById('preferences-panel');
    const toggle = document.getElementById('preferences-toggle');
    if (!panel || !toggle) return;

    toggle.addEventListener('click', () => {
      const isHidden = panel.hidden;
      panel.hidden = !isHidden;
      toggle.setAttribute('aria-expanded', String(isHidden));
    });

    const theme = document.getElementById('pref-theme');
    if (theme) {
      theme.value = loadPreferences().theme || 'dark';
      theme.addEventListener('change', () => {
        applyTheme(theme.value);
        savePreferences({ theme: theme.value });
      });
    }

    const refresh = document.getElementById('pref-refresh');
    if (refresh) {
      refresh.value = String(refreshIntervalMs());
      refresh.addEventListener('change', () => {
        const ms = parseInt(refresh.value, 10);
        savePreferences({ refreshIntervalMs: Number.isFinite(ms) && ms >= 0 ? ms : DEFAULT_REFRESH_MS });
        startRefreshLoop();
      });
    }

    const reduced = document.getElementById('pref-reduced-motion');
    if (reduced) {
      reduced.checked = loadPreferences().reducedMotion || false;
      reduced.addEventListener('change', () => {
        applyReducedMotion(reduced.checked);
        savePreferences({ reducedMotion: reduced.checked });
        applyAutoRotateControls();
        if (window.ObservaThreeViz) window.ObservaThreeViz.init();
      });
    }

    const alertSound = document.getElementById('pref-alert-sound');
    if (alertSound) {
      alertSound.checked = loadPreferences().alertSound === true;
      alertSound.addEventListener('change', () => {
        savePreferences({ alertSound: alertSound.checked });
        if (alertSound.checked) playAlertSound(true);
      });
    }

    const autoRotate = document.getElementById('pref-auto-rotate');
    if (autoRotate) {
      autoRotate.checked = getAutoRotateEnabled();
      autoRotate.addEventListener('change', () => {
        savePreferences({ autoRotate: autoRotate.checked });
        applyAutoRotateControls();
      });
    }

    const autoRotateSpeed = document.getElementById('pref-auto-rotate-speed');
    if (autoRotateSpeed) {
      autoRotateSpeed.value = getAutoRotateSpeed();
      autoRotateSpeed.addEventListener('change', () => {
        savePreferences({ autoRotateSpeed: autoRotateSpeed.value });
        applyAutoRotateControls();
      });
    }

    wireSystemPromptControls();

    document.addEventListener('click', (evt) => {
      if (!panel.hidden && !panel.contains(evt.target) && !toggle.contains(evt.target)) {
        panel.hidden = true;
        toggle.setAttribute('aria-expanded', 'false');
      }
    });
  }

  function refreshMetricsPanels() {
    if (el('metrics-summary')) {
      htmx.ajax('GET', '/partials/metrics-summary', { target: '#metrics-summary', swap: 'outerHTML' });
    }
    if (el('network-panel')) {
      htmx.ajax('GET', '/partials/network', { target: '#network-panel', swap: 'innerHTML' });
    }
    if (el('processes-table')) {
      htmx.ajax('GET', '/partials/metrics', { target: '#processes-table', swap: 'innerHTML' });
    }
  }

  function refreshMetricsGrids(data) {
    if (!data) return;
    const cores = el('cores-grid');
    if (cores) {
      const pctList = data.cpu?.per_core_usage || [];
      cores.innerHTML = pctList
        .map(
          (pct, i) => `
          <div class="core-card">
            <div class="core-label">Core ${i + 1}</div>
            <div class="bar-track"><div class="bar-fill" style="width: ${Math.max(0, Math.min(100, pct))}%"></div></div>
            <div class="core-pct">${pct.toFixed(1)}%</div>
          </div>`,
        )
        .join('');
    }

    const disks = el('disks-grid');
    if (disks) {
      disks.innerHTML = (data.disks || [])
        .map(
          (d) => {
            const pct = d.total_bytes ? (d.used_bytes / d.total_bytes) * 100 : 0;
            return `
          <div class="metric-list-item">
            <div class="metric-list-main">
              <span class="metric-list-name">${escapeHtml(d.name || '')}</span>
              <span class="metric-list-value">${pct.toFixed(0)}%</span>
            </div>
            <div class="bar-track"><div class="bar-fill" style="width: ${Math.max(0, Math.min(100, pct))}%"></div></div>
            <div class="metric-list-detail">${formatBytes(d.used_bytes || 0)} / ${formatBytes(d.total_bytes || 0)}</div>
          </div>`;
          },
        )
        .join('');
    }

    const networks = el('networks-grid');
    if (networks) {
      networks.innerHTML = (data.networks || [])
        .map(
          (n) => `
          <div class="metric-list-item">
            <div class="metric-list-main">
              <span class="metric-list-name">${escapeHtml(n.interface || '')}</span>
              <span class="metric-list-value">RX ${formatBytes(n.rx_rate || 0)}/s · TX ${formatBytes(n.tx_rate || 0)}/s</span>
            </div>
            <div class="metric-list-detail">Total RX ${formatBytes(n.rx_bytes || 0)} · TX ${formatBytes(n.tx_bytes || 0)}</div>
          </div>`,
        )
        .join('');
    }
  }

  function refreshLogsPanel() {
    const panel = el('logs-panel');
    if (!panel) return;
    preserveLogExplanations(panel);
    const form = el('log-filter');
    const params = form ? new URLSearchParams(new FormData(form)).toString() : '';
    const url = '/partials/logs' + (params ? '?' + params : '');
    htmx.ajax('GET', url, { target: '#logs-panel', swap: 'innerHTML' });
  }

  function refreshSecurityPanel() {
    const rows = el('security-rows');
    if (!rows) return;
    preserveLogExplanations(rows);
    htmx.ajax('GET', '/partials/security', { target: '#security-rows', swap: 'innerHTML' });
  }

  const EXPLANATION_TTL_MS = 60_000;

  function saveLogExplanations(container) {
    const explanations = new Map();
    container.querySelectorAll('.log-explanation').forEach((el) => {
      const row = el.closest('tr');
      if (!row) return;
      const message = row.querySelector('.message > span')?.textContent;
      if (message && el.textContent.trim()) {
        explanations.set(message, { html: sanitizeHtml(el.innerHTML), at: Date.now() });
      }
    });
    if (explanations.size === 0) return;
    try {
      const existing = JSON.parse(localStorage.getItem('observa-log-explanations') || '{}');
      const now = Date.now();
      explanations.forEach((value, key) => {
        existing[key] = value;
      });
      Object.keys(existing).forEach((key) => {
        if (now - existing[key].at > EXPLANATION_TTL_MS) delete existing[key];
      });
      localStorage.setItem('observa-log-explanations', JSON.stringify(existing));
    } catch (_) {
      return;
    }
  }

  function restoreLogExplanations(container) {
    try {
      const stored = JSON.parse(localStorage.getItem('observa-log-explanations') || '{}');
      const now = Date.now();
      let restored = 0;
      container.querySelectorAll('tr').forEach((row) => {
        const message = row.querySelector('.message > span')?.textContent;
        const target = row.querySelector('.log-explanation');
        if (!message || !target || target.textContent.trim()) return;
        const value = stored[message];
        if (value && now - value.at <= EXPLANATION_TTL_MS) {
          target.innerHTML = sanitizeHtml(value.html);
          restored += 1;
        }
      });
      if (restored > 0) {
        Object.keys(stored).forEach((key) => {
          if (now - stored[key].at > EXPLANATION_TTL_MS) delete stored[key];
        });
        localStorage.setItem('observa-log-explanations', JSON.stringify(stored));
      }
    } catch (_) {
      return;
    }
  }

  function preserveLogExplanations(container) {
    saveLogExplanations(container);
  }

  const ALLOWED_SEVERITY_CLASSES = new Set(['debug', 'info', 'warn', 'error', 'critical']);

  function severityClass(severity) {
    const s = String(severity || 'info').toLowerCase();
    return ALLOWED_SEVERITY_CLASSES.has(s) ? s : 'info';
  }

  function appendChatMessage(data) {
    const box = findChatMessagesBox();
    if (!box) return;
    const typing = box.querySelector('.typing-indicator');
    if (typing) typing.remove();
    const wrap = document.createElement('div');
    wrap.className = 'chat-message assistant';
    const raw = String(data.content || '');
    const html = sanitizeHtml(raw);
    wrap.innerHTML = `<div class="role">${escapeHtml(data.role || 'assistant')}</div>
      <div class="content">${html}</div>`;
    box.appendChild(wrap);
    scrollChat();
  }

  function findChatMessagesBox() {
    return el('chat-messages') || el('chat-widget-messages');
  }

  function updateSystemStatus(_data) {
    loadStatus();
  }

  function loadStatus() {
    fetch('/api/status')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (!data) return;
        const health = el('health-status-value');
        const chat = el('chat-heartbeat-value');
        const dot = el('system-status-dot');
        if (health) health.textContent = formatHealth(data.health);
        if (chat) chat.textContent = data.heartbeat_seq ? `#${data.heartbeat_seq}` : 'Waiting…';
        if (dot) {
          const ok = data.health === 'healthy';
          dot.className = 'status-dot' + (ok ? ' ok' : data.health === 'unhealthy' ? ' unhealthy' : ' degraded');
        }
      })
      .catch(() => {});
  }

  function updateInsightPanelFromStatus(_data) {
    loadInsights();
  }

  function loadInsights() {
    const panel = el('system-insight-panel');
    if (!panel) return;
    fetch('/api/insights')
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (!data || !data.insight) return;
        const summary = data.insight.summary;
        const health = data.insight.health;
        if (!summary) return;
        const cls = health === 'healthy' ? 'insight-healthy' : health === 'unhealthy' ? 'insight-unhealthy' : 'insight-degraded';
        panel.innerHTML = `<p class="insight-text ${cls}">${escapeHtml(summary)}</p>`;
      })
      .catch(() => {});
  }

  const ACTIVE_ALERTS = new Map();

  function wireSecurityPage() {
    const btn = document.getElementById('verify-chain-btn');
    if (!btn) return;
    const result = document.getElementById('verify-chain-result');
    btn.addEventListener('click', async () => {
      btn.disabled = true;
      btn.textContent = 'Verifying…';
      try {
        const response = await fetch('/api/alerts/verify-chain');
        const data = await response.json();
        if (data.ok) {
          result.className = 'verify-result verify-ok';
          result.textContent = `Chain verified. ${data.checked} alert(s) checked and no tampering detected.`;
        } else {
          result.className = 'verify-result verify-broken';
          result.textContent = `Chain broken for ${data.broken.length} alert(s): ${data.broken.join(', ')}`;
        }
        result.hidden = false;
      } catch (error) {
        result.className = 'verify-result verify-broken';
        result.textContent = 'Verification failed: ' + error.message;
        result.hidden = false;
      } finally {
        btn.disabled = false;
        btn.textContent = 'Verify chain';
      }
    });
  }

  async function showAlertToast(data) {
    if (!data || !data.message) return;
    const key = alertKey(data);
    if (!(await shouldShowAlert(data))) return;

    // Don't stack multiple toasts for the same alert key.
    if (ACTIVE_ALERTS.has(key)) return;

    const severity = severityClass(data.severity);
    const toast = document.createElement('div');
    toast.className = `alert-toast severity-${severity}`;
    toast.setAttribute('role', 'alert');
    toast.setAttribute('data-alert-key', key);
    toast.innerHTML = `
      <div class="alert-toast-header">
        <span class="alert-toast-severity">${escapeHtml(data.severity || 'Alert')}</span>
        <span class="alert-toast-pulse" aria-hidden="true"></span>
      </div>
      <p class="alert-toast-message">${escapeHtml(data.message)}</p>
      <div class="alert-toast-actions">
        <button class="alert-toast-ack" type="button">Acknowledge</button>
        <a href="/security" class="alert-toast-link">View →</a>
      </div>`;

    // Always anchor the container to the top-right of the viewport, even if
    // the stylesheet is cached or fails to load for some reason.
    let container = document.getElementById('toast-container');
    if (!container) {
      container = document.createElement('div');
      container.id = 'toast-container';
      container.className = 'toast-container';
      container.style.cssText = 'position:fixed;top:1rem;right:1rem;z-index:9999;display:flex;flex-direction:column;gap:0.75rem;max-width:min(28rem,calc(100vw - 2rem));pointer-events:none;';
      document.body.appendChild(container);
    }
    container.appendChild(toast);
    ACTIVE_ALERTS.set(key, toast);

    if (loadPreferences().alertSound === true) {
      playAlertSound();
    }

    const removeToast = () => {
      ACTIVE_ALERTS.delete(key);
      toast.remove();
    };

    // Alert toasts are sticky: they stay visible until explicitly acknowledged.
    // The View link lets the user navigate away without losing the alert; it does
    // not dismiss the toast so it will still be there when they come back.
    toast.querySelector('.alert-toast-ack').addEventListener('click', () => {
      acknowledgeAlert(key);
      removeToast();
    });
  }

  function playAlertSound(preview) {
    try {
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const ctx = new AudioCtx();
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = 'sine';
      // Two short beeps for attention.
      const now = ctx.currentTime;
      osc.frequency.setValueAtTime(880, now);
      osc.frequency.setValueAtTime(880, now + 0.15);
      gain.gain.setValueAtTime(0.0001, now);
      gain.gain.exponentialRampToValueAtTime(0.15, now + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.12);
      if (preview) {
        gain.gain.exponentialRampToValueAtTime(0.15, now + 0.17);
        gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.27);
      }
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(now);
      osc.stop(now + (preview ? 0.28 : 0.13));
      setTimeout(() => ctx.close(), 400);
    } catch (_) {
      // Audio is optional; ignore failures.
    }
  }

  function alertKey(data) {
    return [data.severity, data.message, data.ts].filter(Boolean).join('|');
  }

  function loadAcknowledgedAlerts() {
    try {
      const stored = localStorage.getItem('observa-acknowledged-alerts');
      if (stored) return JSON.parse(stored);
    } catch (_) {
      return [];
    }
    return [];
  }

  function acknowledgeAlert(key) {
    const current = loadAcknowledgedAlerts();
    if (current.includes(key)) return;
    current.push(key);
    try {
      localStorage.setItem('observa-acknowledged-alerts', JSON.stringify(current.slice(-100)));
    } catch (_) {
      return;
    }
    fetch('/api/alerts/acknowledge', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ key }),
    }).catch(() => {});
  }

  async function loadServerAcknowledgedAlerts() {
    try {
      const response = await fetch('/api/alerts/acknowledged');
      if (!response.ok) return [];
      const data = await response.json();
      return Array.isArray(data.acknowledged) ? data.acknowledged : [];
    } catch (_) {
      return [];
    }
  }

  async function shouldShowAlert(data) {
    const local = loadAcknowledgedAlerts();
    const key = alertKey(data);
    if (local.includes(key)) return false;
    const server = await loadServerAcknowledgedAlerts();
    return !server.includes(key);
  }

  function formatTime(value) {
    if (!value) return '—';
    const d = new Date(value);
    if (isNaN(d.getTime())) return String(value);
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function el(id) {
    return document.getElementById(id);
  }

  function loadHistory() {
    fetch('/api/metrics/history')
      .then((r) => r.json())
      .then((data) => {
        METRIC_HISTORY.length = 0;
        (data || [])
          .slice(-MAX_HISTORY)
          .forEach((snapshot) => METRIC_HISTORY.push(extractHistoryPoint(snapshot)));
        drawChart();
      })
      .catch(() => {});
  }

  function pushHistory(snapshot) {
    METRIC_HISTORY.push(extractHistoryPoint(snapshot));
    if (METRIC_HISTORY.length > MAX_HISTORY) METRIC_HISTORY.shift();
    drawChart();
  }

  function extractHistoryPoint(snapshot) {
    const cpu = snapshot.cpu?.usage_percent ?? 0;
    const memPct = snapshot.memory?.total_bytes
      ? (snapshot.memory.used_bytes / snapshot.memory.total_bytes) * 100
      : 0;
    return { cpu, mem: memPct };
  }

  let chartCtx, chartCanvas;
  let currentSnapshot = null;

  function initMeters() {
    if (!el('system-meters')) return;
    fetch('/api/metrics/latest')
      .then((r) => r.json())
      .then((data) => {
        if (data) updateMetersFromSnapshot(data);
      })
      .catch(() => {});
  }

  function refreshMeters() {
    if (!el('system-meters')) return;
    fetch('/api/metrics/latest')
      .then((r) => r.json())
      .then((data) => {
        if (data) updateMetersFromSnapshot(data);
      })
      .catch(() => {});
  }

  function updateMetersFromSnapshot(data) {
    if (!data) return;
    currentSnapshot = data;
    const cpu = data.cpu?.usage_percent ?? 0;
    const memPct = data.memory?.total_bytes
      ? (data.memory.used_bytes / data.memory.total_bytes) * 100
      : 0;
    setMeter('meter-cpu', 'meter-cpu-value', cpu, `${cpu.toFixed(1)}%`);
    setMeter('meter-mem', 'meter-mem-value', memPct, `${memPct.toFixed(0)}%`);

    const cpuLegend = el('legend-cpu');
    const memLegend = el('legend-memory');
    if (cpuLegend) cpuLegend.textContent = `${cpu.toFixed(1)}%`;
    if (memLegend) memLegend.textContent = `${memPct.toFixed(0)}%`;

    const diskContainer = el('system-meters');
    if (diskContainer) {
      (data.disks || []).forEach((disk, i) => {
        const pct = disk.total_bytes ? (disk.used_bytes / disk.total_bytes) * 100 : 0;
        setMeter(`meter-disk-${i + 1}`, `meter-disk-${i + 1}-value`, pct, `${pct.toFixed(0)}%`);
      });
      (data.networks || []).forEach((net, i) => {
        const maxBytes = Math.max(net.rx_bytes, net.tx_bytes, 1);
        const pct = Math.min(100, maxBytes / 10_000_000);
        setMeter(`meter-net-${i + 1}`, `meter-net-${i + 1}-value`, pct, `RX ${formatBytes(net.rx_bytes)} · TX ${formatBytes(net.tx_bytes)}`);
      });
    }

    const rxRate = data.networks?.reduce((sum, n) => sum + (n.rx_rate || 0), 0) ?? 0;
    const txRate = data.networks?.reduce((sum, n) => sum + (n.tx_rate || 0), 0) ?? 0;
    setMiniValue('hero-network-rx-value', formatRate(rxRate));
    setMiniValue('hero-network-tx-value', formatRate(txRate));
    if (el('network-rx-value')) setMiniValue('network-rx-value', formatRate(rxRate));
    if (el('network-tx-value')) setMiniValue('network-tx-value', formatRate(txRate));

    const gpuCount = data.gpu?.length ?? 0;
    const gpuUsed = data.gpu?.reduce((sum, g) => sum + (g.memory_used_bytes || 0), 0) ?? 0;
    const gpuTotal = data.gpu?.reduce((sum, g) => sum + (g.memory_total_bytes || 0), 0) ?? 0;
    if (gpuCount > 0) {
      setMiniValue('hero-gpu-memory-value', `${gpuCount} GPU${gpuCount === 1 ? '' : 's'}`);
      setMiniDetail('hero-gpu-memory-detail', `${formatBytes(gpuUsed)} / ${formatBytes(gpuTotal)}`);
      if (el('gpu-memory-value')) setMiniValue('gpu-memory-value', `${gpuCount} GPU${gpuCount === 1 ? '' : 's'}`);
      if (el('gpu-memory-detail')) setMiniDetail('gpu-memory-detail', `${formatBytes(gpuUsed)} / ${formatBytes(gpuTotal)}`);
    } else {
      setMiniValue('hero-gpu-memory-value', 'No GPU');
      setMiniDetail('hero-gpu-memory-detail', '—');
    }
  }

  function setMiniValue(id, text) {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
  }

  function setMiniDetail(id, text) {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
  }

  function formatRate(bytesPerSec) {
    if (bytesPerSec >= 1_000_000_000.0) {
      return `${(bytesPerSec / 1_000_000_000.0).toFixed(2)} GB/s`;
    } else if (bytesPerSec >= 1_000_000.0) {
      return `${(bytesPerSec / 1_000_000.0).toFixed(2)} MB/s`;
    } else if (bytesPerSec >= 1_000.0) {
      return `${(bytesPerSec / 1_000.0).toFixed(1)} KB/s`;
    } else {
      return `${bytesPerSec.toFixed(0)} B/s`;
    }
  }

  function setMeter(fillId, valueId, pct, text) {
    const fill = el(fillId);
    const value = el(valueId);
    if (fill) fill.style.width = `${Math.max(0, Math.min(100, pct))}%`;
    if (value) value.textContent = text;
  }

  function formatBytes(n) {
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    if (n === 0) return '0 B';
    const exp = Math.min(4, Math.floor(Math.log10(n) / Math.log10(1024)));
    const scaled = n / Math.pow(1024, exp);
    return `${scaled.toFixed(1)} ${units[exp]}`;
  }

  function initChart() {
    chartCanvas = el('metrics-chart');
    if (!chartCanvas) return;
    chartCtx = chartCanvas.getContext('2d');
    resizeCanvas(chartCanvas);
    window.addEventListener('resize', () => {
      resizeCanvas(chartCanvas);
      drawChart();
    });
  }

  function resizeCanvas(canvas) {
    const rect = canvas.parentElement.getBoundingClientRect();
    canvas.width = rect.width * window.devicePixelRatio;
    canvas.height = rect.height * window.devicePixelRatio;
  }

  function cssVar(name) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }

  function rgbaVar(name, alpha) {
    const raw = cssVar(name);
    if (!raw) return `rgba(0,0,0,${alpha})`;
    if (raw.startsWith('#')) {
      const hex = raw.replace('#', '');
      const bigint = parseInt(hex.length === 3 ? hex.split('').map((c) => c + c).join('') : hex, 16);
      const r = (bigint >> 16) & 255;
      const g = (bigint >> 8) & 255;
      const b = bigint & 255;
      return `rgba(${r}, ${g}, ${b}, ${alpha})`;
    }
    return raw;
  }

  function drawChart() {
    if (!chartCtx || !chartCanvas || METRIC_HISTORY.length < 2) return;
    const w = chartCanvas.width;
    const h = chartCanvas.height;
    const ctx = chartCtx;
    ctx.clearRect(0, 0, w, h);

    const axisPad = 28 * window.devicePixelRatio;
    const topPad = 12 * window.devicePixelRatio;
    const rightPad = 12 * window.devicePixelRatio;
    const gw = w - axisPad - rightPad;
    const gh = h - topPad - 12 * window.devicePixelRatio;

    const fg = cssVar('--fg');
    const cpuColor = cssVar('--accent');
    const memColor = cssVar('--accent-2');

    // Y-axis labels
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = rgbaVar('--fg', 0.55);
    ctx.font = `${9 * window.devicePixelRatio}px JetBrains Mono`;
    for (let i = 0; i <= 4; i++) {
      const pct = i * 25;
      const y = topPad + gh - (pct / 100) * gh;
      ctx.fillText(`${pct}%`, axisPad - 6 * window.devicePixelRatio, y);
    }

    // Horizontal grid lines
    ctx.strokeStyle = rgbaVar('--fg', 0.08);
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let i = 0; i <= 4; i++) {
      const y = topPad + gh - (i / 4) * gh;
      ctx.moveTo(axisPad, y);
      ctx.lineTo(axisPad + gw, y);
    }
    ctx.stroke();

    function drawLine(data, color) {
      ctx.beginPath();
      ctx.strokeStyle = color;
      ctx.lineWidth = 3 * window.devicePixelRatio;
      ctx.lineCap = 'round';
      ctx.lineJoin = 'round';
      data.forEach((pt, idx) => {
        const x = axisPad + (gw / (MAX_HISTORY - 1)) * idx;
        const y = topPad + gh - (pt / 100) * gh;
        if (idx === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      });
      ctx.stroke();

      // Final point dot
      const last = data[data.length - 1];
      const lx = axisPad + gw;
      const ly = topPad + gh - (last / 100) * gh;
      ctx.beginPath();
      ctx.fillStyle = color;
      ctx.arc(lx, ly, 3 * window.devicePixelRatio, 0, Math.PI * 2);
      ctx.fill();
    }

    const cpu = METRIC_HISTORY.map((m) => m.cpu);
    const mem = METRIC_HISTORY.map((m) => m.mem);
    drawLine(cpu, cpuColor);
    drawLine(mem, memColor);

    // Current value legend in DOM
    const cpuNow = cpu[cpu.length - 1];
    const memNow = mem[mem.length - 1];
    const cpuLegend = el('legend-cpu');
    const memLegend = el('legend-memory');
    if (cpuLegend) cpuLegend.textContent = `${cpuNow.toFixed(1)}%`;
    if (memLegend) memLegend.textContent = `${memNow.toFixed(0)}%`;
  }



  async function wireChat() {
    const panel = document.querySelector('.chat-panel');
    if (!panel) return;

    const urlParams = getChatUrlParams();
    const stored = loadWidgetSession();
    // The chat page renders the authoritative session/owner token; prefer it
    // over the widget localStorage state so the full-page chat and its history
    // stay in sync.
    let sessionId = panel.dataset.session || urlParams.sessionId || stored.sessionId || null;
    let ownerToken = panel.dataset.ownerToken || stored.ownerToken || null;

    if (urlParams.sessionId) {
      // A session id may be shared via URL for linking, but the owner token
      // must never appear in the query string.  Persist the id and load
      // messages using the token from localStorage or the rendered dataset.
      if (stored.sessionId !== urlParams.sessionId) {
        saveWidgetSession(urlParams.sessionId, ownerToken);
      }
      cleanOwnerTokenFromUrl();
    }

    if (!sessionId || !ownerToken) {
      // No session anywhere; create one and reload.
      try {
        ({ sessionId, ownerToken } = await ensureWidgetSession());
      } catch (err) {
        return;
      }
      window.location.href = buildChatUrl(sessionId);
      return;
    }

    panel.dataset.session = sessionId;
    panel.dataset.ownerToken = ownerToken;
    const form = el('chat-form');
    if (form) {
      form.dataset.session = sessionId;
      form.dataset.ownerToken = ownerToken;
      const sessionInput = form.querySelector('input[name="session_id"]');
      const ownerInput = form.querySelector('input[name="owner_token"]');
      if (sessionInput) sessionInput.value = sessionId;
      if (ownerInput) ownerInput.value = ownerToken;
    }

    // Load the conversation history via the partial endpoint using the
    // owner token header rather than the URL.
    const box = el('chat-messages');
    if (box) {
      loadFullPageMessagesFromServer(sessionId, ownerToken, box);
    }

    const input = el('chat-input');
    const spinner = el('chat-spinner');

    if (!form || !input || !box) return;

    bindChatSubmit(form, input, box, spinner, sessionId, ownerToken);
  }

  function getChatUrlParams() {
    const params = new URLSearchParams(window.location.search);
    return {
      sessionId: params.get('session_id') || null,
      ownerToken: null, // owner token must never be read from the URL
    };
  }

  function buildChatUrl(sessionId) {
    const url = new URL('/chat', window.location.origin);
    if (sessionId) url.searchParams.set('session_id', sessionId);
    return url.toString();
  }

  function cleanOwnerTokenFromUrl() {
    try {
      const url = new URL(window.location.href);
      url.searchParams.delete('owner_token');
      window.history.replaceState({}, '', url.toString());
    } catch (_) {
      /* no-op */
    }
  }

  async function loadFullPageMessagesFromServer(sessionId, ownerToken, box) {
    if (!sessionId || !ownerToken || !box) return;
    try {
      const response = await fetch(`/partials/chat?session_id=${encodeURIComponent(sessionId)}`, {
        headers: { 'X-Owner-Token': ownerToken },
      });
      if (response.ok) {
        const html = await response.text();
        box.innerHTML = html;
        scrollChat();
        saveChatHistory(box);
      }
    } catch (_) {
      /* no-op */
    }
  }

  function wireWidget() {
    const widget = el('chat-widget');
    if (!widget) return;

    let { sessionId, ownerToken } = loadWidgetSession();
    widget.dataset.session = sessionId || '';
    widget.dataset.ownerToken = ownerToken || '';
    const sessionInput = widget.querySelector('input[name="session_id"]');
    if (sessionInput) sessionInput.value = sessionId || '';
    const ownerInput = widget.querySelector('input[name="owner_token"]');
    if (ownerInput) ownerInput.value = ownerToken || '';
    if (sessionId && ownerToken) saveWidgetSession(sessionId, ownerToken);

    const toggle = el('chat-widget-toggle');
    const close = el('chat-widget-close');
    const card = el('chat-widget-card');
    const form = el('chat-widget-form');
    const input = el('chat-widget-input');
    const box = el('chat-widget-messages');
    const spinner = el('chat-widget-spinner');

    if (!toggle || !card || !form || !input || !box) return;

    function open(save = true) {
      card.hidden = false;
      widget.classList.add('is-open');
      toggle.setAttribute('aria-expanded', 'true');
      input.focus();
      if (save) saveWidgetOpen(true);
    }

    function closeWidget(save = true) {
      card.hidden = true;
      widget.classList.remove('is-open');
      toggle.setAttribute('aria-expanded', 'false');
      toggle.focus();
      if (save) saveWidgetOpen(false);
    }

    toggle.addEventListener('click', () => {
      if (card.hidden) open();
      else closeWidget();
    });

    if (close) close.addEventListener('click', closeWidget);

    // Restore the widget open/closed state from the last page visit.
    if (loadWidgetOpen()) {
      open(false);
    }

    document.addEventListener('keydown', (evt) => {
      if (evt.key === 'Escape' && !card.hidden) {
        closeWidget();
      }
    });

    bindChatSubmit(form, input, box, spinner, sessionId, ownerToken);
    if (sessionId && ownerToken) {
      loadWidgetMessagesFromServer(sessionId, ownerToken, box);
    } else {
      restoreChatHistory(sessionId, box);
    }
  }

  async function loadWidgetMessagesFromServer(sessionId, ownerToken, box) {
    if (!sessionId || !ownerToken || !box) return;
    try {
      const url = `/partials/chat?session_id=${encodeURIComponent(sessionId)}&owner_token=${encodeURIComponent(ownerToken)}`;
      const response = await fetch(url);
      if (response.ok) {
        const html = await response.text();
        box.innerHTML = html;
        scrollChat();
        saveChatHistory(box);
      } else {
        restoreChatHistory(sessionId, box);
      }
    } catch (_) {
      restoreChatHistory(sessionId, box);
    }
  }

  function wireMobileNav() {
    const toggle = el('nav-toggle');
    const menu = el('nav-menu');
    if (!toggle || !menu) return;

    toggle.addEventListener('click', () => {
      const isOpen = menu.classList.toggle('open');
      toggle.setAttribute('aria-expanded', String(isOpen));
      toggle.setAttribute('aria-label', isOpen ? 'Close navigation' : 'Open navigation');
    });

    menu.querySelectorAll('a').forEach((link) => {
      link.addEventListener('click', () => {
        menu.classList.remove('open');
        toggle.setAttribute('aria-expanded', 'false');
        toggle.setAttribute('aria-label', 'Open navigation');
      });
    });
  }

  function loadWidgetSession() {
    try {
      const stored = localStorage.getItem(WIDGET_STORAGE_KEY);
      if (stored) {
        const parsed = JSON.parse(stored);
        if (parsed && parsed.sessionId && parsed.ownerToken) return parsed;
      }
    } catch (_) {
      /* no-op */
    }
    return { sessionId: null, ownerToken: null };
  }

  function saveWidgetSession(sessionId, ownerToken) {
    try {
      localStorage.setItem(WIDGET_STORAGE_KEY, JSON.stringify({ sessionId, ownerToken }));
    } catch (_) {
      /* no-op */
    }
  }

  function loadWidgetOpen() {
    try {
      return localStorage.getItem(WIDGET_OPEN_KEY) === 'true';
    } catch (_) {
      return false;
    }
  }

  function saveWidgetOpen(isOpen) {
    try {
      localStorage.setItem(WIDGET_OPEN_KEY, isOpen ? 'true' : 'false');
    } catch (_) {
      /* no-op */
    }
  }

  async function ensureWidgetSession() {
    let { sessionId, ownerToken } = loadWidgetSession();
    if (sessionId && ownerToken) return { sessionId, ownerToken };
    try {
      const response = await fetch('/api/chat/session', { method: 'POST' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const data = await response.json();
      sessionId = data.session_id;
      ownerToken = data.owner_token;
      saveWidgetSession(sessionId, ownerToken);
      const widget = el('chat-widget');
      if (widget) {
        widget.dataset.session = sessionId;
        widget.dataset.ownerToken = ownerToken;
      }
      const sessionInput = widget && widget.querySelector('input[name="session_id"]');
      if (sessionInput) sessionInput.value = sessionId;
      const ownerInput = widget && widget.querySelector('input[name="owner_token"]');
      if (ownerInput) ownerInput.value = ownerToken;
    } catch (err) {
      throw new Error(`failed to create session: ${err.message || 'unknown'}`);
    }
    return { sessionId, ownerToken };
  }

  function bindChatSubmit(form, input, box, spinner, initialSessionId, initialOwnerToken) {
    form.addEventListener('submit', async (evt) => {
      evt.preventDefault();
      const text = input.value.trim();
      if (!text) return;

      let sessionId = initialSessionId;
      let ownerToken = initialOwnerToken;
      if (!sessionId || !ownerToken) {
        try {
          ({ sessionId, ownerToken } = await ensureWidgetSession());
        } catch (err) {
          appendChatBubble('assistant', `Error: ${err.message || 'failed to create session'}`);
          return;
        }
      }

      const empty = box.querySelector('.empty-state');
      if (empty) empty.remove();

      appendChatBubble('user', text);
      showTypingIndicator();
      if (spinner) spinner.classList.add('active');
      scrollChat();

      try {
        const response = await fetch('/api/chat/ask-html', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ session_id: sessionId, owner_token: ownerToken, message: text, system_prompt: getSystemPrompt() }),
        });
        const html = await response.text();
        if (response.ok) {
          appendAssistantBubble(html);
        } else {
          appendChatBubble('assistant', `Error: ${response.status} ${response.statusText}`);
        }
      } catch (err) {
        appendChatBubble('assistant', `Error: ${err.message || 'failed to reach assistant'}`);
      } finally {
        removeTypingIndicator();
        if (spinner) spinner.classList.remove('active');
        input.value = '';
        scrollChat();
        saveChatHistory(box);
      }
    });
  }

  function appendChatBubble(role, text) {
    const box = findChatMessagesBox();
    if (!box) return;
    removeEmptyState(box);
    const wrap = document.createElement('div');
    wrap.className = `chat-message ${role}`;
    wrap.innerHTML = `<div class="bubble"><div class="role">${escapeHtml(role)}</div><div class="content">${escapeHtml(text)}</div></div>`;
    box.appendChild(wrap);
    scrollChat();
    saveChatHistory(box);
  }

  function appendAssistantBubble(html) {
    const box = findChatMessagesBox();
    if (!box) return;
    removeEmptyState(box);
    const clean = extractReplyContent(html);
    const wrap = document.createElement('div');
    wrap.className = 'chat-message assistant';
    wrap.innerHTML = `<div class="bubble"><div class="role">assistant</div><div class="content">${clean}</div></div>`;
    box.appendChild(wrap);
    scrollChat();
    saveChatHistory(box);
  }

  function extractReplyContent(html) {
    const tmp = document.createElement('div');
    tmp.innerHTML = html;
    const content = tmp.querySelector('.content');
    const inner = content ? content.innerHTML : tmp.innerHTML;
    return sanitizeHtml(inner);
  }

  function removeEmptyState(box) {
    const empty = box.querySelector('.empty-state');
    if (empty) empty.remove();
  }

  function saveChatHistory(box) {
    if (!box) return;
    const sessionId = getChatSessionIdFromBox(box);
    if (!sessionId) return;
    const messages = [];
    box.querySelectorAll('.chat-message').forEach((msg) => {
      const role = msg.classList.contains('user') ? 'user' : 'assistant';
      const content = msg.querySelector('.content');
      if (!content) return;
      messages.push({ role, html: content.innerHTML });
    });
    try {
      const raw = sessionStorage.getItem(CHAT_HISTORY_KEY);
      const all = raw ? JSON.parse(raw) : {};
      if (messages.length) all[sessionId] = messages;
      else delete all[sessionId];
      sessionStorage.setItem(CHAT_HISTORY_KEY, JSON.stringify(all));
    } catch (_) {
      /* no-op */
    }
  }

  function restoreChatHistory(sessionId, box) {
    if (!sessionId || !box) return;
    try {
      const raw = sessionStorage.getItem(CHAT_HISTORY_KEY);
      const all = raw ? JSON.parse(raw) : {};
      const messages = all[sessionId];
      if (!Array.isArray(messages) || !messages.length) return;
      removeEmptyState(box);
      messages.forEach((m) => {
        const wrap = document.createElement('div');
        wrap.className = `chat-message ${m.role}`;
        const clean = extractReplyContent(m.html);
        wrap.innerHTML = `<div class="bubble"><div class="role">${escapeHtml(m.role)}</div><div class="content">${clean}</div></div>`;
        box.appendChild(wrap);
      });
      scrollChat();
    } catch (_) {
      /* no-op */
    }
  }

  function getChatSessionIdFromBox(box) {
    if (!box) return null;
    const panel = box.closest('.chat-panel');
    if (panel && panel.dataset.session) return panel.dataset.session;
    const widget = box.closest('.chat-widget');
    if (widget && widget.dataset.session) return widget.dataset.session;
    return null;
  }

  function showTypingIndicator() {
    const box = findChatMessagesBox();
    if (!box || box.querySelector('.typing-indicator')) return;
    const empty = box.querySelector('.empty-state');
    if (empty) empty.remove();
    const wrap = document.createElement('div');
    wrap.className = 'chat-message assistant typing-indicator';
    wrap.innerHTML = `<div class="bubble"><div class="role">assistant</div><div class="content"><span class="thinking-text">Assistant is thinking</span><span class="dots"><span></span><span></span><span></span></span></div></div>`;
    box.appendChild(wrap);
    scrollChat();
  }

  function removeTypingIndicator() {
    const box = findChatMessagesBox();
    if (!box) return;
    const typing = box.querySelector('.typing-indicator');
    if (typing) typing.remove();
  }

  function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  function sanitizeHtml(html) {
    const allowed = new Set(['BR', 'CODE', 'PRE', 'STRONG', 'EM', 'A']);
    const tmp = document.createElement('div');
    tmp.innerHTML = html;
    tmp.querySelectorAll('*').forEach((node) => {
      if (!allowed.has(node.tagName)) {
        node.replaceWith(document.createTextNode(node.outerHTML));
        return;
      }
      if (node.tagName === 'A') {
        const href = node.getAttribute('href') || '';
        if (!/^https?:\/\//i.test(href)) {
          node.removeAttribute('href');
        }
        node.setAttribute('rel', 'noopener noreferrer');
        node.setAttribute('target', '_blank');
      }
    });
    return tmp.innerHTML;
  }

  function scrollChat() {
    const box = findChatMessagesBox();
    if (box) box.scrollTop = box.scrollHeight;
  }

  function scrollLogs() {
    const box = el('logs-body');
    if (box) box.scrollTop = box.scrollHeight;
  }

  function updateSecurityCount() {
    const countEl = el('security-count');
    const rows = document.querySelectorAll('#security-rows .security-row');
    if (countEl) countEl.textContent = String(rows.length);
  }

  function loadPreferences() {
    try {
      const raw = localStorage.getItem(PREFS_KEY);
      return raw ? JSON.parse(raw) : {};
    } catch (_) {
      return {};
    }
  }

  function savePreferences(prefs) {
    const merged = { ...loadPreferences(), ...prefs };
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify(merged));
    } catch (_) {}
  }

  function initPreferences() {
    const prefs = loadPreferences();
    applyTheme(prefs.theme || 'dark');
    applyReducedMotion(prefs.reducedMotion || false);
    applyAutoRotateControls();
  }

  function getAutoRotateEnabled() {
    return loadPreferences().autoRotate === true;
  }

  function getAutoRotateSpeed() {
    const prefs = loadPreferences();
    const allowed = new Set(['slow', 'normal', 'fast']);
    return allowed.has(prefs.autoRotateSpeed) ? prefs.autoRotateSpeed : 'normal';
  }

  function isReducedMotionActive() {
    const userPref = document.documentElement.getAttribute('data-reduced-motion');
    if (userPref === 'true') return true;
    if (userPref === 'false') return false;
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  }

  function applyAutoRotateControls() {
    const toggle = document.getElementById('pref-auto-rotate');
    const speed = document.getElementById('pref-auto-rotate-speed');
    if (!toggle || !speed) return;
    const reduced = isReducedMotionActive();
    toggle.checked = getAutoRotateEnabled() && !reduced;
    toggle.disabled = reduced;
    speed.value = getAutoRotateSpeed();
    speed.disabled = reduced;
    document.documentElement.setAttribute('data-auto-rotate', String(toggle.checked));
    if (window.ObservaThreeViz && window.ObservaThreeViz.registry) {
      window.ObservaThreeViz.registry.applyAutoRotateToAll();
    }
  }

  function applyTheme(theme) {
    const resolved = theme === 'auto'
      ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
      : theme;
    document.documentElement.setAttribute('data-theme', resolved);
    const selector = document.getElementById('pref-theme');
    if (selector) selector.value = theme;
  }

  function applyReducedMotion(reduced) {
    document.documentElement.setAttribute('data-reduced-motion', String(reduced));
    const cb = document.getElementById('pref-reduced-motion');
    if (cb) cb.checked = reduced;
  }

  function refreshIntervalMs() {
    const prefs = loadPreferences();
    const raw = prefs.refreshIntervalMs;
    if (typeof raw === 'number' && raw > 0) return raw;
    if (raw === 0 || raw === false) return 0;
    return DEFAULT_REFRESH_MS;
  }

  function startRefreshLoop() {
    if (refreshIntervalId) clearInterval(refreshIntervalId);
    currentRefreshMs = refreshIntervalMs();
    const selector = document.getElementById('pref-refresh');
    if (selector) selector.value = String(currentRefreshMs);
    if (currentRefreshMs <= 0) return;
    refreshIntervalId = setInterval(() => {
      refreshMeters();
      refreshMetricsPanels();
      if (el('logs-panel')) refreshLogsPanel();
      if (el('security-rows')) refreshSecurityPanel();
    }, currentRefreshMs);
  }

  function initTheme() {
    const prefs = loadPreferences();
    applyTheme(prefs.theme || 'dark');
  }

  function initThreeViz() {
    const ribbon = document.getElementById('metrics-ribbon');
    const chart = document.getElementById('metrics-chart');
    const hasViz = document.querySelector('[data-three-viz]');
    if (!hasViz || !window.WebGLDetector || !window.WebGLDetector.isAvailable()) {
      enableChartFallback();
      return;
    }
    try {
      if (window.ObservaThreeViz) {
        window.ObservaThreeViz.init();
      }
      if (ribbon) ribbon.hidden = false;
      if (chart) chart.hidden = true;
    } catch (err) {
      console.warn('failed to init three.js viz, falling back to canvas', err);
      enableChartFallback();
    }
  }

  function enableChartFallback() {
    const ribbon = document.getElementById('metrics-ribbon');
    const chart = document.getElementById('metrics-chart');
    if (ribbon) ribbon.hidden = true;
    if (chart) chart.hidden = false;
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init().catch((err) => console.warn('Observa init failed', err));
  }

  const SYSTEM_PROMPT_PRESETS = {
    default: 'You are Observa, a terse system observability assistant. Answer in one or two sentences using the provided metrics and logs. Do not show your thinking process, chain-of-thought, or any internal analysis. Only output the final answer.',
    detailed: 'You are Observa, a system observability assistant. Use the provided metrics and logs to give a helpful, moderately detailed answer. Explain your reasoning briefly, then give a clear recommendation or summary. Keep answers under four sentences when possible.',
    concise: 'You are Observa. Reply with a single sentence. No preamble, no reasoning, no markdown.',
    pirate: 'Ahoy! Ye be Observa, a swashbucklin\' system observability assistant. Answer like a pirate captain readin\' the ship\'s log: use nautical words, call metrics "treasures" and logs "scrolls," and end with a hearty "Arrr!" Keep it to one or two sentences.',
  };

  function getSystemPrompt() {
    const prefs = loadPreferences();
    const raw = prefs.systemPrompt || '';
    if (raw.length > 4000) return raw.slice(0, 4000);
    return raw;
  }

  function updateSystemPromptCounter() {
    const countEl = document.getElementById('system-prompt-count');
    const textarea = document.getElementById('pref-system-prompt');
    if (!countEl || !textarea) return;
    countEl.textContent = `${textarea.value.length}/4000`;
  }

  function wireSystemPromptControls() {
    const textarea = document.getElementById('pref-system-prompt');
    if (!textarea) return;
    textarea.value = getSystemPrompt();
    updateSystemPromptCounter();
    textarea.addEventListener('input', () => {
      const trimmed = textarea.value.slice(0, 4000);
      if (textarea.value.length > 4000) textarea.value = trimmed;
      savePreferences({ systemPrompt: trimmed });
      updateSystemPromptCounter();
    });

    document.querySelectorAll('.system-prompt-presets .preset-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        const key = btn.dataset.preset;
        const value = SYSTEM_PROMPT_PRESETS[key] || '';
        textarea.value = value;
        savePreferences({ systemPrompt: value });
        updateSystemPromptCounter();
      });
    });
  }

  window.ObservaPreferences = {
    get autoRotate() { return getAutoRotateEnabled() && !isReducedMotionActive(); },
    get autoRotateSpeed() { return getAutoRotateSpeed(); },
    get reducedMotion() { return isReducedMotionActive(); },
    get systemPrompt() { return getSystemPrompt(); },
  };

  wirePreferencesPanel();
})();
