import { describe, expect, it } from "vitest";
import type { GameMediaView } from "./contracts";
import {
  backgroundMedia,
  inferMediaRole,
  isBackgroundMedia,
} from "./media-role";

function media(
  id: string,
  title: string,
  extra: Partial<GameMediaView> = {},
): GameMediaView {
  return {
    id,
    kind: "wallpaper",
    title,
    previewUrl: `game-media:${id}.jpg`,
    origin: "downloaded",
    selected: false,
    availableOffline: true,
    ...extra,
  };
}

describe("inferMediaRole", () => {
  it("trusts the stored role over everything else", () => {
    expect(
      inferMediaRole(media("a", "A Way Out — logo", { role: "background" })),
    ).toBe("background");
    expect(inferMediaRole(media("b", "Anything", { role: "logo" }))).toBe(
      "logo",
    );
  });

  it("reads a legacy record from the end of its title", () => {
    expect(inferMediaRole(media("a", "A Way Out — logo"))).toBe("logo");
    expect(inferMediaRole(media("a", "A Way Out — library logo"))).toBe("logo");
    expect(inferMediaRole(media("a", "A Way Out — cover"))).toBe("cover");
    expect(inferMediaRole(media("a", "A Way Out — capsule"))).toBe("landscape");
    expect(inferMediaRole(media("a", "A Way Out — header"))).toBe("landscape");
    expect(inferMediaRole(media("a", "A Way Out — key art (4K)"))).toBe(
      "landscape",
    );
    expect(inferMediaRole(media("a", "A Way Out - logo"))).toBe("logo");
  });

  it("uses the slot a cover-kind record already stands for", () => {
    expect(inferMediaRole(media("a", "Untitled", { kind: "cover" }))).toBe(
      "cover",
    );
  });

  it("says nothing for a title that names no role", () => {
    expect(inferMediaRole(media("a", "A Way Out — background (4K)"))).toBeNull();
    expect(inferMediaRole(media("a", "A Way Out — artwork 2"))).toBeNull();
    expect(inferMediaRole(media("a", "Wallpaper"))).toBeNull();
    // A hyphen inside the game's own name is not a suffix.
    expect(inferMediaRole(media("a", "Cover-Up Heroes wallpaper"))).toBeNull();
    expect(inferMediaRole(media("a", "Logo Quest"))).toBeNull();
  });
});

describe("backgroundMedia", () => {
  it("keeps backgrounds and unknowns, and drops logos, covers and capsules", () => {
    const all = [
      media("bg", "A Way Out — background (4K)"),
      media("plain", "Wallpaper"),
      media("logo", "A Way Out — logo"),
      media("cover", "A Way Out — cover"),
      media("capsule", "A Way Out — capsule"),
      media("stored-logo", "Odd title", { role: "logo" }),
      media("stored-bg", "A Way Out — logo", { role: "background" }),
      media("video", "Trailer", { kind: "video" }),
    ];
    expect(backgroundMedia(all).map((item) => item.id)).toEqual([
      "bg",
      "plain",
      "stored-bg",
    ]);
    expect(isBackgroundMedia(all[2])).toBe(false);
  });
});
