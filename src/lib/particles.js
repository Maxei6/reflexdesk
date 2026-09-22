const COLORS = {
  idle: [235, 235, 240],
  ready: [235, 235, 240],
  listening: [105, 223, 255],
  hearing: [69, 241, 255],
  transcribing: [91, 157, 255],
  thinking: [169, 125, 255],
  executing: [117, 99, 255],
  success: [93, 236, 145],
  warning: [255, 183, 78],
  error: [255, 86, 108],
};

export class ParticleOrb {
  constructor(canvas, { compact = false } = {}) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");
    this.compact = compact;
    this.level = 0.08;
    this.targetLevel = 0.08;
    this.phase = 0;
    this.points = [];
    this.running = false;
    this.rafId = null;
    this.state = "idle";
    this.color = [...COLORS.idle];
    this.targetColor = [...COLORS.idle];
    this.reducedMotion = false;

    if (typeof window !== "undefined" && window.matchMedia) {
      const mediaQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
      this.reducedMotion = Boolean(mediaQuery && mediaQuery.matches);
      if (mediaQuery && typeof mediaQuery.addEventListener === "function") {
        mediaQuery.addEventListener("change", (e) => {
          this.reducedMotion = Boolean(e.matches);
          if (this.reducedMotion) {
            this.stopRaf();
            this.drawStaticFrame();
          } else if (this.running) {
            this.start();
          }
        });
      }
    }

    this.resize = this.resize.bind(this);
    this.resize();
    if (typeof window !== "undefined") {
      window.addEventListener("resize", this.resize);
    }
    this.seed();
  }

  resize() {
    const rect = this.canvas.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = Math.max(1, Math.round(rect.width * dpr));
    this.canvas.height = Math.max(1, Math.round(rect.height * dpr));
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (this.reducedMotion) {
      this.drawStaticFrame();
    }
  }

  seed() {
    const count = this.compact ? 280 : 620;
    this.points = Array.from({ length: count }, (_, i) => {
      const t = i / count;
      const angle = t * Math.PI * 2 * (7 + Math.random() * 2);
      const r = 0.56 + Math.random() * 0.44;
      return {
        angle,
        r,
        speed: 0.0007 + Math.random() * 0.00135,
        jitter: Math.random() * Math.PI * 2,
        size: 0.4 + Math.random() * 1.15,
        alpha: 0.15 + Math.random() * 0.82,
      };
    });
  }

  setLevel(level) {
    this.targetLevel = Math.max(0.02, Math.min(1, level));
    if (this.reducedMotion) {
      this.level = this.targetLevel;
      this.drawStaticFrame();
    }
  }

  setState(state) {
    if (!COLORS[state]) state = "idle";
    this.state = state;
    this.targetColor = [...COLORS[state]];

    const floor = {
      idle: 0.05,
      ready: 0.08,
      listening: 0.18,
      hearing: 0.55,
      transcribing: 0.5,
      thinking: 0.48,
      executing: 0.65,
      success: 0.42,
      warning: 0.4,
      error: 0.48,
    }[state];

    this.setLevel(Math.max(this.targetLevel, floor || 0.08));
    if (this.reducedMotion) {
      this.color = [...this.targetColor];
      this.drawStaticFrame();
    }
  }

  start() {
    this.running = true;
    if (this.reducedMotion) {
      this.stopRaf();
      this.drawStaticFrame();
      return;
    }
    if (this.rafId !== null) return;
    const draw = (time) => {
      if (!this.running || this.reducedMotion) {
        this.stopRaf();
        return;
      }
      this.draw(time);
      this.rafId = requestAnimationFrame(draw);
    };
    this.rafId = requestAnimationFrame(draw);
  }

  stopRaf() {
    if (this.rafId !== null) {
      if (typeof cancelAnimationFrame === "function") {
        cancelAnimationFrame(this.rafId);
      }
      this.rafId = null;
    }
  }

  stop() {
    this.running = false;
    this.stopRaf();
  }

  drawStaticFrame() {
    this.draw(0);
  }

  draw(time) {
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    const ctx = this.ctx;
    ctx.clearRect(0, 0, w, h);

    this.level += (this.targetLevel - this.level) * 0.1;
    this.phase += 0.008 + this.level * 0.013;

    for (let i = 0; i < 3; i += 1) {
      this.color[i] += (this.targetColor[i] - this.color[i]) * 0.08;
    }

    const cx = w / 2;
    const cy = h / 2;
    const base = Math.min(w, h) * (this.compact ? 0.285 : 0.32);
    const energy = 1 + this.level * 0.18;
    const red = Math.round(this.color[0]);
    const green = Math.round(this.color[1]);
    const blue = Math.round(this.color[2]);

    ctx.save();
    ctx.globalCompositeOperation = "lighter";

    for (const p of this.points) {
      const wave =
        Math.sin(p.angle * 1.73 + this.phase * 2.5 + p.jitter)
        * (2.6 + this.level * 11);
      const wobble =
        Math.cos(p.angle * 0.71 - this.phase * 1.45)
        * (1.5 + this.level * 6);

      const angle = p.angle + time * p.speed * 0.05;
      const radius = base * p.r * energy + wave;
      const x = cx + Math.cos(angle) * radius;
      const y = cy + Math.sin(angle) * (radius * 0.74 + wobble);
      const edgeBoost = Math.max(0.12, p.r);
      const alpha = p.alpha * edgeBoost * (0.58 + this.level * 0.42);

      ctx.fillStyle = "rgba(" + red + "," + green + "," + blue + "," + alpha + ")";
      ctx.beginPath();
      ctx.arc(x, y, p.size + this.level * 0.55, 0, Math.PI * 2);
      ctx.fill();
    }

    ctx.restore();
  }
}
