import { describe, expect, it } from "vitest";
import { avatarSource, cleanName, DEFAULT_AVATAR, loadProfile, saveProfile } from "./profile";

const memory = () => {
  const items = new Map<string, string>();
  return {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
  };
};

describe("player profile", () => {
  it("round-trips a name and a picture", () => {
    const storage = memory();
    saveProfile({ name: "  Spectre  ", avatar: "data:image/jpeg;base64,AAA" }, storage);
    expect(loadProfile(storage)).toEqual({ name: "Spectre", avatar: "data:image/jpeg;base64,AAA" });
  });

  it("falls back to the default picture and refuses anything but an image", () => {
    const storage = memory();
    storage.setItem("orivo.profile", JSON.stringify({ name: 3, avatar: "javascript:alert(1)" }));
    const profile = loadProfile(storage);
    expect(profile).toEqual({ name: "", avatar: "" });
    expect(avatarSource(profile)).toBe(DEFAULT_AVATAR);
  });

  it("keeps a name to one short line", () => {
    expect(cleanName("a\n  b")).toBe("a b");
    expect(cleanName("x".repeat(50))).toHaveLength(32);
  });
});
