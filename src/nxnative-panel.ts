/* ---------------------------------------------------------------------------
   Settings › Plugins › Native Switch (Switch Natif in French), built on nxnative.

   Drawn by docs/DESIGN.md §4.4: words, pills and buttons, no paragraph.

     - Build (or Stop) in the detail header: pick a game, follow
       `--progress json`, the Mac app lands in the library (nxnative_build);
     - Requirements: one line with a count pill and Install all, then only
       what is missing, each with the button that fixes it. A fix runs inside
       Orivo; its output streams under its row, the whole log behind a
       toggle (nxnative_status, nxnative_run_fix, nxnative_install_all);
     - Games, once there are any: each game's own settings, written to the
       app's saved choices (nxnative_games, nxnative_set_choice).

   The host keeps every path. The page sees check states, folder names and
   build events, and names a link or a setting, never a file.
   --------------------------------------------------------------------------- */

import { t, type MessageKey } from "./i18n";
import { icon, type IconName } from "./icons";

export type CheckState = "ok" | "missing" | "outdated";

export interface NxnativeCheck {
  id: string;
  state: CheckState;
  found: string | null;
  fix: string | null;
}

export interface NxnativeStatus {
  supported: boolean;
  checkout: string | null;
  checks: NxnativeCheck[];
  building: boolean;
  /** The item being installed, while an install runs. */
  fixing: string | null;
  keysChosen: boolean;
}

export interface LauncherSetting {
  env: string;
  kind: string;
  section: string | null;
  label: string | null;
  note: string | null;
  default: string | null;
  options: [string, string][];
  min: number | null;
  max: number | null;
  step: number | null;
  on: string | null;
  off: string | null;
  level: string | null;
  needs: string | null;
}

export interface NxnativeGame {
  gameId: string;
  title: string;
  titleId: string;
  settings: LauncherSetting[];
  saved: Record<string, string>;
  /** Orivo starts it covering the screen on the player's desktop: its Display mode is Orivo's. */
  keepsSpace: boolean;
}

/** The setting Orivo sets itself for a game it starts on the player's desktop. */
export const DISPLAY_MODE = "DM_FULLSCREEN";

/** The settings the panel draws for a game: all of them, less the ones Orivo sets at Play. */
export function drawnSettings(game: NxnativeGame): LauncherSetting[] {
  return game.keepsSpace ? game.settings.filter((setting) => setting.env !== DISPLAY_MODE) : game.settings;
}

export type BuildEvent =
  | { kind: "stage"; index: number; count: number; name: string }
  | { kind: "step"; name: string }
  | { kind: "progress"; name: string; fraction: number | null; etaSeconds: number | null; note: string | null }
  | { kind: "stage-end"; name: string; ok: boolean; summary: string | null; warn: boolean }
  | { kind: "shaders-stale"; count: number | null }
  | { kind: "game"; titleId: string; name: string; version: string | null }
  | { kind: "say"; message: string; error: boolean };

export type BuildOutcome = "finished" | "failed" | "invalid-arguments" | "cancelled" | "crashed";

export type BuildMessage =
  | { type: "started"; game: string }
  | { type: "event"; event: BuildEvent }
  | { type: "finished"; outcome: BuildOutcome; message: string | null; gameId: string | null };

/** What the build area shows. One build at a time, so one of these. */
export interface BuildView {
  running: boolean;
  game: string;
  stage: string;
  step: string;
  /** 0–1, or null while the current work cannot be counted. */
  fraction: number | null;
  etaSeconds: number | null;
  lastMessage: string;
  outcome: BuildOutcome | null;
  error: string | null;
  platform: "mac-app" | "ipa";
}

export const IDLE_BUILD: Readonly<BuildView> = Object.freeze({
  running: false,
  game: "",
  stage: "",
  step: "",
  fraction: null,
  etaSeconds: null,
  lastMessage: "",
  outcome: null,
  error: null,
  platform: "mac-app",
});

