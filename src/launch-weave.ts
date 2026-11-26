/* ---------------------------------------------------------------------------
   The strings behind a stream's splash.

   A streamed game lives on another machine, so the splash draws the link:
   a loom of fine strings across the game's art, violet on Orivo's side and
   blue towards the PC. They are slack while Moonlight looks for the PC, and
   each step the stream takes sends a pulse of light along them and draws
   them a little tighter. When the first picture arrives they close into one
   line of light, and that line opens like a seam onto the stream: the black
   the stream's own window starts on. Coming back plays it the other way —
   the seam closes from the edges of the screen, and the line comes apart
   into strings over the art again.

   Two canvases: the strings sit behind the words, the seam in front of
   everything. Only `transform`-free 2D drawing, at the screen's pixel ratio
   (at most 2), and nothing runs when motion is reduced: the strings are drawn
   once, and the seam becomes a plain fade.
   --------------------------------------------------------------------------- */

export type WeaveStage = "connecting" | "busy" | "closing" | "launching" | "handshake" | "visible";

export interface LaunchWeave {
  /** Starts drawing. The strings come in slack. */
  start(): void;
  /** One step further: a pulse along the strings, and a little more tension. */
  stage(stage: WeaveStage): void;
  /** Close into one line and open the seam over the whole screen. */
  dive(): Promise<void>;
  /** From black: the seam closes, the line comes apart into strings. */
  unfold(): Promise<void>;
  stop(): void;
}

export interface LaunchWeaveOptions {
  /** Behind the words. */
  strings: HTMLCanvasElement;
  /** In front of everything. */
  seam: HTMLCanvasElement;
  reducedMotion: () => boolean;
  /** Where the strings run, as a share of the screen's height. */
  line?: number;
}

const THREADS = 26;
const SEGMENTS = 88;
const DIVE_MS = 1_050;
const UNFOLD_MS = 1_150;

/** Orivo's side of the link, then the PC's: the ring light's own colours. */
const VIOLET = [125, 84, 244] as const;
const LILAC = [198, 178, 255] as const;
const BLUE = [79, 125, 255] as const;
const FROST = [169, 220, 255] as const;

/** How taut each step leaves the strings: slack while looking, nearly straight at the picture. */
const TENSION: Record<WeaveStage, number> = {
  connecting: 0.12,
  busy: 0.05,
  closing: 0.3,
  launching: 0.5,
  handshake: 0.74,
  visible: 0.9,
};

interface Thread {
  /** Resting offset from the line, from -1 to 1. */
  lane: number;
  amplitude: number;
  wavelength: number;
  speed: number;
  phase: number;
  width: number;
  alpha: number;
}

interface Pulse {
  /** Head position along the strings, 0 to 1.2 (it leaves the screen). */
  at: number;
  speed: number;
  strength: number;
}

const easeInOut = (t: number): number => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);
const easeOut = (t: number): number => 1 - (1 - t) ** 3;
const clamp01 = (t: number): number => Math.min(1, Math.max(0, t));
const mix = (a: number, b: number, t: number): number => a + (b - a) * t;

/** A seeded spread, so the loom looks the same every time it is drawn. */
function threads(): Thread[] {
  let seed = 7;
  const random = (): number => {
    seed = (seed * 16807) % 2147483647;
    return (seed - 1) / 2147483646;
  };
  return Array.from({ length: THREADS }, (_, index) => {
    const lane = (index / (THREADS - 1)) * 2 - 1;
    return {
      lane: lane * (0.55 + random() * 0.45),
      amplitude: 0.35 + random() * 0.65,
      wavelength: 0.55 + random() * 0.9,
      speed: 0.12 + random() * 0.22,
      phase: random() * Math.PI * 2,
      width: 0.55 + random() * 0.75,
      alpha: 0.22 + random() * 0.4,
    };
  });
}

function colourAt(t: number): readonly [number, number, number] {
  const stops = [VIOLET, LILAC, BLUE, FROST];
  const scaled = clamp01(t) * (stops.length - 1);
  const index = Math.min(stops.length - 2, Math.floor(scaled));
  const local = scaled - index;
  const from = stops[index];
  const to = stops[index + 1];
  return [mix(from[0], to[0], local), mix(from[1], to[1], local), mix(from[2], to[2], local)];
}

