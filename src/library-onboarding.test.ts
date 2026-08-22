import { describe, expect, it } from "vitest";
import { CONNECTED_SOURCES } from "./source-model";
import {
  INITIAL_ONBOARDING_VIEW,
  ONBOARDING_SOURCES,
  ONBOARDING_STEPS,
  isOnboardingSource,
  isSameOnboardingView,
  onboardingBack,
  onboardingBackLabel,
  onboardingSignInLine,
  onboardingSourceDescriptor,
} from "./library-onboarding";

describe("onboarding sources", () => {
  it("offers Steam plus every connectable store, in registry order", () => {
    expect(ONBOARDING_SOURCES.map((source) => source.provider)).toEqual([
      "steam",
      ...CONNECTED_SOURCES.map((source) => source.provider),
    ]);
  });

  it("gives every store a label, a mark and a sentence about what it brings", () => {
    // A store added to the registry without copy here would render a blank
    // row in the one screen whose entire job is to explain the choice.
    for (const source of ONBOARDING_SOURCES) {
      expect(source.label).not.toBe("");
      expect(source.icon).not.toBe("");
      expect(source.detail.length).toBeGreaterThan(10);
    }
  });

  it("takes each store's label from the source registry rather than restating it", () => {
    for (const registered of CONNECTED_SOURCES) {
      const descriptor = onboardingSourceDescriptor(registered.provider);
      expect(descriptor.label).toBe(registered.label);
      expect(descriptor.icon).toBe(registered.icon);
    }
  });

  it("only Steam carries its own sign-in sentence; the rest read theirs off their status", () => {
    expect(onboardingSourceDescriptor("steam").signIn).toBeTruthy();
    for (const registered of CONNECTED_SOURCES) {
      expect(onboardingSourceDescriptor(registered.provider).signIn).toBeUndefined();
    }
  });

  it("answers with the first store for an unknown provider rather than throwing", () => {
    expect(onboardingSourceDescriptor("nintendo" as never).provider).toBe("steam");
  });

  it("recognises exactly the stores it offers", () => {
    expect(isOnboardingSource("steam")).toBe(true);
    expect(isOnboardingSource("gog")).toBe(true);
    expect(isOnboardingSource("humble")).toBe(false);
    expect(isOnboardingSource("")).toBe(false);
  });

  it("describes the two sign-in styles as the different experiences they are", () => {
    expect(onboardingSignInLine("session")).not.toBe(onboardingSignInLine("token"));
    expect(onboardingSignInLine("session")).toMatch(/window/i);
    expect(onboardingSignInLine("token")).toMatch(/keychain/i);
  });
});

describe("onboarding steps", () => {
  it("promises three steps and says so in three cards", () => {
    expect(ONBOARDING_STEPS).toHaveLength(3);
    for (const step of ONBOARDING_STEPS) {
      expect(step.title).not.toBe("");
      expect(step.detail).not.toBe("");
    }
  });
});

describe("onboarding navigation", () => {
  it("starts at the choice between a store and a local game", () => {
    expect(INITIAL_ONBOARDING_VIEW).toEqual({ step: "choice" });
  });

  it("walks back one level at a time and stops at the root", () => {
    expect(onboardingBack({ step: "connect", provider: "epic" })).toEqual({ step: "sources" });
    expect(onboardingBack({ step: "sources" })).toEqual({ step: "choice" });
    expect(onboardingBack({ step: "choice" })).toEqual({ step: "choice" });
  });

  it("names the destination on the way back out of a store", () => {
    expect(onboardingBackLabel({ step: "connect", provider: "gog" })).toBe("Libraries");
    expect(onboardingBackLabel({ step: "sources" })).toBe("Back");
  });

  it("treats two connect views for different stores as different views", () => {
    expect(
      isSameOnboardingView(
        { step: "connect", provider: "epic" },
        { step: "connect", provider: "epic" },
      ),
    ).toBe(true);
    expect(
      isSameOnboardingView(
        { step: "connect", provider: "epic" },
        { step: "connect", provider: "gog" },
      ),
    ).toBe(false);
    expect(isSameOnboardingView({ step: "sources" }, { step: "choice" })).toBe(false);
  });
});