/** Fold one host message into the build view. Pure, so it is tested alone. */
export function applyBuildMessage(view: BuildView, message: BuildMessage): BuildView {
  switch (message.type) {
    case "started":
      return { ...IDLE_BUILD, platform: view.platform, running: true, game: message.game };
    case "finished":
      return {
        ...view,
        running: false,
        fraction: message.outcome === "finished" ? 1 : view.fraction,
        etaSeconds: null,
        outcome: message.outcome,
        error: message.message,
      };
    case "event": {
      const event = message.event;
      switch (event.kind) {
        case "stage":
          return {
            ...view,
            stage: t("nxnative.stage", { index: event.index, count: event.count, name: event.name }),
            step: "",
            fraction: null,
            etaSeconds: null,
          };
        case "step":
          return { ...view, step: event.name, fraction: null, etaSeconds: null };
        case "progress":
          return {
            ...view,
            step: event.note ? `${event.name} · ${event.note}` : event.name,
            fraction: event.fraction,
            etaSeconds: event.etaSeconds,
          };
        case "game":
          return { ...view, game: event.name || view.game };
        case "say":
          return event.error
            ? { ...view, lastMessage: event.message, error: event.message }
            : { ...view, lastMessage: event.message };
        default:
          return view;
      }
    }
  }
}

/** Every check passed: the Build buttons are offered. */
export function readyToBuild(status: NxnativeStatus | null): boolean {
  return Boolean(status?.supported && status.checks.length && status.checks.every((check) => check.state === "ok"));
}

/** What is still to fix, in the checklist's order. */
export function missingChecks(status: NxnativeStatus): NxnativeCheck[] {
  return status.checks.filter((check) => check.state !== "ok");
}

/* ---------------------------------------------------------------------------
   Installs: each fix runs inside Orivo and streams its output as
   `nxnative-fix` messages, shown under its row.
   --------------------------------------------------------------------------- */

export type FixMessage =
  | { type: "started"; check: string }
  | { type: "line"; check: string; line: string }
  | { type: "finished"; check: string; ok: boolean; message: string | null }
  | { type: "done" };

export interface FixLog {
  state: "running" | "ok" | "failed";
  lines: string[];
  message: string | null;
}

/** The log keeps its last lines only: Homebrew's progress alone is hundreds. */
export const MAX_LOG_LINES = 400;

/** Fold one host message into the logs. Pure, so it is tested alone. */
export function applyFixMessage(logs: Record<string, FixLog>, message: FixMessage): Record<string, FixLog> {
  switch (message.type) {
    case "started":
      return { ...logs, [message.check]: { state: "running", lines: [], message: null } };
    case "line": {
      const log = logs[message.check] ?? { state: "running", lines: [], message: null };
      const lines = [...log.lines, message.line];
      return { ...logs, [message.check]: { ...log, lines: lines.slice(-MAX_LOG_LINES) } };
    }
    case "finished": {
      const log = logs[message.check] ?? { state: "running", lines: [], message: null };
      return { ...logs, [message.check]: { ...log, state: message.ok ? "ok" : "failed", message: message.message } };
    }
    case "done":
      return logs;
  }
}

/** The items Orivo can install itself, in the order "Install all" takes them. */
export const FIXABLE = ["xcode", "metal-toolchain", "build-tools", "python", "python-modules", "checkout"];

export function canFix(check: NxnativeCheck): boolean {
  return check.state !== "ok" && FIXABLE.includes(check.id);
}

/** The nxnative row is always drawn: it carries Update and the folder's Change. */
export const ALWAYS_DRAWN = "checkout";

/** The rows drawn: nxnative, what is missing, and what was installed in this session. */
export function visibleChecks(status: NxnativeStatus, logs: Record<string, FixLog>): NxnativeCheck[] {
  return status.checks.filter((check) => check.id === ALWAYS_DRAWN || check.state !== "ok" || check.id in logs);
}

