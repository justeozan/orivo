import { beforeEach, describe, expect, it, vi } from "vitest";

const hoisted = vi.hoisted(() => ({
  tauriInvoke: vi.fn(),
  reportFailure: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: hoisted.tauriInvoke }));
vi.mock("./sentry", () => ({ reportFailure: hoisted.reportFailure }));

import { invoke } from "./host";

beforeEach(() => {
  hoisted.tauriInvoke.mockReset();
  hoisted.reportFailure.mockReset();
});

describe("invoke", () => {
  it("passes the call to Tauri exactly as it was made", async () => {
    hoisted.tauriInvoke.mockResolvedValue({ games: 3 });

    await expect(invoke("get_library", { folder: "a" })).resolves.toEqual({ games: 3 });
    expect(hoisted.tauriInvoke).toHaveBeenCalledWith("get_library", { folder: "a" });

    await invoke("get_version");
    // No stray `undefined` arguments: a stand-in that checks arity would see them.
    expect(hoisted.tauriInvoke).toHaveBeenLastCalledWith("get_version");
    expect(hoisted.reportFailure).not.toHaveBeenCalled();
  });

  it("reports a rejection with the command's name and gives the caller the same one", async () => {
    hoisted.tauriInvoke.mockRejectedValue("The game folder is gone");

    await expect(invoke("launch_game", { id: "x" })).rejects.toBe("The game folder is gone");
    expect(hoisted.reportFailure).toHaveBeenCalledWith("launch_game", "The game folder is gone");
  });

  it("accepts a host that answers with a plain value", async () => {
    hoisted.tauriInvoke.mockReturnValue("ready");

    await expect(invoke("get_state")).resolves.toBe("ready");
  });
});
