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
  GAMESTREAM_PLUGIN_ID,
  isRunnerImportRunning,
  runnerErrorMessage,
  runnerImportSummary,
  runnerProfileStatusLabel,
  type InstalledRunnerView,
  type RunnerManagerController,
  type RunnerProfileView,
} from "./runner-manager";
import { t } from "./i18n";

export interface RunnerPanelHandlers {
  showToast(message: string): void;
}

function pluginStateLabel(runner: InstalledRunnerView): string {
  if (runner.state === "ready") return "";
  return runner.state === "incompatible" ? t("runners.view.incompatible") : t("runners.view.invalid");
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
      revoked.textContent = t("runners.view.accessRemoved");
      row.append(label, revoked);
    } else {
      row.append(label);
    }
    if (directory.granted) {
      const revoke = iconButton(t("runners.view.removeAccess"), "icon");
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
    toggleText.textContent = t("runners.view.enabled");
    toggleLabel.append(toggle, toggleText);

    // What this profile is allowed to launch. It is a permission over the
    // user's own machine, so it is theirs to set and the plugin is never
    // asked: a profile that may only start game files cannot be talked into
    // starting a stream, whichever way the plugin's answer leans.
    const modeLabel = document.createElement("label");
    modeLabel.className = "runner-profile-card__mode";
    const modeText = document.createElement("span");
    modeText.textContent = t("runners.view.launches");
    const mode = document.createElement("select");
    mode.className = "settings-select";
    mode.dataset.runnerLaunchMode = profile.id;
    for (const [value, label] of [
      ["default", t("runners.view.modeDefault")],
      ["stream", t("runners.view.modeStream")],
    ] as const) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      mode.append(option);
    }
    mode.value = profile.launchMode;
    mode.disabled = busy.has(`mode:${profile.id}`);
    mode.addEventListener("change", () => {
      const chosen = mode.value === "stream" ? "stream" : "default";
      void withBusy(`mode:${profile.id}`, () => controller.setLaunchMode(profile.id, chosen));
    });
    modeLabel.append(modeText, mode);
    // Streaming has its own setup now (Settings › Plugins › Game streaming), and
    // an emulator never streams, so the choice is only offered to a profile left
    // in stream mode by an older Orivo — to let it come back.
    modeLabel.hidden = profile.launchMode !== "stream";

    const directories = document.createElement("div");
    directories.className = "runner-directory-list";
    for (const directory of profile.directories) directories.append(renderDirectory(profile, directory));

    const addFolder = iconButton(t("runners.view.addFolder"));
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
        ? t("runners.view.imported", { count: profile.gameCount })
        : grantedFolders
          ? t("runners.view.noneYet")
          : t("runners.view.addFolderFirst"));
    // Artwork is looked for after the games are in, and it is slow: a request
    // and up to four downloads per card. Saying so is the difference between
    // "still working" and "nothing happened".
    const findingArtwork = controller.isFindingArtwork(profile.id);
    if (findingArtwork) {
      summary.textContent = t("runners.view.findingArtwork", { count: profile.gameCount });
    }
    const importButton = iconButton(
      running
        ? t("runners.view.cancelImport")
        : profile.importResumable
          ? t("runners.view.continueImport")
          : t("runners.view.importGames"),
    );
    importButton.disabled =
      !running && (findingArtwork || !grantedFolders || busy.has(`import:${profile.id}`));
    importButton.addEventListener("click", () => {
      if (running) {
        void controller.cancelImport(profile.id).then(render);
        return;
      }
      void withBusy(`import:${profile.id}`, () => controller.startImport(profile.id));
    });
    importSection.append(summary, importButton);
    if (running || findingArtwork) {
      const bar = document.createElement("div");
      bar.className = "plugin-progress";
      const fill = document.createElement("div");
      fill.className = "plugin-progress__bar";
      fill.style.width = job && job.complete ? "100%" : "60%";
      bar.append(fill);
      importSection.append(bar);
    }

    const remove = iconButton(t("runners.view.removeProfile"), "icon");
    remove.disabled = busy.has(`delete:${profile.id}`);
    remove.addEventListener("click", () => {
      void withBusy(`delete:${profile.id}`, () => controller.deleteProfile(profile.id));
    });

    card.append(
      header,
      application,
      toggleLabel,
      modeLabel,
      directories,
      addFolder,
      importSection,
      remove,
    );
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

    const adders = document.createElement("div");
    adders.className = "runner-group__adders";
    const addProfile = iconButton(t("runners.view.addEmulator"));
    addProfile.disabled = runner.state !== "ready" || busy.has(`create:${runner.id}`);
    addProfile.addEventListener("click", () => {
      void withBusy(`create:${runner.id}`, () => controller.createProfile(runner.id, runner.name));
    });
    adders.append(addProfile);

    const profiles = document.createElement("div");
    profiles.className = "runner-profile-list";
    for (const profile of runner.profiles) profiles.append(renderProfile(profile));

    group.append(header, adders, profiles);
    return group;
  }

  function render(): void {
    // Game streaming is set up in its own place (`gamestream-setup.ts`), where
    // the steps run in order. Listing its profile here too would be a second,
    // worse way in: an emulator's folder picker for a PC's game list.
    const runners = controller
      .runners()
      .filter((runner) => runner.id !== GAMESTREAM_PLUGIN_ID);
    root.replaceChildren();
    if (runners.length === 0) {
      const empty = document.createElement("p");
      empty.className = "settings-hint";
      empty.textContent = t("runners.view.empty");
      root.append(empty);
      return;
    }
    for (const runner of runners) root.append(renderRunnerGroup(runner));
  }

  controller.onChange(render);
  render();
  return { render };
}