/** The folder is made without a question when there is none, once, if git is there to do it. */
export function shouldAutoClone(status: NxnativeStatus | null, logs: Record<string, FixLog>, tried: boolean): boolean {
  if (!status?.supported || status.fixing !== null || tried || "checkout" in logs) return false;
  return status.checks.some((check) => check.id === "checkout" && check.state === "missing");
}

/** A check's state, as the one or two words of its pill. */
export function checkPill(check: NxnativeCheck): string {
  if (check.state === "ok") return t("nxnative.state.ok");
  if (check.state === "outdated") return check.found ?? t("nxnative.state.outdated");
  return t("nxnative.state.missing");
}

/** What was found, for the row's tooltip: a missing list, a commit, a version. */
export function checkDetail(check: NxnativeCheck): string {
  return check.found ?? "";
}

/** The outcome as one short line, or null while nothing has finished. */
export function outcomeLine(view: BuildView): string | null {
  switch (view.outcome) {
    case null:
      return null;
    case "finished":
      return view.platform === "ipa" ? t("nxnative.finishedIpa") : t("nxnative.finished");
    case "cancelled":
      return t("nxnative.cancelled");
    default:
      return view.error ? t("nxnative.failed", { message: view.error }) : t("nxnative.failedUnknown");
  }
}

/** A setting's row is drawn only while the row it `needs` has that value. */
export function settingVisible(setting: LauncherSetting, values: Record<string, string>): boolean {
  if (!setting.needs) return true;
  const [variable, wanted] = setting.needs.split("=", 2);
  if (!variable || wanted === undefined) return true;
  if (!(variable in values)) return true;
  return values[variable] === wanted;
}

/** What each setting is set to now: the saved choice, else the tested default. */
export function currentValues(game: NxnativeGame): Record<string, string> {
  const values: Record<string, string> = {};
  for (const setting of game.settings) {
    const value = game.saved[setting.env] ?? setting.default;
    if (value !== null && value !== undefined) values[setting.env] = value;
  }
  return values;
}

/* ---------------------------------------------------------------------------
   The view
   --------------------------------------------------------------------------- */

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, handler: (event: { payload: T }) => void) => Promise<unknown>;

export interface NxnativePanelDeps {
  invoke: Invoke;
  listen: Listen;
  toast: (message: string) => void;
  /** A built game was added to the library. */
  libraryChanged: () => void;
  /** Start a game from the library, splash and all, as its card's Play does. */
  play: (gameId: string) => void;
  /** The plugin was removed: its card moves to the catalogue. */
  removed: () => void;
  /** Games folders were added or looked in: the library may have changed. */
  foldersChanged?: () => void;
}

export interface NxnativePanel {
  /** The panel was opened: read the Mac and the games again. */
  open(): void;
}

const CHECK_LINKS: Record<string, { link: string; label: MessageKey }> = {
  xcode: { link: "xcode", label: "nxnative.getXcode" },
  homebrew: { link: "homebrew", label: "nxnative.getHomebrew" },
};