export function createLaunchWeave(options: LaunchWeaveOptions): LaunchWeave {
  const { strings, seam } = options;
  const line = options.line ?? 0.44;
  const loom = threads();
  const pulses: Pulse[] = [];
  let frame = 0;
  let running = false;
  let width = 0;
  let height = 0;
  let ratio = 1;
  let last = 0;
  let clock = 0;
  /** Eased towards `targetTension` every frame. */
  let tension = 0;
  let targetTension = TENSION.connecting;
  /** How present the strings are: they fade in at the start. */
  let presence = 0;
  /** Time runs slower while Orivo waits on the player. */
  let pace = 1;
  let targetPace = 1;
  /** 0 → 1 while the strings close into a line and the seam opens; 1 → 0 when it closes. */
  let collapse = 0;
  let opening = 0;
  let transition: { from: number; to: number; started: number; ms: number; done: () => void; kind: "dive" | "unfold" } | null = null;

  const contexts = (): [CanvasRenderingContext2D | null, CanvasRenderingContext2D | null] => [
    strings.getContext("2d"),
    seam.getContext("2d"),
  ];

  const resize = (): void => {
    ratio = Math.min(2, window.devicePixelRatio || 1);
    width = strings.clientWidth || window.innerWidth;
    height = strings.clientHeight || window.innerHeight;
    for (const canvas of [strings, seam]) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
    }
  };

  const drawStrings = (context: CanvasRenderingContext2D): void => {
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    if (presence <= 0.001) return;
    const centre = height * line;
    const band = height * mix(0.2, 0.035, tension) * (1 - collapse);
    const swing = height * mix(0.05, 0.006, tension) * (1 - collapse);
    const brightness = mix(0.75, 1.25, tension) * presence;
    context.globalCompositeOperation = "lighter";
    context.lineCap = "round";

    // The pulses, as a brightness along x: a soft head with a long tail.
    const glowAt = (x: number): number => {
      let glow = 0;
      for (const pulse of pulses) {
        const behind = pulse.at - x;
        if (behind < -0.04 || behind > 0.32) continue;
        const head = behind < 0 ? 1 + behind / 0.04 : 1 - behind / 0.32;
        glow += pulse.strength * head * head;
      }
      return Math.min(1.6, glow);
    };

    const xs = Array.from({ length: SEGMENTS + 1 }, (_, index) => index / SEGMENTS);
    const glows = xs.map(glowAt);
    for (const thread of loom) {
      const points = xs.map((x) => {
        // The strings are pinned off-screen on both sides and loosest in
        // the middle, the way a string between two points hangs.
        const envelope = Math.sin(Math.PI * (0.08 + x * 0.84));
        const wave =
          Math.sin((x / thread.wavelength) * Math.PI * 2 + clock * thread.speed * 2.4 + thread.phase) * 0.7 +
          Math.sin((x / (thread.wavelength * 0.43)) * Math.PI * 2 - clock * thread.speed * 1.3) * 0.3;
        const y = centre + thread.lane * band * envelope + wave * swing * thread.amplitude * envelope;
        return [x * width, y] as const;
      });
      // Base stroke, coloured from Orivo's side to the PC's.
      const gradient = context.createLinearGradient(0, 0, width, 0);
      for (const stop of [0, 0.35, 0.7, 1]) {
        const [r, g, b] = colourAt(stop);
        gradient.addColorStop(stop, `rgba(${r | 0}, ${g | 0}, ${b | 0}, ${Math.min(1, thread.alpha * brightness)})`);
      }
      context.strokeStyle = gradient;
      context.lineWidth = thread.width * mix(1, 1.25, collapse);
      context.beginPath();
      points.forEach(([x, y], index) => (index === 0 ? context.moveTo(x, y) : context.lineTo(x, y)));
      context.stroke();
      // Where a pulse is passing, the same string again, brighter and wider.
      if (pulses.length === 0) continue;
      for (let index = 0; index < SEGMENTS; index += 1) {
        const glow = (glows[index] + glows[index + 1]) / 2;
        if (glow < 0.04) continue;
        const [r, g] = colourAt(xs[index]);
        // Lit towards white, keeping the string's own hue.
        context.strokeStyle = `rgba(${Math.min(255, r + 60) | 0}, ${Math.min(255, g + 50) | 0}, 255, ${Math.min(1, glow * 0.5 * presence)})`;
        context.lineWidth = thread.width + glow * 1.4;
        context.beginPath();
        context.moveTo(points[index][0], points[index][1]);
        context.lineTo(points[index + 1][0], points[index + 1][1]);
        context.stroke();
      }
    }
    // The line they become: brightest when they have all closed into it.
    if (collapse > 0) {
      const glow = context.createLinearGradient(0, 0, width, 0);
      glow.addColorStop(0, "rgba(125, 84, 244, 0)");
      glow.addColorStop(0.22, `rgba(198, 178, 255, ${0.85 * collapse})`);
      glow.addColorStop(0.5, `rgba(255, 255, 255, ${collapse})`);
      glow.addColorStop(0.78, `rgba(169, 220, 255, ${0.85 * collapse})`);
      glow.addColorStop(1, "rgba(79, 125, 255, 0)");
      context.fillStyle = glow;
      context.fillRect(0, centre - 0.75, width, 1.5);
      context.globalAlpha = 0.35 * collapse;
      context.fillRect(0, centre - 6, width, 12);
      context.globalAlpha = 1;
    }
    context.globalCompositeOperation = "source-over";
  };

  /** The seam: black between two edges of light, opening from the line to the screen's edges. */
  const drawSeam = (context: CanvasRenderingContext2D): void => {
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    if (opening <= 0) return;
    const centre = height * line;
    // Both edges reach the screen's edges together, whatever the line's height.
    const top = centre - centre * opening;
    const bottom = centre + (height - centre) * opening;
    context.fillStyle = "#000";
    context.fillRect(0, top, width, bottom - top);
    const edge = 1 - opening;
    if (edge <= 0.001) return;
    for (const y of [top, bottom]) {
      const glow = context.createLinearGradient(0, 0, width, 0);
      glow.addColorStop(0, `rgba(125, 84, 244, ${0.25 * edge})`);
      glow.addColorStop(0.5, `rgba(255, 255, 255, ${0.95 * edge})`);
      glow.addColorStop(1, `rgba(79, 125, 255, ${0.25 * edge})`);
      context.fillStyle = glow;
      context.fillRect(0, y - 1, width, 2);
      context.globalAlpha = 0.3 * edge;
      context.fillRect(0, y - 10, width, 20);
      context.globalAlpha = 1;
    }
  };

  const draw = (): void => {
    const [stringsContext, seamContext] = contexts();
    if (stringsContext) drawStrings(stringsContext);
    if (seamContext) drawSeam(seamContext);
  };

  const step = (now: number): void => {
    if (!running) return;
    const delta = Math.min(0.05, last ? (now - last) / 1000 : 0);
    last = now;
    pace += (targetPace - pace) * Math.min(1, delta * 3);
    clock += delta * pace;
    tension += (targetTension - tension) * Math.min(1, delta * 1.6);
    if (!transition) presence = Math.min(1, presence + delta * 1.4);
    for (const pulse of pulses) pulse.at += pulse.speed * delta;
    while (pulses.length && pulses[0].at > 1.35) pulses.shift();

    if (transition) {
      const t = clamp01((now - transition.started) / transition.ms);
      if (transition.kind === "dive") {
        // First the strings close into the line, then the seam opens.
        collapse = easeInOut(clamp01(t / 0.45));
        opening = easeInOut(clamp01((t - 0.4) / 0.6));
      } else {
        // The seam closes onto the line, then the line comes apart.
        opening = 1 - easeInOut(clamp01(t / 0.5));
        collapse = 1 - easeOut(clamp01((t - 0.42) / 0.58));
        presence = 1;
      }
      if (t >= 1) {
        const done = transition.done;
        transition = null;
        done();
      }
    }
    draw();
    frame = requestAnimationFrame(step);
  };

  const run = (): void => {
    if (running) return;
    running = true;
    last = 0;
    frame = requestAnimationFrame(step);
  };

  const onResize = (): void => {
    resize();
    if (!running) draw();
  };

  const transitionTo = (kind: "dive" | "unfold", ms: number): Promise<void> =>
    new Promise((resolve) => {
      if (options.reducedMotion()) {
        // No seam to watch open: the screen goes to black, or comes back from it.
        collapse = kind === "dive" ? 1 : 0;
        opening = kind === "dive" ? 1 : 0;
        presence = 1;
        draw();
        resolve();
        return;
      }
      if (transition) transition.done();
      transition = { from: 0, to: 1, started: performance.now(), ms, done: resolve, kind };
      run();
    });

  return {
    start() {
      resize();
      window.addEventListener("resize", onResize);
      tension = 0;
      targetTension = TENSION.connecting;
      presence = 0;
      collapse = 0;
      opening = 0;
      pace = targetPace = 1;
      pulses.length = 0;
      if (options.reducedMotion()) {
        presence = 1;
        tension = targetTension;
        draw();
        return;
      }
      run();
    },
    stage(stage) {
      targetTension = TENSION[stage];
      targetPace = stage === "busy" ? 0.35 : 1;
      if (stage !== "busy") pulses.push({ at: -0.05, speed: 0.95, strength: stage === "visible" ? 1.4 : 1 });
      if (!running) {
        tension = targetTension;
        draw();
      }
    },
    dive() {
      return transitionTo("dive", DIVE_MS);
    },
    unfold() {
      collapse = 1;
      opening = 1;
      presence = 1;
      tension = targetTension = TENSION.visible;
      if (!running) {
        resize();
        window.addEventListener("resize", onResize);
      }
      draw();
      return transitionTo("unfold", UNFOLD_MS).then(() => {
        targetTension = TENSION.connecting;
      });
    },
    stop() {
      running = false;
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
      if (transition) {
        const done = transition.done;
        transition = null;
        done();
      }
    },
  };
}
