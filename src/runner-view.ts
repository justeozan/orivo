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
  type GameStreamPairing,
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
  /**
   * The PIN of the pairing attempt in flight, per profile. It lives here rather
   * than on the profile view because it is not state the host keeps: the host
   * has a child process waiting on the other machine, and this is the one
   * number the user needs while it waits. It is cleared when the user says the
   * attempt is done, not on a timer — a PIN that vanished while they were
   * walking to the other machine would be the one failure mode worth avoiding.
   */
  const pairing = new Map<string, GameStreamPairing>();

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

    // What this profile is allowed to launch. It is a permission over the
    // user's own machine, so it is theirs to set and the plugin is never
    // asked: a profile that may only start game files cannot be talked into
    // starting a stream, whichever way the plugin's answer leans.
    const modeLabel = document.createElement("label");
    modeLabel.className = "runner-profile-card__mode";
    const modeText = document.createElement("span");
    modeText.textContent = "Launches";
    const mode = document.createElement("select");
    mode.className = "settings-select";
    mode.dataset.runnerLaunchMode = profile.id;
    for (const [value, label] of [
      ["default", "Games in its folders"],
      ["stream", "Streams from another machine"],
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

    const directories = document.createElement("div");
    directories.className = "runner-directory-list";
    for (const directory of profile.directories) directories.append(renderDirectory(profile, directory));

    const streaming = profile.launchMode === "stream";
    const addFolder = iconButton(
      streaming ? "Add a folder for its stream cards…" : "Add a game folder…",
    );
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
          ? streaming
            ? "No games yet. Importing asks the streaming host what it can stream."
            : "No games imported yet."
          : streaming
            ? "Add a folder for its stream cards before importing."
            : "Add a game folder before importing.");
    // Artwork is looked for after the games are in, and it is slow: a request
    // and up to four downloads per card. Saying so is the difference between
    // "still working" and "nothing happened".
    const findingArtwork = controller.isFindingArtwork(profile.id);
    if (findingArtwork) {
      summary.textContent = `${profile.gameCount} game(s) imported. Finding artwork…`;
    }
    const importButton = iconButton(
      running ? "Cancel import" : profile.importResumable ? "Continue import" : "Import games",
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

    // Pairing is Moonlight's handshake with the machine, and it has to happen
    // once before any stream starts. It reads no credential, so it is offered
    // even while the feed credentials are being refused — which is exactly
    // when a user is most likely to be looking for it.
    const pairingSection = document.createElement("div");
    pairingSection.className = "runner-pairing";
    if (streaming) {
      const answer = pairing.get(profile.id);
      if (answer) {
        pairingSection.dataset.runnerPairing = profile.id;
        const done = iconButton("Done", "icon");
        done.addEventListener("click", () => {
          pairing.delete(profile.id);
          render();
        });
        if (answer.state === "alreadyPaired") {
          // Not a PIN, and deliberately not a failure either: this is the state
          // the user was trying to reach. Showing a PIN here would be showing
          // one the other machine can only reject, because a paired client
          // refuses to start a second handshake.
          const copy = document.createElement("small");
          copy.textContent = answer.host
            ? `${answer.host} is already paired with this client. Nothing to do.`
            : "This machine is already paired with this client. Nothing to do.";
          pairingSection.append(copy, done);
        } else {
          const pin = document.createElement("strong");
          pin.className = "runner-pairing__pin";
          pin.textContent = answer.pin;
          const copy = document.createElement("small");
          copy.textContent = answer.host
            ? `Type this PIN into Sunshine or Apollo on ${answer.host}, then come back.`
            : "Type this PIN into Sunshine or Apollo on the other machine.";
          pairingSection.append(pin, copy, done);
        }
      } else {
        const pair = iconButton("Pair with this host…");
        pair.disabled = busy.has(`pair:${profile.id}`);
        pair.addEventListener("click", () => {
          void withBusy(`pair:${profile.id}`, async () => {
            pairing.set(profile.id, await controller.beginPairing(profile.id));
          });
        });
        const hint = document.createElement("small");
        hint.textContent =
          "Needed once per machine, before the first stream. Orivo picks the PIN and you type it on the other side.";
        pairingSection.append(pair, hint);
      }
    }

    const remove = iconButton("Remove profile", "icon");
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
      pairingSection,
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

    // One button per application Orivo already found on this machine, then the
    // picker. The found ones come first because they are the answer in the
    // common case: a user who streams has the client installed, and making them
    // walk a file dialog to a bundle Orivo can see is asking them to do its
    // work. The picker stays, because detection only looks where a normal
    // install puts things.
    const adders = document.createElement("div");
    adders.className = "runner-group__adders";
    for (const client of controller.streamClients()) {
      const quick = iconButton(`+ Add ${client.label}`);
      quick.disabled = runner.state !== "ready" || busy.has(`create:${runner.id}`);
      quick.addEventListener("click", () => {
        void withBusy(`create:${runner.id}`, () =>
          controller.createProfile(runner.id, client.label, client.id),
        );
      });
      adders.append(quick);
    }
    const addProfile = iconButton("+ Add an emulator");
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
