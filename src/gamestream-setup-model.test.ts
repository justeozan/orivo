import { describe, expect, it } from "vitest";
import {
  appName,
  flowScreen,
  hostPills,
  streamingCardStatus,
  type FlowInput,
} from "./gamestream-setup-model";

const AT_REST: FlowInput = {
  loading: false,
  stage: "done",
  editing: null,
  pairingPhase: "paired",
  savedHost: "astra.local",
  addingHost: false,
};

describe("flowScreen", () => {
  it("asks one question per step, in order", () => {
    expect(flowScreen({ ...AT_REST, loading: true })).toBe("loading");
    expect(flowScreen({ ...AT_REST, stage: "client", savedHost: "" })).toBe("missing");
    expect(flowScreen({ ...AT_REST, stage: "host", savedHost: "" })).toBe("address");
    expect(flowScreen({ ...AT_REST, stage: "host" })).toBe("pick");
    expect(flowScreen({ ...AT_REST, stage: "pair", pairingPhase: "idle" })).toBe("pair");
    expect(flowScreen({ ...AT_REST, stage: "pair", pairingPhase: "failed" })).toBe("pair");
    expect(flowScreen({ ...AT_REST, stage: "pair", pairingPhase: "waiting" })).toBe("pin");
    expect(flowScreen({ ...AT_REST, stage: "games" })).toBe("host");
    expect(flowScreen(AT_REST)).toBe("host");
  });

  it("opens the address field for Other, over the PC it knows", () => {
    expect(flowScreen({ ...AT_REST, stage: "host", addingHost: true })).toBe("address");
  });

  it("puts a change asked for from the menu before the stage", () => {
    expect(flowScreen({ ...AT_REST, editing: "host" })).toBe("pick");
    expect(flowScreen({ ...AT_REST, editing: "host", addingHost: true })).toBe("address");
    expect(flowScreen({ ...AT_REST, editing: "client" })).toBe("app");
    // Even when the app went missing since: the person asked for this screen.
    expect(flowScreen({ ...AT_REST, stage: "client", editing: "client" })).toBe("app");
  });
});

describe("hostPills", () => {
  it("says whether the PC answers, then how many games it gave", () => {
    expect(hostPills({ checking: false, reachable: true, paired: true, gameCount: 42 })).toEqual([
      { text: "Online", tone: "ok" },
      { text: "42 games", tone: "muted" },
    ]);
    expect(hostPills({ checking: false, reachable: false, paired: false, gameCount: 0 })).toEqual([
      { text: "Offline", tone: "warn" },
    ]);
    expect(hostPills({ checking: true, reachable: false, paired: false, gameCount: 1 })).toEqual([
      { text: "Checking…", tone: "muted" },
      { text: "1 game", tone: "muted" },
    ]);
  });

  it("counts a paired PC as online", () => {
    expect(hostPills({ checking: false, reachable: false, paired: true, gameCount: 0 })[0]).toEqual({
      text: "Online",
      tone: "ok",
    });
  });
});

describe("streamingCardStatus", () => {
  const profile = { applicationLabel: "Moonlight.app", applicationAvailable: true, gameCount: 42 };

  it("is ready with the games it brought, and names the app and the PC", () => {
    expect(streamingCardStatus({ issue: null, profile, savedHost: "ASTRA" })).toEqual({
      status: { text: "Ready · 42 games", tone: "ok" },
      detail: "Moonlight · ASTRA",
    });
  });

  it("wears the warning until the setup is finished", () => {
    expect(streamingCardStatus({ issue: null, profile: null, savedHost: "" })).toEqual({
      status: { text: "To set up", tone: "warn" },
      detail: "",
    });
    expect(
      streamingCardStatus({ issue: null, profile: { ...profile, gameCount: 0 }, savedHost: "ASTRA" }).status,
    ).toEqual({ text: "To set up", tone: "warn" });
  });

  it("says the app is missing when it was uninstalled, and the plugin's own trouble first", () => {
    expect(
      streamingCardStatus({ issue: null, profile: { ...profile, applicationAvailable: false }, savedHost: "" })
        .status,
    ).toEqual({ text: "Moonlight missing", tone: "warn" });
    expect(streamingCardStatus({ issue: "Can't run", profile, savedHost: "" }).status).toEqual({
      text: "Can't run",
      tone: "warn",
    });
  });
});

describe("appName", () => {
  it("drops the .app and falls back on Moonlight", () => {
    expect(appName("Moonlight.app")).toBe("Moonlight");
    expect(appName("Artemis")).toBe("Artemis");
    expect(appName("")).toBe("Moonlight");
  });
});
