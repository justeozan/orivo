/**
 * Controller bridge.
 *
 * The Gamepad API has no events for button presses, only a snapshot you have to
 * poll, so this reads the pads once per frame while one is connected and turns
 * the deltas into the same verbs the keyboard uses. Nothing here touches the
 * DOM beyond a body flag — every action goes through `spatial-nav.ts`.
 */

import type { NavDirection } from "./spatial-nav";

export interface GamepadActions {
  move(direction: NavDirection): void;
  activate(): void;
  back(): void;
  launch(): void;
  focusSearch(): void;
  cycleNav(delta: number): void;
  scroll(delta: number): void;
  /** Any pad input at all, so the focus ring can switch to controller mode. */
  onActivity(): void;
}

export interface GamepadBridge {
  start(): void;
  stop(): void;
  destroy(): void;
}

const DEAD_ZONE = 0.55;
const REPEAT_DELAY = 420;
const REPEAT_INTERVAL = 110;

// Standard mapping. Face buttons first, then shoulders, then the d-pad.
const A = 0;
const B = 1;
const X = 2;
const Y = 3;
const LB = 4;
const RB = 5;
const START = 9;
const DPAD_UP = 12;
const DPAD_DOWN = 13;
const DPAD_LEFT = 14;
const DPAD_RIGHT = 15;

const pressed = (pad: Gamepad, index: number): boolean => pad.buttons[index]?.pressed === true;

export function createGamepadBridge(actions: GamepadActions): GamepadBridge {
  let frame = 0;
  let running = false;
  const held = new Set<number>();
  let heldDirection: NavDirection | null = null;
  let directionSince = 0;
  let directionLast = 0;

  const setConnected = (connected: boolean): void => {
    if (connected) document.body.dataset.gamepad = "connected";
    else delete document.body.dataset.gamepad;
  };

  const readDirection = (pad: Gamepad): NavDirection | null => {
    if (pressed(pad, DPAD_UP)) return "up";
    if (pressed(pad, DPAD_DOWN)) return "down";
    if (pressed(pad, DPAD_LEFT)) return "left";
    if (pressed(pad, DPAD_RIGHT)) return "right";
    const x = pad.axes[0] ?? 0;
    const y = pad.axes[1] ?? 0;
    if (Math.abs(x) < DEAD_ZONE && Math.abs(y) < DEAD_ZONE) return null;
    // One axis at a time: diagonals on a stick are almost always a mis-push.
    if (Math.abs(x) >= Math.abs(y)) return x > 0 ? "right" : "left";
    return y > 0 ? "down" : "up";
  };

  /** Edge detection: a held button must not fire on every single frame. */
  const tapped = (pad: Gamepad, index: number): boolean => {
    const down = pressed(pad, index);
    const was = held.has(index);
    if (down && !was) {
      held.add(index);
      return true;
    }
    if (!down && was) held.delete(index);
    return false;
  };

  const poll = (now: number): void => {
    const pads = navigator.getGamepads?.() ?? [];
    const pad = Array.from(pads).find((entry): entry is Gamepad => entry !== null);

    if (pad) {
      const direction = readDirection(pad);
      if (direction !== heldDirection) {
        heldDirection = direction;
        directionSince = now;
        directionLast = now;
        if (direction) {
          actions.onActivity();
          actions.move(direction);
        }
      } else if (direction) {
        // Hold to repeat, with the usual slow-then-fast ramp.
        const elapsed = now - directionSince;
        if (elapsed > REPEAT_DELAY && now - directionLast > REPEAT_INTERVAL) {
          directionLast = now;
          actions.move(direction);
        }
      }

      if (tapped(pad, A)) {
        actions.onActivity();
        actions.activate();
      }
      if (tapped(pad, B)) {
        actions.onActivity();
        actions.back();
      }
      if (tapped(pad, X)) {
        actions.onActivity();
        actions.launch();
      }
      if (tapped(pad, Y)) {
        actions.onActivity();
        actions.focusSearch();
      }
      if (tapped(pad, LB)) {
        actions.onActivity();
        actions.cycleNav(-1);
      }
      if (tapped(pad, RB)) {
        actions.onActivity();
        actions.cycleNav(1);
      }
      if (tapped(pad, START)) {
        actions.onActivity();
        actions.activate();
      }

      const rightStickY = pad.axes[3] ?? 0;
      if (Math.abs(rightStickY) > DEAD_ZONE) {
        actions.onActivity();
        actions.scroll(rightStickY * 28);
      }
    }

    frame = window.requestAnimationFrame(poll);
  };

  const start = (): void => {
    if (running) return;
    running = true;
    setConnected(true);
    frame = window.requestAnimationFrame(poll);
  };

  const stop = (): void => {
    if (!running) return;
    running = false;
    window.cancelAnimationFrame(frame);
    held.clear();
    heldDirection = null;
    setConnected(false);
  };

  const onConnected = (): void => start();
  const onDisconnected = (): void => {
    const pads = navigator.getGamepads?.() ?? [];
    if (!Array.from(pads).some((entry) => entry !== null)) stop();
  };

  window.addEventListener("gamepadconnected", onConnected);
  window.addEventListener("gamepaddisconnected", onDisconnected);

  // A pad that was already connected before load only shows up once polled.
  if (Array.from(navigator.getGamepads?.() ?? []).some((entry) => entry !== null)) start();

  return {
    start,
    stop,
    destroy() {
      stop();
      window.removeEventListener("gamepadconnected", onConnected);
      window.removeEventListener("gamepaddisconnected", onDisconnected);
    },
  };
}
