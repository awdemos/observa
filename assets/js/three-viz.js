/* Observa Three.js panel visualizations — liquid glass, lazy init, SSE driven. */
(function () {
  'use strict';

  const DATA_ATTR = 'data-three-viz';
  const REDUCED_MOTION = window.matchMedia('(prefers-reduced-motion: reduce)');

  function isMotionReduced() {
    const userPref = document.documentElement.getAttribute('data-reduced-motion');
    if (userPref === 'true') return true;
    if (userPref === 'false') return false;
    return REDUCED_MOTION.matches;
  }

  function clamp(v, min, max) {
    return Math.min(Math.max(v, min), max);
  }

  function cssVar(name, fallback) {
    const raw = getComputedStyle(document.documentElement).getPropertyValue(name);
    return (raw || fallback).trim();
  }

  function hexToRgb(hex) {
    const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex)
      || /^#?([a-f\d])([a-f\d])([a-f\d])$/i.exec(hex);
    if (!m) return { r: 128, g: 128, b: 128 };
    const f = (i) => (m[i].length === 1 ? m[i] + m[i] : m[i]);
    return {
      r: parseInt(f(1), 16),
      g: parseInt(f(2), 16),
      b: parseInt(f(3), 16),
    };
  }

  function colorToThree(hex) {
    const rgb = hexToRgb(hex);
    return new THREE.Color(`rgb(${rgb.r}, ${rgb.g}, ${rgb.b})`);
  }

  /* -------------------------------------------------------------------------- */
  /* WebGL detector                                                             */
  /* -------------------------------------------------------------------------- */

  const WebGLDetector = {
    isAvailable() {
      try {
        const c = document.createElement('canvas');
        return !!(window.WebGLRenderingContext && (c.getContext('webgl') || c.getContext('experimental-webgl')));
      } catch (_) {
        return false;
      }
    },
  };

  window.WebGLDetector = WebGLDetector;

  /* -------------------------------------------------------------------------- */
  /* Material library — liquid glass + glow                                   */
  /* -------------------------------------------------------------------------- */

  const MaterialLib = {
    envMap: null,

    ensureEnvMap(renderer) {
      if (this.envMap) return this.envMap;
      const scene = new THREE.Scene();
      const size = 128;
      const data = new Uint8Array(size * size * 4);
      for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
          const i = (y * size + x) * 4;
          const t = (y / size) * 0.5 + 0.2;
          data[i] = Math.floor(t * 60);
          data[i + 1] = Math.floor(t * 70);
          data[i + 2] = Math.floor(t * 90);
          data[i + 3] = 255;
        }
      }
      const tex = new THREE.DataTexture(data, size, size, THREE.RGBAFormat);
      tex.needsUpdate = true;
      scene.background = tex;
      const pmrem = new THREE.PMREMGenerator(renderer);
      pmrem.compileEquirectangularShader();
      this.envMap = pmrem.fromScene(scene, 0.04).texture;
      scene.background.dispose();
      tex.dispose();
      pmrem.dispose();
      return this.envMap;
    },

    liquidGlass(hex, renderer) {
      return new THREE.MeshPhysicalMaterial({
        color: colorToThree(hex),
        metalness: 0.1,
        roughness: 0.15,
        transmission: 0.95,
        ior: 1.5,
        clearcoat: 1.0,
        clearcoatRoughness: 0.1,
        transparent: true,
        opacity: 1.0,
        envMap: renderer ? this.ensureEnvMap(renderer) : null,
        envMapIntensity: 1.0,
        side: THREE.DoubleSide,
      });
    },

    solidGlass(hex, renderer) {
      return new THREE.MeshPhysicalMaterial({
        color: colorToThree(hex),
        metalness: 0.2,
        roughness: 0.25,
        transmission: 0.35,
        ior: 1.4,
        clearcoat: 0.8,
        clearcoatRoughness: 0.15,
        transparent: true,
        opacity: 0.9,
        envMap: renderer ? this.ensureEnvMap(renderer) : null,
        envMapIntensity: 0.8,
      });
    },

    glow(hex, opacity = 0.6) {
      return new THREE.MeshBasicMaterial({
        color: colorToThree(hex),
        transparent: true,
        opacity,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
      });
    },

    line(hex, opacity = 0.5) {
      return new THREE.LineBasicMaterial({
        color: colorToThree(hex),
        transparent: true,
        opacity,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
      });
    },
  };

  /* -------------------------------------------------------------------------- */
  /* Base scene                                                                */
  /* -------------------------------------------------------------------------- */

  class ThreeScene {
    constructor(container) {
      this.container = container;
      this.type = container.getAttribute(DATA_ATTR);
      this.running = false;
      this.visible = false;
      this.lastTime = performance.now();
      this.dpr = Math.min(window.devicePixelRatio || 1, 2);

      const rect = container.getBoundingClientRect();
      const width = Math.max(1, Math.floor(rect.width));
      const height = Math.max(1, Math.floor(rect.height));

      this.renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
      this.renderer.setPixelRatio(this.dpr);
      this.renderer.setSize(width, height, false);
      this.renderer.setClearColor(0x000000, 0);
      this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
      this.renderer.toneMappingExposure = 1.0;
      container.appendChild(this.renderer.domElement);

      this.scene = new THREE.Scene();
      this.camera = new THREE.PerspectiveCamera(45, width / height, 0.1, 1000);
      this.camera.position.set(0, 0, 5);

      this.controls = new THREE.OrbitControls(this.camera, this.renderer.domElement);
      this.controls.enableDamping = true;
      this.controls.dampingFactor = 0.06;
      this.controls.enablePan = false;
      this.controls.enableZoom = false;
      this.controls.enableRotate = true;
      this.controls.autoRotate = false;
      this.controls.autoRotateSpeed = 1.0;

      // Allow page scrollwheel to scroll; OrbitControls wheel listener is removed.
      this.renderer.domElement.addEventListener('wheel', (evt) => {
        evt.stopPropagation();
      }, { passive: true, capture: true });

      this.lights = this.buildLights();
      this.scene.add(...this.lights);

      this.build();
      this.applyAutoRotatePreferences();
      this.resize();
    }

    applyAutoRotatePreferences() {
      if (!this.controls) return;
      const prefs = window.ObservaPreferences || {};
      const speedMap = { slow: 0.4, normal: 1.0, fast: 2.0 };
      const speed = speedMap[prefs.autoRotateSpeed] ?? 1.0;
      this.controls.autoRotate = Boolean(prefs.autoRotate) && !isMotionReduced();
      this.controls.autoRotateSpeed = speed;
    }

    buildLights() {
      const key = new THREE.DirectionalLight(0xffffff, 1.2);
      key.position.set(3, 4, 5);
      const fill = new THREE.DirectionalLight(0xaaccff, 0.4);
      fill.position.set(-4, 1, -3);
      const rim = new THREE.PointLight(0xffffff, 0.8, 20);
      rim.position.set(0, 2, -4);
      return [key, fill, rim];
    }

    build() {
      // override in subclasses
    }

    update() {
      // override in subclasses
    }

    resize() {
      const rect = this.container.getBoundingClientRect();
      const width = Math.max(1, Math.floor(rect.width));
      const height = Math.max(1, Math.floor(rect.height));
      this.camera.aspect = width / height;
      this.camera.updateProjectionMatrix();
      this.renderer.setSize(width, height, false);
    }

    setVisible(v) {
      this.visible = v;
      if (v && !this.running) {
        this.running = true;
        this.lastTime = performance.now();
        this.animate();
      }
    }

    animate() {
      if (!this.running) return;
      requestAnimationFrame(() => this.animate());
      if (!this.visible) return;
      const now = performance.now();
      const dt = Math.min((now - this.lastTime) / 1000, 0.1);
      this.lastTime = now;
      this.tick(dt, now / 1000);
      this.controls.update();
      this.renderer.render(this.scene, this.camera);
    }

    tick() {
      // override in subclasses
    }

    dispose() {
      this.running = false;
      this.controls.dispose();
      this.renderer.dispose();
      this.scene.traverse((o) => {
        if (o.geometry) o.geometry.dispose();
        if (o.material) {
          if (Array.isArray(o.material)) o.material.forEach((m) => m.dispose());
          else o.material.dispose();
        }
      });
      if (this.renderer.domElement.parentNode) {
        this.renderer.domElement.parentNode.removeChild(this.renderer.domElement);
      }
    }
  }

  /* -------------------------------------------------------------------------- */
  /* System status resource orb                                               */
  /* -------------------------------------------------------------------------- */

  class SystemOrbScene extends ThreeScene {
    build() {
      this.coreRadius = 1.0;
      this.moons = [
        { name: 'CPU', key: 'cpu', var: '--accent', orbit: 1.7, speed: 0.5 },
        { name: 'Memory', key: 'mem', var: '--accent-2', orbit: 2.0, speed: 0.35 },
        { name: 'Disk', key: 'disk', var: '--accent-3', orbit: 2.3, speed: 0.25 },
        { name: 'Network', key: 'net', var: '--success', orbit: 2.6, speed: 0.4 },
        { name: 'GPU', key: 'gpu', var: '--warn', orbit: 2.9, speed: 0.3 },
      ];

      const coreMat = MaterialLib.liquidGlass(cssVar('--accent', '#00f0ff'), this.renderer);
      const coreGeo = new THREE.SphereGeometry(this.coreRadius, 64, 64);
      this.core = new THREE.Mesh(coreGeo, coreMat);
      this.scene.add(this.core);

      this.innerGlow = new THREE.Mesh(
        new THREE.SphereGeometry(this.coreRadius * 0.85, 32, 32),
        MaterialLib.glow(cssVar('--accent', '#00f0ff'), 0.25)
      );
      this.core.add(this.innerGlow);

      this.moonGroup = new THREE.Group();
      this.scene.add(this.moonGroup);

      this.moons.forEach((m, i) => {
        const mat = MaterialLib.liquidGlass(cssVar(m.var, '#ffffff'), this.renderer);
        const mesh = new THREE.Mesh(new THREE.SphereGeometry(0.22, 32, 32), mat);
        const angle = (i / this.moons.length) * Math.PI * 2;
        mesh.position.set(Math.cos(angle) * m.orbit, 0, Math.sin(angle) * m.orbit);
        const glow = new THREE.Mesh(new THREE.SphereGeometry(0.35, 16, 16), MaterialLib.glow(cssVar(m.var, '#ffffff'), 0.4));
        mesh.add(glow);
        m.mesh = mesh;
        m.glow = glow;
        m.angle = angle;
        this.moonGroup.add(mesh);
      });

      this.camera.position.set(0, 3, 6);
      this.controls.minDistance = 4;
      this.controls.maxDistance = 10;
      this.controls.target.set(0, 0, 0);
    }

    update(snapshot) {
      if (!snapshot) return;
      const cpu = snapshot.cpu?.usage_percent ?? 0;
      const mem = snapshot.memory?.total_bytes ? (snapshot.memory.used_bytes / snapshot.memory.total_bytes) * 100 : 0;
      const diskMax = (snapshot.disks || []).reduce((max, d) => Math.max(max, d.total_bytes ? (d.used_bytes / d.total_bytes) * 100 : 0), 0);
      const netRate = (snapshot.networks || []).reduce((sum, n) => sum + (n.rx_rate || 0) + (n.tx_rate || 0), 0);
      const net = Math.min(100, netRate / 20_000_000);
      const gpuMem = (snapshot.gpu || []).reduce((sum, g) => sum + (g.memory_total_bytes ? (g.memory_used_bytes / g.memory_total_bytes) : 0), 0);
      const gpu = snapshot.gpu?.length ? (gpuMem / snapshot.gpu.length) * 100 : 0;
      const loads = { cpu, mem, disk: diskMax, net, gpu };

      this.moons.forEach((m) => {
        const pct = clamp(loads[m.key] || 0, 0, 100);
        const s = 0.22 + (pct / 100) * 0.28;
        m.mesh.scale.setScalar(s);
        m.glow.scale.setScalar(1 + pct / 100);
        m.glow.material.opacity = 0.2 + (pct / 100) * 0.6;
        m.speed = 0.15 + (pct / 100) * 0.9;
      });

      const hottest = Object.entries(loads).sort((a, b) => b[1] - a[1])[0];
      const map = {
        cpu: cssVar('--accent', '#00f0ff'),
        mem: cssVar('--accent-2', '#ff2a8b'),
        disk: cssVar('--accent-3', '#ffe66d'),
        net: cssVar('--success', '#00ff9d'),
        gpu: cssVar('--warn', '#ff9f1c'),
      };
      const targetColor = colorToThree(map[hottest[0]] || cssVar('--accent', '#00f0ff'));
      this.core.material.color.lerp(targetColor, 0.05);
      this.innerGlow.material.color.copy(this.core.material.color);
    }

    tick(dt, t) {
      this.moons.forEach((m) => {
        m.angle += dt * m.speed;
        m.mesh.position.set(Math.cos(m.angle) * m.orbit, Math.sin(t + m.angle) * 0.2, Math.sin(m.angle) * m.orbit);
      });
      this.core.rotation.y += dt * 0.05;
      this.moonGroup.rotation.y += dt * 0.02;
    }
  }

  /* -------------------------------------------------------------------------- */
  /* Metrics trend depth ribbon                                                */
  /* -------------------------------------------------------------------------- */

  class MetricsRibbonScene extends ThreeScene {
    build() {
      this.maxPoints = 60;
      this.ribbonGroup = new THREE.Group();
      this.scene.add(this.ribbonGroup);

      this.cpuGeo = new THREE.BufferGeometry();
      this.memGeo = new THREE.BufferGeometry();
      this.cpuMat = MaterialLib.liquidGlass(cssVar('--accent', '#00f0ff'), this.renderer);
      this.memMat = MaterialLib.liquidGlass(cssVar('--accent-2', '#ff2a8b'), this.renderer);

      this.cpuMesh = new THREE.Mesh(this.cpuGeo, this.cpuMat);
      this.memMesh = new THREE.Mesh(this.memGeo, this.memMat);
      this.ribbonGroup.add(this.cpuMesh, this.memMesh);

      this.floor = new THREE.GridHelper(9, 18, colorToThree(cssVar('--border', '#333')), colorToThree(cssVar('--border', '#333')));
      this.floor.position.y = -0.05;
      this.floor.material.opacity = 0.18;
      this.floor.material.transparent = true;
      this.scene.add(this.floor);

      this.camera.position.set(0, 2.6, 6.5);
      this.controls.target.set(0, 1.1, 0);
      this.controls.minPolarAngle = Math.PI * 0.22;
      this.controls.maxPolarAngle = Math.PI * 0.52;
      this.controls.minDistance = 4;
      this.controls.maxDistance = 10;

      this.history = [];
    }

    setHistory(arr) {
      this.history = arr.slice(-this.maxPoints);
      this.rebuild();
    }

    update(snapshot) {
      if (!snapshot) return;
      if (window.OBSERVA_METRIC_HISTORY) {
        this.setHistory(window.OBSERVA_METRIC_HISTORY);
      }
    }

    buildRibbon(series, geo, zOffset, color) {
      const n = series.length;
      if (n < 2) return;
      const width = 8;
      const half = width / 2;
      const depth = 0.8;
      const pos = [];
      const idx = [];
      const colors = [];
      const baseColor = colorToThree(color);
      for (let i = 0; i < n; i++) {
        const t = i / (n - 1);
        const x = -half + t * width;
        const y = Math.max(0.02, (series[i] / 100) * 3.2);
        pos.push(x, y, zOffset, x, y, zOffset + depth);
        const dim = baseColor.clone().multiplyScalar(0.6 + 0.4 * t);
        colors.push(dim.r, dim.g, dim.b, dim.r, dim.g, dim.b);
      }
      for (let i = 0; i < n - 1; i++) {
        const a = i * 2;
        idx.push(a, a + 2, a + 1, a + 1, a + 2, a + 3);
      }
      if (geo.attributes.position) geo.dispose();
      geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
      geo.setAttribute('color', new THREE.Float32BufferAttribute(colors, 3));
      geo.setIndex(idx);
      geo.computeVertexNormals();
      if (geo.attributes.color) {
        if (!geo.userData.vertexColors) geo.userData.vertexColors = true;
      }
    }

    rebuild() {
      if (this.history.length < 2) return;
      this.cpuMat.vertexColors = true;
      this.memMat.vertexColors = true;
      this.buildRibbon(this.history.map((h) => h.cpu || 0), this.cpuGeo, 0, cssVar('--accent', '#00f0ff'));
      this.buildRibbon(this.history.map((h) => h.mem || 0), this.memGeo, 1.6, cssVar('--accent-2', '#ff2a8b'));
    }

  }

  /* -------------------------------------------------------------------------- */
  /* Security alerts constellation                                             */
  /* -------------------------------------------------------------------------- */

  class AlertsConstellationScene extends ThreeScene {
    build() {
      this.gemGroup = new THREE.Group();
      this.scene.add(this.gemGroup);
      this.alerts = [];
      this.maxAlerts = 50;
      this.camera.position.set(0, 0, 5);
      this.controls.minDistance = 3;
      this.controls.maxDistance = 10;
    }

    updateAlerts(alerts) {
      if (!Array.isArray(alerts)) return;
      const normalized = alerts.slice(-this.maxAlerts);
      const removed = this.alerts.filter((a) => !normalized.find((n) => n.id === a.id));
      removed.forEach((a) => {
        this.gemGroup.remove(a.mesh);
        a.mesh.geometry.dispose();
        a.mesh.material.dispose();
      });
      const kept = this.alerts.filter((a) => normalized.find((n) => n.id === a.id));
      const added = normalized.filter((n) => !kept.find((a) => a.id === n.id));
      added.forEach((alert) => kept.push(this.createGem(alert)));
      this.alerts = kept;
      this.layout();
    }

    createGem(alert) {
      const color = this.severityColor(alert.severity);
      const mat = MaterialLib.liquidGlass(color, this.renderer);
      const mesh = new THREE.Mesh(new THREE.IcosahedronGeometry(this.severitySize(alert.severity), 0), mat);
      const glow = new THREE.Mesh(new THREE.IcosahedronGeometry(0.35, 0), MaterialLib.glow(color, 0.5));
      mesh.add(glow);
      mesh.scale.setScalar(0);
      mesh.userData.targetScale = 1;
      mesh.userData.seed = Math.random() * 1000;
      return { id: alert.id, mesh, severity: alert.severity, created: performance.now() };
    }

    severityColor(sev) {
      const s = (sev || '').toLowerCase();
      if (s === 'critical' || s === 'error') return cssVar('--error', '#ff4d4d');
      if (s === 'warn' || s === 'warning') return cssVar('--warn', '#ff9f1c');
      return cssVar('--accent', '#00f0ff');
    }

    severitySize(sev) {
      const s = (sev || '').toLowerCase();
      if (s === 'critical') return 0.45;
      if (s === 'error') return 0.38;
      if (s === 'warn' || s === 'warning') return 0.32;
      return 0.25;
    }

    layout() {
      const count = this.alerts.length;
      this.alerts.forEach((a, i) => {
        const phi = Math.acos(1 - 2 * (i + 0.5) / count);
        const theta = Math.PI * (1 + Math.sqrt(5)) * i;
        const r = 1.8 + Math.sin(a.mesh.userData.seed) * 0.4;
        const x = r * Math.sin(phi) * Math.cos(theta);
        const y = r * Math.sin(phi) * Math.sin(theta);
        const z = r * Math.cos(phi);
        a.mesh.userData.targetPos = new THREE.Vector3(x, y, z);
      });
    }

    update(payload) {
      if (payload && payload.alerts) {
        this.updateAlerts(payload.alerts);
      }
    }

    tick(dt, t) {
      this.alerts.forEach((a) => {
        const m = a.mesh;
        if (m.userData.targetScale && m.scale.x < m.userData.targetScale) {
          const ns = Math.min(m.scale.x + dt * 2, m.userData.targetScale);
          m.scale.setScalar(ns);
        }
        if (m.userData.targetPos) {
          m.position.lerp(m.userData.targetPos, 0.03);
        }
        m.rotation.x += dt * 0.2;
        m.rotation.y += dt * 0.3;
        m.position.y += Math.sin(t + m.userData.seed) * 0.002;
      });
    }
  }

  /* -------------------------------------------------------------------------- */
  /* Network traffic graph                                                     */
  /* -------------------------------------------------------------------------- */

  class NetworkTrafficScene extends ThreeScene {
    build() {
      this.nodeGroup = new THREE.Group();
      this.linkGroup = new THREE.Group();
      this.pulseGroup = new THREE.Group();
      this.scene.add(this.nodeGroup, this.linkGroup, this.pulseGroup);

      this.hostMat = MaterialLib.liquidGlass(cssVar('--accent', '#00f0ff'), this.renderer);
      this.host = new THREE.Mesh(new THREE.SphereGeometry(0.5, 32, 32), this.hostMat);
      this.host.position.set(0, 0, 0);
      this.hostGlow = new THREE.Mesh(new THREE.SphereGeometry(0.7, 16, 16), MaterialLib.glow(cssVar('--accent', '#00f0ff'), 0.4));
      this.host.add(this.hostGlow);
      this.nodeGroup.add(this.host);

      this.nodes = [];
      this.links = [];
      this.pulses = [];

      this.camera.position.set(0, 3, 6);
      this.controls.target.set(0, 0, 0);
      this.controls.minDistance = 3;
      this.controls.maxDistance = 12;
    }

    updateNetwork(networks) {
      if (!Array.isArray(networks)) return;
      // Reconcile nodes
      const existing = new Map(this.nodes.map((n) => [n.name, n]));
      const current = new Set(networks.map((n) => n.name));
      this.nodes.forEach((n) => {
        if (!current.has(n.name)) {
          this.nodeGroup.remove(n.mesh);
          n.mesh.geometry.dispose();
          n.mesh.material.dispose();
        }
      });
      this.nodes = this.nodes.filter((n) => current.has(n.name));

      networks.forEach((n, i) => {
        const angle = (i / Math.max(networks.length, 1)) * Math.PI * 2;
        const r = 2.2;
        const pos = new THREE.Vector3(Math.cos(angle) * r, Math.sin(angle) * 0.3, Math.sin(angle) * r);
        let node = existing.get(n.name);
        if (!node) {
          const color = cssVar(i % 2 === 0 ? '--accent' : '--accent-2', '#00f0ff');
          const mesh = new THREE.Mesh(new THREE.SphereGeometry(0.3, 32, 32), MaterialLib.liquidGlass(color, this.renderer));
          const glow = new THREE.Mesh(new THREE.SphereGeometry(0.5, 16, 16), MaterialLib.glow(color, 0.35));
          mesh.add(glow);
          this.nodeGroup.add(mesh);
          node = { name: n.name, mesh, color };
          this.nodes.push(node);
        }
        node.mesh.position.copy(pos);
        node.rate = (n.rx_rate || 0) + (n.tx_rate || 0);
      });

      // Rebuild links
      this.links.forEach((l) => {
        l.tube.geometry.dispose();
        l.tube.material.dispose();
      });
      this.linkGroup.clear();
      this.links = [];
      this.nodes.forEach((node) => {
        const path = new THREE.LineCurve3(this.host.position, node.mesh.position);
        const geo = new THREE.TubeGeometry(path, 8, 0.03 + Math.min(0.06, node.rate / 50_000_000), 8, false);
        const intensity = Math.min(1, node.rate / 10_000_000);
        const mat = MaterialLib.line(node.color, 0.2 + intensity * 0.6);
        const tube = new THREE.Mesh(geo, mat);
        this.linkGroup.add(tube);
        this.links.push({ node, tube, intensity });
      });
    }

    update(snapshot) {
      if (snapshot && snapshot.networks) {
        this.updateNetwork(snapshot.networks);
      }
    }

    tick(dt, t) {
      for (let i = this.pulses.length - 1; i >= 0; i--) {
        const p = this.pulses[i];
        p.t += dt * p.speed;
        if (p.t >= 1) {
          this.pulseGroup.remove(p.mesh);
          p.mesh.geometry.dispose();
          p.mesh.material.dispose();
          this.pulses.splice(i, 1);
          continue;
        }
        const start = this.host.position;
        const end = p.node.mesh.position;
        p.mesh.position.lerpVectors(start, end, p.t);
      }

      this.links.forEach((link) => {
        if (link.node.rate > 0 && Math.random() < link.intensity * dt * 2) {
          const pulse = new THREE.Mesh(new THREE.SphereGeometry(0.08, 8, 8), MaterialLib.glow(link.node.color, 0.9));
          pulse.position.copy(this.host.position);
          this.pulseGroup.add(pulse);
          this.pulses.push({ mesh: pulse, node: link.node, t: 0, speed: 0.2 + link.intensity * 1.2 });
        }
      });
    }
  }

  class NetworkCombinedScene extends NetworkTrafficScene {}

  class LogStreamScene extends ThreeScene {
    build() {
      this.particles = [];
      this.maxParticles = 36;
      this.pathGroup = new THREE.Group();
      this.scene.add(this.pathGroup);

      const pathGeo = new THREE.BufferGeometry();
      const pathPos = [];
      for (let i = 0; i <= 80; i++) {
        const t = i / 80;
        const x = Math.sin(t * Math.PI * 2) * 1.2;
        const y = Math.cos(t * Math.PI * 1.5) * 0.6;
        const z = -3 + t * 5.5;
        pathPos.push(x, y, z);
      }
      pathGeo.setAttribute('position', new THREE.Float32BufferAttribute(pathPos, 3));
      const pathMat = MaterialLib.line(cssVar('--fg-dim', '#555555'), 0.1);
      this.streamLine = new THREE.Line(pathGeo, pathMat);
      this.pathGroup.add(this.streamLine);

      this.camera.position.set(0, 0.8, 4.2);
      this.controls.target.set(0, 0, 0);
      this.controls.minDistance = 2;
      this.controls.maxDistance = 8;

      this.pathPoints = [];
      for (let i = 0; i <= 80; i++) {
        const t = i / 80;
        this.pathPoints.push(new THREE.Vector3(
          Math.sin(t * Math.PI * 2) * 1.2,
          Math.cos(t * Math.PI * 1.5) * 0.6,
          -3 + t * 5.5
        ));
      }
    }

    severityColor(sev) {
      const s = (sev || '').toLowerCase();
      if (s === 'critical' || s === 'error') return cssVar('--error', '#e53935');
      if (s === 'warn' || s === 'warning') return cssVar('--warn', '#cc8800');
      return cssVar('--accent', '#00b8c2');
    }

    addLog(log) {
      if (!log) return;
      const color = this.severityColor(log.severity_class || log.severity);
      const mat = MaterialLib.liquidGlass(color, this.renderer);
      const mesh = new THREE.Mesh(new THREE.IcosahedronGeometry(0.12, 0), mat);
      const glow = new THREE.Mesh(new THREE.IcosahedronGeometry(0.22, 0), MaterialLib.glow(color, 0.55));
      mesh.add(glow);
      this.pathGroup.add(mesh);
      this.particles.push({
        mesh,
        t: 0,
        speed: 0.08 + Math.random() * 0.06 + (log.severity_class === 'critical' ? 0.1 : 0),
        seed: Math.random() * 1000,
      });
      while (this.particles.length > this.maxParticles) {
        const old = this.particles.shift();
        this.pathGroup.remove(old.mesh);
        old.mesh.geometry.dispose();
        old.mesh.children.forEach((c) => c.geometry && c.geometry.dispose());
        old.mesh.material.dispose();
      }
    }

    setLogs(list) {
      this.particles.forEach((p) => {
        this.pathGroup.remove(p.mesh);
        p.mesh.geometry.dispose();
        p.mesh.children.forEach((c) => c.geometry && c.geometry.dispose());
        p.mesh.material.dispose();
      });
      this.particles = [];
      if (Array.isArray(list)) {
        list.slice(-this.maxParticles).forEach((l) => this.addLog(l));
      }
    }

    update(payload) {
      if (payload && Array.isArray(payload.logs)) {
        this.setLogs(payload.logs);
      } else if (payload && payload.severity) {
        this.addLog(payload);
      }
    }

    applyAutoRotatePreferences() {
      if (!this.controls) return;
      this.controls.autoRotate = false;
      this.controls.autoRotateSpeed = 0;
    }

    tick(dt, t) {
      for (let i = this.particles.length - 1; i >= 0; i--) {
        const p = this.particles[i];
        p.t += dt * p.speed;
        if (p.t >= 1) {
          this.pathGroup.remove(p.mesh);
          p.mesh.geometry.dispose();
          p.mesh.children.forEach((c) => c.geometry && c.geometry.dispose());
          p.mesh.material.dispose();
          this.particles.splice(i, 1);
          continue;
        }
        const idx = Math.min(79, Math.floor(p.t * 80));
        const frac = p.t * 80 - idx;
        const a = this.pathPoints[idx];
        const b = this.pathPoints[idx + 1];
        p.mesh.position.lerpVectors(a, b, frac);
        p.mesh.position.y += Math.sin(t * 2 + p.seed) * 0.05;
        const scale = 1 - Math.pow(p.t - 0.5, 2) * 1.5;
        p.mesh.scale.setScalar(Math.max(0.4, scale));
        p.mesh.rotation.x += dt * 0.5;
        p.mesh.rotation.y += dt * 0.7;
      }
    }

    dispose() {
      super.dispose();
      if (this.streamLine && this.streamLine.material) this.streamLine.material.dispose();
    }
  }

  /* -------------------------------------------------------------------------- */
  /* Registry                                                                  */
  /* -------------------------------------------------------------------------- */

  const FACTORY = {
    orb: SystemOrbScene,
    ribbon: MetricsRibbonScene,
    alerts: AlertsConstellationScene,
    network: NetworkTrafficScene,
    networkCombined: NetworkCombinedScene,
    logs: LogStreamScene,
  };

  class VizRegistry {
    constructor() {
      this.instances = new Map();
      this.observer = new IntersectionObserver(
        (entries) => entries.forEach((e) => this.handleIntersection(e)),
        { root: null, threshold: 0.1 }
      );
      this.resizeTimer = null;
      window.addEventListener('resize', () => {
        clearTimeout(this.resizeTimer);
        this.resizeTimer = setTimeout(() => this.resizeAll(), 150);
      });
      document.addEventListener('visibilitychange', () => this.handleVisibility());
      REDUCED_MOTION.addEventListener('change', () => this.handleMotionChange());
    }

    scan() {
      document.querySelectorAll(`[${DATA_ATTR}]`).forEach((el) => {
        if (this.instances.has(el)) return;
        const type = el.getAttribute(DATA_ATTR);
        if (!FACTORY[type]) return;
        if (!WebGLDetector.isAvailable()) {
          el.classList.add('no-webgl');
          return;
        }
        try {
          const scene = new FACTORY[type](el);
          this.instances.set(el, scene);
          this.observer.observe(el);
          if (type === 'ribbon' && window.OBSERVA_METRIC_HISTORY) {
            scene.setHistory(window.OBSERVA_METRIC_HISTORY);
          }
          if (type === 'alerts') {
            const dataEl = document.getElementById('security-data');
            if (dataEl) {
              try {
                const data = JSON.parse(dataEl.textContent || '[]');
                scene.updateAlerts(data.map((a, i) => ({ ...a, id: a.id || `${a.source}-${a.message}-${i}` })));
              } catch (_) {
                // ignore
              }
            }
          }
          if (type === 'network' || type === 'networkCombined') {
            const dataEl = document.getElementById('network-data');
            if (dataEl) {
              try {
                const data = JSON.parse(dataEl.textContent || '[]');
                scene.updateNetwork(data);
              } catch (_) {
              }
            }
          }
          if (type === 'logs') {
            const dataEl = document.getElementById('log-data');
            if (dataEl) {
              try {
                const data = JSON.parse(dataEl.textContent || '[]');
                scene.setLogs(data);
              } catch (_) {
              }
            }
          }
        } catch (err) {
          console.warn('Failed to init Three.js scene', type, err);
          el.classList.add('no-webgl');
          el.setAttribute('aria-label', 'Visualization failed: ' + (err && err.message ? err.message : String(err)));
          this.addRetryButton(el);
        }
      });
    }

    addRetryButton(container) {
      if (!container.dataset.threeViz) return;
      if (container.querySelector('.viz-retry')) return;
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'viz-retry';
      btn.textContent = 'Retry visualization';
      btn.addEventListener('click', () => {
        btn.remove();
        container.classList.remove('no-webgl');
        container.removeAttribute('aria-label');
        this.scan();
      });
      container.appendChild(btn);
    }

    handleIntersection(entry) {
      const scene = this.instances.get(entry.target);
      if (!scene) return;
      scene.setVisible(entry.isIntersecting);
    }

    handleVisibility() {
      this.instances.forEach((scene) => scene.setVisible(document.visibilityState === 'visible' && scene.visible));
    }

    handleMotionChange() {
      this.applyAutoRotateToAll();
    }

    applyAutoRotateToAll() {
      this.instances.forEach((scene) => {
        if (scene.applyAutoRotatePreferences) scene.applyAutoRotatePreferences();
      });
    }

    resizeAll() {
      this.instances.forEach((scene) => scene.resize());
    }

    update(type, payload) {
      this.instances.forEach((scene) => {
        if (type === 'metric' && scene.update) scene.update(payload);
        if (type === 'log' && scene.update) {
          if (payload && (payload.severity || payload.logs)) {
            scene.update(payload);
          } else {
            this.refreshSceneData(scene);
          }
        }
      });
    }

    refreshSceneData(scene) {
      if (scene.type === 'alerts') {
        const dataEl = document.getElementById('security-data');
        if (dataEl) {
          try {
            const data = JSON.parse(dataEl.textContent || '[]');
            scene.updateAlerts(data.map((a, i) => ({ ...a, id: a.id || `${a.source}-${a.message}-${i}` })));
          } catch (_) {
          }
        }
      }
      if (scene.type === 'logs') {
        const dataEl = document.getElementById('log-data');
        if (dataEl) {
          try {
            const data = JSON.parse(dataEl.textContent || '[]');
            scene.setLogs(data);
          } catch (_) {
          }
        }
      }
    }

    disposeFor(element) {
      const scene = this.instances.get(element);
      if (scene) {
        this.observer.unobserve(element);
        scene.dispose();
        this.instances.delete(element);
      }
    }

    disposeAll() {
      this.instances.forEach((scene, el) => {
        this.observer.unobserve(el);
        scene.dispose();
      });
      this.instances.clear();
    }
  }

  /* -------------------------------------------------------------------------- */
  /* Public API                                                                */
  /* -------------------------------------------------------------------------- */

  let registry = null;

  window.ObservaThreeViz = {
    WebGLDetector,
    MaterialLib,
    SystemOrbScene,
    MetricsRibbonScene,
    AlertsConstellationScene,
    NetworkTrafficScene,
    NetworkCombinedScene,
    LogStreamScene,
    isAvailable() {
      return WebGLDetector.isAvailable();
    },
    init() {
      if (!WebGLDetector.isAvailable()) return null;
      if (!registry) registry = new VizRegistry();
      registry.scan();
      return registry;
    },
    get registry() {
      return registry;
    },
    update(type, payload) {
      if (registry) registry.update(type, payload);
    },
    dispose() {
      if (registry) {
        registry.disposeAll();
        registry = null;
      }
    },
  };
})();