export function mountNxnativePanel(root: HTMLElement, deps: NxnativePanelDeps): NxnativePanel {
  let status: NxnativeStatus | null = null;
  let loading = false;
  let logs: Record<string, FixLog> = {};
  const openLogs = new Set<string>();
  let build: BuildView = { ...IDLE_BUILD };
  /** The card the last finished build added, for its Play button. */
  let builtGameId: string | null = null;
  let games: NxnativeGame[] = [];
  let autoCloneTried = false;
  // The panel's one main action (Build, or Stop while building) lives in the
  // detail header, as Save does for Wallpaper Searcher.
  const headerActions = root.closest(".plugin-detail")?.querySelector<HTMLElement>(".plugin-detail__actions") ?? null;

  const refresh = async (): Promise<void> => {
    loading = true;
    render();
    try {
      status = await deps.invoke<NxnativeStatus>("nxnative_status");
      if (status.building && !build.running) build = { ...build, running: true };
      // Opened while an install runs: its row shows it running.
      if (status.fixing && !logs[status.fixing]) {
        logs = applyFixMessage(logs, { type: "started", check: status.fixing });
      }
      games = status.supported ? await deps.invoke<NxnativeGame[]>("nxnative_games") : [];
    } catch (error) {
      deps.toast(String(error));
    } finally {
      loading = false;
      render();
    }
    // No folder yet: Orivo makes one, as the Download button would have.
    if (shouldAutoClone(status, logs, autoCloneTried)) {
      autoCloneTried = true;
      runFix("nxnative_run_fix", "checkout");
    }
  };

  void deps.listen<BuildMessage>("nxnative-build", ({ payload }) => {
    build = applyBuildMessage(build, payload);
    if (payload.type === "started") builtGameId = null;
    if (payload.type === "finished") {
      builtGameId = payload.gameId;
      if (payload.gameId) deps.libraryChanged();
      void refresh();
    }
    render();
  });
  void deps.listen<FixMessage>("nxnative-fix", ({ payload }) => {
    logs = applyFixMessage(logs, payload);
    if (status && payload.type === "started") status = { ...status, fixing: payload.check };
    if (payload.type === "finished" && !payload.ok) openLogs.add(payload.check);
    if (payload.type === "done") {
      if (status) status = { ...status, fixing: null };
      void refresh();
    }
    scheduleRender();
  });

  // Output arrives line by line; drawing once a frame is plenty.
  let renderQueued = false;
  const scheduleRender = (): void => {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      render();
    });
  };

  const runFix = (command: "nxnative_run_fix" | "nxnative_install_all", check?: string): void => {
    if (status) status = { ...status, fixing: check ?? FIXABLE[0] };
    render();
    void deps.invoke(command, check ? { check } : {}).catch((error) => {
      if (status) status = { ...status, fixing: null };
      deps.toast(String(error));
      render();
    });
  };

  const startBuild = async (platform: "mac-app" | "ipa"): Promise<void> => {
    build = { ...IDLE_BUILD, platform };
    render();
    try {
      await deps.invoke<boolean>("nxnative_build", { platform });
    } catch (error) {
      deps.toast(String(error));
    }
  };

  const update = (): void => {
    if (status) status = { ...status, fixing: "checkout" };
    render();
    void deps.invoke("nxnative_update").catch((error) => {
      if (status) status = { ...status, fixing: null };
      deps.toast(String(error));
      render();
    });
  };

  const remove = async (): Promise<void> => {
    try {
      await deps.invoke("nxnative_remove");
      status = null;
      games = [];
      logs = {};
      autoCloneTried = false;
      deps.removed();
    } catch (error) {
      deps.toast(String(error));
    }
  };

  function render(): void {
    const body = element("div", "nxnative-panel");
    headerActions?.replaceChildren();

    if (!status || !status.supported) {
      const text = !status ? (loading ? t("nxnative.loading") : "") : t("nxnative.unsupported");
      body.append(element("p", "nxnative-note", text));
      root.replaceChildren(body);
      return;
    }

    renderHeaderActions(status);
    body.append(renderRequirements(status));
    const progress = renderProgress();
    if (progress) body.append(progress);
    if (games.length) body.append(renderGames());
    root.replaceChildren(body);
    for (const log of root.querySelectorAll<HTMLElement>(".nxnative-log")) log.scrollTop = log.scrollHeight;
  }

  function renderHeaderActions(current: NxnativeStatus): void {
    if (!headerActions) return;
    if (build.running) {
      const stop = button(t("nxnative.cancel"), "primary");
      stop.addEventListener("click", () => void deps.invoke("nxnative_cancel_build"));
      headerActions.append(stop);
      return;
    }
    const trash = iconButton("trash", t("nxnative.remove"));
    trash.disabled = current.fixing !== null;
    trash.addEventListener("click", () => void remove());
    headerActions.append(trash);
    const ready = readyToBuild(current);
    const ipa = button(t("nxnative.buildIpa"));
    ipa.title = ready ? t("nxnative.buildIpaTitle") : t("nxnative.notReady");
    ipa.disabled = !ready;
    ipa.addEventListener("click", () => void startBuild("ipa"));
    const mac = button(t("nxnative.build"), "primary");
    mac.title = ready ? t("nxnative.buildMacTitle") : t("nxnative.notReady");
    mac.disabled = !ready;
    mac.addEventListener("click", () => void startBuild("mac-app"));
    headerActions.append(ipa, mac);
  }

  /** One head line (name, count, Install all, re-check), then a line per item. */
  function renderRequirements(current: NxnativeStatus): HTMLElement {
    const section = element("section", "nxnative-section");
    const todo = missingChecks(current);
    const busy = current.fixing !== null;

    const head = element("div", "nxnative-head");
    head.append(element("span", "plugin-field__label", t("nxnative.requirements")));
    head.append(
      element(
        "span",
        todo.length ? "ws-pill" : "ws-pill ws-pill--on",
        todo.length
          ? t("nxnative.count", { done: current.checks.length - todo.length, total: current.checks.length })
          : t("nxnative.state.ok"),
      ),
    );
    if (todo.some(canFix)) {
      const all = button(t("nxnative.installAll"), "primary");
      all.disabled = busy;
      all.addEventListener("click", () => runFix("nxnative_install_all"));
      head.append(all);
    }
    const again = iconButton("refresh", t("nxnative.recheck"));
    again.disabled = loading || busy;
    again.addEventListener("click", () => void refresh());
    head.append(again);
    section.append(head);

    const shown = visibleChecks(current, logs);
    if (!shown.length) return section;
    const rows = element("div", "plugin-fields");
    for (const check of shown) rows.append(renderCheck(current, check));
    section.append(rows);
    return section;
  }

  function renderCheck(current: NxnativeStatus, check: NxnativeCheck): HTMLElement {
    const busy = current.fixing !== null;
    const log = logs[check.id];
    const row = element("div", "plugin-field nxnative-check");
    row.dataset.nxnativeCheck = check.id;
    const label = element("span", "plugin-field__label", t(`nxnative.check.${check.id}` as MessageKey));
    const detail = checkDetail(check);
    if (detail) label.title = detail;
    row.append(label);

    const folder = check.id === "checkout" && check.state === "ok" ? current.checkout : null;
    const [pillText, pillClass] =
      log?.state === "running"
        ? [t(check.id === "checkout" && check.state === "ok" ? "nxnative.updating" : "nxnative.installing"), "ws-pill nxnative-pill--busy"]
        : log?.state === "failed"
          ? [t("nxnative.state.failed"), "ws-pill nxnative-pill--error"]
          : check.state === "ok" || log?.state === "ok"
            ? [folder ?? t("nxnative.state.ok"), "ws-pill ws-pill--on"]
            : [checkPill(check), "ws-pill"];
    row.append(element("span", pillClass, pillText));

    const actions = element("span", "nxnative-check__actions");
    // Installed in this session: the row stays, with its log, and no button.
    if (check.state !== "ok" && log?.state !== "running" && log?.state !== "ok") {
      const link = CHECK_LINKS[check.id];
      if (link) {
        const open = button(t(link.label));
        open.addEventListener("click", () => void deps.invoke("nxnative_open_link", { link: link.link }));
        actions.append(open);
      }
      if (canFix(check)) {
        // The page names the check; the host runs the command it holds for it.
        const label = check.id === "xcode" ? "nxnative.runSelect" : check.id === "checkout" ? "nxnative.clone" : "nxnative.run";
        const run = button(t(label), "primary");
        run.disabled = busy;
        if (check.fix) run.title = check.fix;
        run.addEventListener("click", () => runFix("nxnative_run_fix", check.id));
        actions.append(run);
      }
    }
    if (check.id === "checkout" && log?.state !== "running") {
      // Always there: nxnative's folder is changed, and brought up to date, from this row.
      if (check.state === "ok") {
        const updateButton = button(t("nxnative.update"));
        updateButton.disabled = busy;
        updateButton.title = t("nxnative.updateTitle");
        updateButton.addEventListener("click", update);
        actions.append(updateButton);
      }
      const change = iconButton("folder", t("nxnative.chooseCheckout"));
      change.disabled = busy;
      change.addEventListener("click", () => {
        void deps
          .invoke<string | null>("nxnative_choose_checkout")
          .then((picked) => {
            if (picked) void refresh();
          })
          .catch((error) => deps.toast(String(error)));
      });
      actions.append(change);
    }
    if (log) {
      const open = openLogs.has(check.id);
      const toggle = iconButton(open ? "chevron-up" : "chevron-down", t("nxnative.logs"));
      toggle.classList.add("nxnative-log-toggle");
      toggle.setAttribute("aria-expanded", String(open));
      toggle.addEventListener("click", () => {
        if (openLogs.has(check.id)) openLogs.delete(check.id);
        else openLogs.add(check.id);
        render();
      });
      actions.append(toggle);
    }
    row.append(actions);

    if (log) {
      const under = element("div", "nxnative-check__under");
      if (log.state === "running") {
        under.append(element("div", "nxnative-indeterminate"));
        if (!openLogs.has(check.id)) under.append(element("p", "nxnative-live", log.lines.at(-1) ?? ""));
      } else if (log.state === "failed" && log.message && !openLogs.has(check.id)) {
        under.append(element("p", "nxnative-live nxnative-live--error", log.message));
      }
      if (openLogs.has(check.id)) {
        const pre = element("pre", "nxnative-log", log.lines.join("\n"));
        pre.setAttribute("role", "log");
        under.append(pre);
      }
      if (under.childElementCount) row.append(under);
    }
    return row;
  }

  /** While a build runs: its game, a bar and one line. After: one line. */
  function renderProgress(): HTMLElement | null {
    if (build.running) {
      const section = element("section", "nxnative-section nxnative-build");
      const parts = [build.stage, build.step].filter(Boolean);
      if (build.etaSeconds !== null && build.etaSeconds > 0) {
        parts.push(t("nxnative.eta", { minutes: Math.max(1, Math.round(build.etaSeconds / 60)) }));
      }
      const head = element("div", "nxnative-head");
      head.append(element("span", "plugin-field__label", build.game || "…"));
      section.append(head);
      const bar = element("progress", "nxnative-progress");
      bar.max = 1;
      if (build.fraction !== null) bar.value = build.fraction;
      section.append(bar);
      const line = element("p", "nxnative-note", parts.join(" · "));
      if (build.lastMessage) line.title = build.lastMessage;
      section.append(line);
      return section;
    }
    const outcome = outcomeLine(build);
    if (!outcome) return null;
    const line = element("p", build.outcome === "finished" ? "nxnative-note nxnative-note--ok" : "nxnative-note nxnative-note--error", outcome);
    const gameId = build.outcome === "finished" ? builtGameId : null;
    if (!gameId) return line;
    const row = element("div", "nxnative-head");
    row.append(line, playButton(gameId));
    return row;
  }

  function renderGames(): HTMLElement {
    const section = element("section", "nxnative-section");
    section.append(element("p", "plugin-detail__section", t("nxnative.games")));
    for (const game of games) {
      const details = element("details", "plugin-more nxnative-game");
      const summary = element("summary", "nxnative-game__summary");
      summary.title = game.titleId;
      summary.append(element("strong", "", game.title), playButton(game.gameId));
      details.append(summary);
      details.append(renderSettings(game));
      section.append(details);
    }
    return section;
  }

  function renderSettings(game: NxnativeGame): HTMLElement {
    const box = element("div", "plugin-fields nxnative-settings");
    if (!game.settings.length) {
      box.append(element("p", "nxnative-note", t("nxnative.noSettings")));
      return box;
    }
    const values = currentValues(game);
    let section: string | null = null;
    for (const setting of drawnSettings(game)) {
      if (!settingVisible(setting, values)) continue;
      const control = settingControl(setting, values[setting.env] ?? "", (value) => void save(game, setting, value));
      if (!control) continue;
      if (setting.section && setting.section !== section) {
        section = setting.section;
        box.append(element("p", "nxnative-settings__section", section));
      }
      const row = element("label", "plugin-field");
      const label = element("span", "plugin-field__label", setting.label ?? setting.env);
      if (setting.note) row.title = setting.note;
      row.append(label, control);
      box.append(row);
    }
    return box;
  }

  async function save(game: NxnativeGame, setting: LauncherSetting, value: string): Promise<void> {
    try {
      await deps.invoke("nxnative_set_choice", { gameId: game.gameId, env: setting.env, value });
      if (value === setting.default) delete game.saved[setting.env];
      else game.saved[setting.env] = value;
      render();
    } catch {
      deps.toast(t("nxnative.saveFailed"));
    }
  }

  function playButton(gameId: string): HTMLButtonElement {
    const play = button(t("nxnative.play"), "primary");
    play.addEventListener("click", (event) => {
      // In a summary, a click would also fold the game's settings open.
      event.preventDefault();
      deps.play(gameId);
    });
    return play;
  }

  // Something installed outside Orivo (Xcode, Homebrew) is seen on return.
  window.addEventListener("focus", () => {
    if (status?.supported && !status.fixing && !loading && !build.running && root.offsetParent !== null) void refresh();
  });

  return {
    open() {
      void refresh();
    },
  };
}

