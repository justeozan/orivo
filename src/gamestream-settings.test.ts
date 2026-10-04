import { beforeEach, describe, expect, it } from "vitest";
import {
  EMPTY_GAMESTREAM_SETTINGS,
  createGameStreamClient,
  gameStreamErrorMessage,
  mountGameStreamPanel,
  readGameStreamSettings,
  type GameStreamClient,
  type GameStreamSettingsUpdate,
  type GameStreamSettingsView,
} from "./gamestream-settings";

const liveSignal = (): AbortSignal => new AbortController().signal;
const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

interface RecordedClient {
  client: GameStreamClient;
  saved: GameStreamSettingsUpdate[];
}

function createFakeClient(
  stored: GameStreamSettingsView = { host: "astra.local" },
  onSave?: (update: GameStreamSettingsUpdate) => Promise<GameStreamSettingsView>,
): RecordedClient {
  const saved: GameStreamSettingsUpdate[] = [];
  return {
    saved,
    client: {
      async load() {
        return stored;
      },
      async save(update) {
        saved.push(update);
        if (onSave) return onSave(update);
        return { host: update.host ?? stored.host };
      },
    },
  };
}

describe("readGameStreamSettings", () => {
  it("reads the address and nothing else", () => {
    expect(readGameStreamSettings({ host: "astra.local:47990", somethingElse: true })).toEqual({
      host: "astra.local:47990",
    });
  });

  it("degrades anything that is not that document to empty", () => {
    expect(readGameStreamSettings(null)).toEqual(EMPTY_GAMESTREAM_SETTINGS);
    expect(readGameStreamSettings("astra.local")).toEqual(EMPTY_GAMESTREAM_SETTINGS);
    expect(readGameStreamSettings({ host: 47990 })).toEqual(EMPTY_GAMESTREAM_SETTINGS);
  });

  // The listing is authorised by the client's pairing, so there is no
  // credential in this feature. A host that answered with one is answering
  // about something else, and none of it is carried.
  it("carries no credential, whatever the host answered with", () => {
    const read = readGameStreamSettings({
      host: "astra.local",
      username: "admin",
      password: "pw",
      passwordSet: true,
    });
    expect(read).toEqual({ host: "astra.local" });
    expect(JSON.stringify(read)).not.toContain("pw");
  });
});

describe("gameStreamErrorMessage", () => {
  it("prefers the host's own sentence", () => {
    expect(gameStreamErrorMessage("That address is not one Orivo can use.")).toBe(
      "That address is not one Orivo can use.",
    );
    expect(gameStreamErrorMessage(new Error("Nope"))).toBe("Nope");
    expect(gameStreamErrorMessage({})).toBe("Orivo could not save that. Try again.");
  });
});

describe("the default client outside the desktop shell", () => {
  it("reads as unconfigured instead of failing", async () => {
    await expect(createGameStreamClient().load(liveSignal())).resolves.toEqual(
      EMPTY_GAMESTREAM_SETTINGS,
    );
  });

  it("refuses to save, because there is nowhere to save it", async () => {
    await expect(
      createGameStreamClient().save({ host: "astra.local" }, liveSignal()),
    ).rejects.toThrow("desktop app");
  });
});

describe("mountGameStreamPanel", () => {
  let root: HTMLElement;
  let toasts: string[];

  beforeEach(() => {
    root = document.createElement("div");
    toasts = [];
  });

  const mount = (fake: RecordedClient) =>
    mountGameStreamPanel(root, fake.client, { showToast: (message) => toasts.push(message) });

  it("renders one empty field before anything is loaded", () => {
    mount(createFakeClient());
    expect(root.querySelector<HTMLInputElement>("#gamestream-host")!.value).toBe("");
    expect(root.querySelector<HTMLElement>("#gamestream-status")!.textContent).toBe("No host yet.");
  });

  // There is nothing else on this card, and a box asking for a password would
  // be asking for something the feature does not use.
  it("asks for an address and never for a credential", () => {
    mount(createFakeClient());
    expect(root.querySelectorAll("input")).toHaveLength(1);
    expect(root.querySelectorAll("input[type='password']")).toHaveLength(0);
    // No box asks for one. The card does mention that none is needed, which is
    // worth saying to anyone who set this up when one was.
    expect([...root.querySelectorAll("label")].map((label) => label.textContent)).toEqual([
      "Host address",
    ]);
  });

  it("fills the field from the host on load", async () => {
    const panel = mount(createFakeClient());
    await panel.load(liveSignal());

    expect(root.querySelector<HTMLInputElement>("#gamestream-host")!.value).toBe("astra.local");
    expect(root.querySelector<HTMLElement>("#gamestream-status")!.textContent).toContain("Saved");
  });

  it("sends the address as it stands and confirms what came back", async () => {
    const fake = createFakeClient(EMPTY_GAMESTREAM_SETTINGS);
    const panel = mount(fake);
    await panel.load(liveSignal());

    root.querySelector<HTMLInputElement>("#gamestream-host")!.value = "astra.local";
    root.querySelector<HTMLButtonElement>("#gamestream-save")!.click();
    await flush();

    expect(fake.saved).toEqual([{ host: "astra.local" }]);
    expect(toasts).toEqual(["Game streaming host saved."]);
    expect(root.querySelector<HTMLElement>("#gamestream-status")!.textContent).toContain("Saved");
  });

  it("says so when the address is cleared rather than claiming a host was saved", async () => {
    const fake = createFakeClient({ host: "" });
    const panel = mount(fake);
    await panel.load(liveSignal());

    root.querySelector<HTMLButtonElement>("#gamestream-save")!.click();
    await flush();

    expect(toasts).toEqual(["Game streaming host cleared."]);
  });

  // The host validates the address where the user typed it. Its refusal has to
  // land beside the box, not only in a toast that is gone by the time they look
  // back at the field.
  it("shows the host's refusal beside the field and keeps what was typed", async () => {
    const fake = createFakeClient(EMPTY_GAMESTREAM_SETTINGS, async () => {
      throw "That host address is not one Orivo can use.";
    });
    const panel = mount(fake);
    await panel.load(liveSignal());

    root.querySelector<HTMLInputElement>("#gamestream-host")!.value = "a host with spaces";
    root.querySelector<HTMLButtonElement>("#gamestream-save")!.click();
    await flush();

    expect(root.querySelector<HTMLElement>("#gamestream-status")!.textContent).toBe(
      "That host address is not one Orivo can use.",
    );
    expect(root.querySelector<HTMLInputElement>("#gamestream-host")!.value).toBe(
      "a host with spaces",
    );
    expect(toasts).toEqual(["That host address is not one Orivo can use."]);
    expect(root.querySelector<HTMLButtonElement>("#gamestream-save")!.disabled).toBe(false);
  });

  it("does not let a reload overwrite a save the user is in the middle of", async () => {
    let release: (settings: GameStreamSettingsView) => void = () => {};
    const fake = createFakeClient(
      { host: "old.local" },
      () => new Promise<GameStreamSettingsView>((resolve) => (release = resolve)),
    );
    const panel = mount(fake);
    await panel.load(liveSignal());

    root.querySelector<HTMLInputElement>("#gamestream-host")!.value = "new.local";
    root.querySelector<HTMLButtonElement>("#gamestream-save")!.click();
    await panel.load(liveSignal());

    expect(root.querySelector<HTMLInputElement>("#gamestream-host")!.value).toBe("new.local");

    release({ host: "new.local" });
    await flush();
    expect(root.querySelector<HTMLInputElement>("#gamestream-host")!.value).toBe("new.local");
  });
});
