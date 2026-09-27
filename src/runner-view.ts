/* ---------------------------------------------------------------------------
   The "Third-party runners" panel inside Settings › Plugins.

   Owns one subtree end to end — markup, listeners and re-rendering on the
   controller's own change events — so `app.ts` only has to mount it once and
   toggle the panel's `hidden` attribute like it already does for the Wine and
   Wallpaper Searcher panels. Nothing here ever sees a path: an application and
   a folder are chosen by a native picker Rust opens, and what comes back is an
   id and a label (`runner-manager.ts`, `runner_commands.rs`).
   --------------------------------------------------------------------------- */

import {
  isRunnerImportRunning,
  runnerErrorMessage,
  runnerImportSummary,
  runnerProfileStatusLabel,
  type InstalledRunnerView,
  type RunnerManagerController,
  type RunnerProfileView,
} from "./runner-manager";

export interface RunnerPanelHandlers {
  showToast(message: string): void;
}

function pluginStateLabel(runner: InstalledRunnerView): string {
  if (runner.state === "ready") return "";
  return runner.state === "incompatible" ? "Incompatible" : "Invalid";
}

function iconButton(label: string, kind: "icon" | "text" = "text"): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = kind === "icon" ? "settings-button settings-button--quiet" : "settings-button";
  button.textContent = label;
  return button;
}

export function mountRunnerPanel(
  root: HTMLElement,
  controller: RunnerManagerController,
  handlers: RunnerPanelHandlers,
): { render: () => void } {
  const busy = new Set<string>();

  const withBusy = async (key: string, run: () => Promise<unknown>): Promise<void> => {
    if (busy.has(key)) return;
    busy.add(key);
    render();
    try {
      await run();
    } catch (error) {
      handlers.showToast(runnerErrorMessage(error));
    } finally {
      busy.delete(key);
      render();
    }
  };

  function renderDirectory(profile: RunnerProfileView, directory: RunnerProfileView["directories"][number]): HTMLElement {
    const row = document.createElement("div");
    row.className = "runner-directory-row";
    const label = document.createElement("span");
    label.textContent = directory.label;
    if (!directory.granted) {
      const revoked = document.createElement("small");
      revoked.className = "runner-directory-row__revoked";
      revoked.textContent = "Access removed";
      row.append(label, revoked);
    } else {
      row.append(label);
    }
    if (directory.granted) {
      const revoke = iconButton("Remove access", "icon");
      revoke.disabled = busy.has(`revoke:${profile.id}:${directory.id}`);
      revoke.addEventListener("click", () => {
        void withBusy(`revoke:${profile.id}:${directory.id}`, () =>
          controller.revokeDirectory(profile.id, directory.id),
        );
      });
      row.append(revoke);
    }
    return row;
  }

  function renderProfile(profile: RunnerProfileView): HTMLElement {
    const card = document.createElement("div");
    card.className = "runner-profile-card";
    card.dataset.runnerProfile = profile.id;

    const header = document.createElement("div");
    header.className = "runner-profile-card__header";
    const name = document.createElement("strong");
    name.textContent = profile.displayName;
    const status = document.createElement("span");
    status.className = "runner-profile-card__status";
    if (profile.status === "rejected") status.classList.add("runner-profile-card__status--error");
    if (profile.status === "unvalidated") status.classList.add("runner-profile-card__status--pending");
    status.textContent = runnerProfileStatusLabel(profile);
    header.append(name, status);

    const application = document.createElement("small");
    application.textContent = profile.applicationLabel;

    const toggleLabel = document.createElement("label");
    toggleLabel.className = "runner-profile-card__toggle";
    const toggle = document.createElement("input");
    toggle.type = "checkbox";
    toggle.checked = profile.enabled;
    toggle.disabled = busy.has(`enable:${profile.id}`);
    toggle.addEventListener("change", () => {
      void withBusy(`enable:${profile.id}`, () => controller.setEnabled(profile.id, toggle.checked));
    });
    const toggleText = document.createElement("span");
    toggleText.textContent = "Enabled";
    toggleLabel.append(toggle, toggleText);

    const directories = document.createElement("div");
    directories.className = "runner-directory-list";
    for (const directory of profile.directories) directories.append(renderDirectory(profile, directory));

    const addFolder = iconButton("Add a game folder…");
    addFolder.disabled = busy.has(`grant:${profile.id}`);
    addFolder.addEventListener("click", () => {
      void withBusy(`grant:${profile.id}`, () => controller.grantDirectory(profile.id));
    });

    const importSection = document.createElement("div");
    importSection.className = "runner-import";
    const job = controller.importFor(profile.id);
    const running = isRunnerImportRunning(job);
    const summary = document.createElement("small");
    const grantedFolders = profile.directories.some((directory) => directory.granted);
    summary.textContent =
      runnerImportSummary(job) ||
      (profile.gameCount > 0
        ? `${profile.gameCount} game(s) imported.`
        : grantedFolders
          ? "No games imported yet."
          : "Add a game folder before importing.");
    const importButton = iconButton(
      running ? "Cancel import" : profile.importResumable ? "Continue import" : "Import games",
    );
    importButton.disabled = !running && (!grantedFolders || busy.has(`import:${profile.id}`));
    importButton.addEventListener("click", () => {
      if (running) {
        void controller.cancelImport(profile.id).then(render);
        return;
      }
      void withBusy(`import:${profile.id}`, () => controller.startImport(profile.id));
    });
    importSection.append(summary, importButton);
    if (running) {
      const bar = document.createElement("div");
      bar.className = "plugin-progress";
      const fill = document.createElement("div");
      fill.className = "plugin-progress__bar";
      fill.style.width = job && job.complete ? "100%" : "60%";
      bar.append(fill);
      importSection.append(bar);
    }

    const remove = iconButton("Remove profile", "icon");
    remove.disabled = busy.has(`delete:${profile.id}`);
    remove.addEventListener("click", () => {
      void withBusy(`delete:${profile.id}`, () => controller.deleteProfile(profile.id));
    });

    card.append(header, application, toggleLabel, directories, addFolder, importSection, remove);
    return card;
  }

  function renderRunnerGroup(runner: InstalledRunnerView): HTMLElement {
    const group = document.createElement("section");
    group.className = "runner-group";
    group.dataset.runnerPlugin = runner.id;

    const header = document.createElement("div");
    header.className = "runner-group__header";
    const name = document.createElement("strong");
    name.textContent = runner.name;
    header.append(name);
    const state = pluginStateLabel(runner);
    if (state) {
      const badge = document.createElement("span");
      badge.className = "runner-group__state";
      badge.textContent = runner.message ? `${state}: ${runner.message}` : state;
      header.append(badge);
    }

    const addProfile = iconButton("+ Add an emulator");
    addProfile.disabled = runner.state !== "ready" || busy.has(`create:${runner.id}`);
    addProfile.addEventListener("click", () => {
      void withBusy(`create:${runner.id}`, () => controller.createProfile(runner.id, runner.name));
    });

    const profiles = document.createElement("div");
    profiles.className = "runner-profile-list";
    for (const profile of runner.profiles) profiles.append(renderProfile(profile));

    group.append(header, addProfile, profiles);
    return group;
  }

  function render(): void {
    const runners = controller.runners();
    root.replaceChildren();
    if (runners.length === 0) {
      const empty = document.createElement("p");
      empty.className = "settings-hint";
      empty.textContent = "Install a runner plugin to add an emulator here.";
      root.append(empty);
      return;
    }
    for (const runner of runners) root.append(renderRunnerGroup(runner));
  }

  controller.onChange(render);
  render();
  return { render };
}
