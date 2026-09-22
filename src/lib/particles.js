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
    this.resize = this.resize.bind(this);
    this.resize();
    window.addEventListener("resize", this.resize);
    this.seed();
  }

  resize() {
    const rect = this.canvas.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = Math.max(1, rect.width * dpr);
    this.canvas.height = Math.max(1, rect.height * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  seed() {
    const count = this.compact ? 520 : 900;
    this.points = Array.from({ length: count }, (_, i) => {
      const t = i / count;
      const angle = t * Math.PI * 2 * (8 + Math.random() * 2);
      const r = 0.58 + Math.random() * 0.42;
      return {
        angle, r,
        speed: 0.00065 + Math.random() * 0.0012,
        jitter: Math.random() * Math.PI * 2,
        size: 0.45 + Math.random() * 1.25,
        alpha: 0.16 + Math.random() * 0.84,
      };
    });
  }

  setLevel(level) { this.targetLevel = Math.max(0.02, Math.min(1, level)); }

  start() {
    if (this.running) return;
    this.running = true;
    const draw = (time) => {
      if (!this.running) return;
      this.draw(time);
      requestAnimationFrame(draw);
    };
    requestAnimationFrame(draw);
  }

  stop() { this.running = false; }

  draw(time) {
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    const ctx = this.ctx;
    ctx.clearRect(0, 0, w, h);
    this.level += (this.targetLevel - this.level) * 0.1;
    this.phase += 0.008 + this.level * 0.012;
    const cx = w / 2, cy = h / 2;
    const base = Math.min(w, h) * (this.compact ? 0.31 : 0.34);
    const energy = 1 + this.level * 0.17;

    ctx.save();
    ctx.globalCompositeOperation = "lighter";
    for (const p of this.points) {
      const wave = Math.sin(p.angle * 1.73 + this.phase * 2.4 + p.jitter) * (4 + this.level * 18);
      const wobble = Math.cos(p.angle * 0.71 - this.phase * 1.4) * (2 + this.level * 9);
      const angle = p.angle + time * p.speed * 0.05;
      const radius = base * p.r * energy + wave;
      const x = cx + Math.cos(angle) * radius;
      const y = cy + Math.sin(angle) * (radius * 0.78 + wobble);
      const edgeBoost = Math.max(0.12, p.r);
      ctx.fillStyle = `rgba(255,255,255,${p.alpha * edgeBoost * (0.62 + this.level * 0.38)})`;
      ctx.beginPath();
      ctx.arc(x, y, p.size + this.level * 0.75, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
  }
}
