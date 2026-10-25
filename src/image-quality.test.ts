import { describe, expect, it } from "vitest";
import { imageQualityLabel } from "./image-quality";

describe("imageQualityLabel", () => {
  it("names the tier by whichever edge reaches it", () => {
    expect(imageQualityLabel(3840, 2160, "background")).toBe("4K");
    // A 3840x1240 hero is 4K by width alone.
    expect(imageQualityLabel(3840, 1240, "landscape")).toBe("4K");
    expect(imageQualityLabel(2160, 3840, "cover")).toBe("4K");
    // A portrait cover is read sideways: 1500x2160 is 2160 on its long edge.
    expect(imageQualityLabel(1500, 2160, "cover")).toBe("1440p");
    expect(imageQualityLabel(2560, 1440, "background")).toBe("1440p");
    expect(imageQualityLabel(1920, 1080, "background")).toBe("1080p");
    expect(imageQualityLabel(1920, 620, "landscape")).toBe("1080p");
    expect(imageQualityLabel(1280, 720, "background")).toBe("720p");
  });

  it("falls back to the pixel size below 720p", () => {
    expect(imageQualityLabel(616, 353, "landscape")).toBe("616×353");
    expect(imageQualityLabel(600, 900, "cover")).toBe("600×900");
  });

  it("always shows the pixel size for a logo", () => {
    expect(imageQualityLabel(3840, 1200, "logo")).toBe("3840×1200");
    expect(imageQualityLabel(800, 300, "logo")).toBe("800×300");
  });

  it("shows nothing when the size is unknown rather than inventing one", () => {
    expect(imageQualityLabel(undefined, undefined, "cover")).toBeNull();
    expect(imageQualityLabel(1920, undefined)).toBeNull();
    expect(imageQualityLabel(0, 0, "background")).toBeNull();
    expect(imageQualityLabel(Number.NaN, 100)).toBeNull();
  });
});