/** The control a setting's kind is drawn with; null for a kind this reader does not know. */
function settingControl(
  setting: LauncherSetting,
  value: string,
  change: (value: string) => void,
): HTMLElement | null {
  switch (setting.kind) {
    case "choice": {
      const select = element("select", "settings-select");
      for (const [optionValue, label] of setting.options) {
        const option = element(
          "option",
          "",
          optionValue === setting.default ? t("nxnative.defaultSuffix", { label }) : label,
        );
        option.value = optionValue;
        select.append(option);
      }
      select.value = value;
      select.addEventListener("change", () => change(select.value));
      return select;
    }
    case "toggle": {
      const input = element("input", "");
      input.type = "checkbox";
      input.checked = value === setting.on;
      input.addEventListener("change", () => change((input.checked ? setting.on : setting.off) ?? ""));
      return input;
    }
    case "range": {
      const wrap = element("span", "nxnative-range");
      const input = element("input", "");
      input.type = "range";
      if (setting.min !== null) input.min = String(setting.min);
      if (setting.max !== null) input.max = String(setting.max);
      if (setting.step !== null) input.step = String(setting.step);
      input.value = value;
      const shown = element("output", "", value);
      input.addEventListener("input", () => (shown.textContent = input.value));
      input.addEventListener("change", () => change(input.value));
      wrap.append(input, shown);
      return wrap;
    }
    default:
      return null;
  }
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
  text = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function button(label: string, kind: "primary" | "quiet" = "quiet"): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.className = kind === "primary" ? "plugin-install-button" : "settings-button settings-button--quiet";
  node.textContent = label;
  return node;
}

function iconButton(name: IconName, label: string): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.className = "settings-button settings-button--quiet nxnative-icon-button";
  node.title = label;
  node.setAttribute("aria-label", label);
  node.innerHTML = icon(name);
  return node;
}
