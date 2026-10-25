import { describe, expect, it } from "vitest";
import {
  EMPTY_GAMESTREAM_SETTINGS,
  createGameStreamClient,
  gameStreamErrorMessage,
  readGameStreamSettings,
} from "./gamestream-settings";

const liveSignal = (): AbortSignal => new AbortController().signal;

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

  it("refuses to open a page, in the same words", async () => {
    await expect(createGameStreamClient().openLink("moonlight", liveSignal())).rejects.toThrow(
      "desktop app",
    );
  });
});
