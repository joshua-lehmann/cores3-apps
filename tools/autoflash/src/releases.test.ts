import { describe, expect, it, vi } from "vitest";
import { discoverReleases, downloadFirmware, parseReleases, versionsForApp, firmwarePath } from "./releases";

const asset = { name: "timer.bin", state: "uploaded", size: 24, browser_download_url: "https://github.com/joshua-lehmann/cores3-apps/releases/download/v1.2.3/timer.bin" };
const release = { draft: false, prerelease: false, tag_name: "v1.2.3", assets: [asset] };

describe("release discovery", () => {
  it("accepts stable releases with repository-owned uploaded images", () => {
    expect(parseReleases([release, { ...release, prerelease: true }, { ...release, draft: true }])).toEqual([{ version: "v1.2.3", assets: [{ app: "timer", name: "timer.bin", url: asset.browser_download_url, size: 24 }] }]);
    expect(parseReleases([{ ...release, assets: [{ ...asset, browser_download_url: "https://example.com/timer.bin" }, { ...asset, name: "../timer.bin" }] }])).toEqual([]);
  });
  it("offers only versions containing the selected application", () => {
    const found = parseReleases([release]);
    expect(versionsForApp(found, "timer")).toEqual(["v1.2.3"]);
    expect(versionsForApp(found, "simon_says")).toEqual([]);
  });
  it("uses the Pages base path while retaining the local relay", () => {
    const selected = parseReleases([release])[0].assets[0];
    expect(firmwarePath(selected, true, "/cores3-apps/")).toBe("/cores3-apps/firmware/v1.2.3/timer.bin");
    expect(firmwarePath(selected, false, "/")).toBe("/api/firmware/v1.2.3/timer.bin");
  });
  it("surfaces API failures", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 403 }));
    await expect(discoverReleases(fetcher)).rejects.toThrow("403");
  });
  it("downloads the selected image and checks its size and header", async () => {
    const selected = parseReleases([release])[0].assets[0];
    const bytes = new Uint8Array(24); bytes[0] = 0xe9;
    const progress = vi.fn();
    const file = await downloadFirmware(selected, progress, vi.fn().mockResolvedValue(new Response(bytes)));
    expect(file.name).toBe("timer.bin"); expect(progress).toHaveBeenCalledWith(24, 24);
    await expect(downloadFirmware(selected, progress, vi.fn().mockResolvedValue(new Response(new Uint8Array(24))))).rejects.toThrow("not an ESP");
    await expect(downloadFirmware(selected, progress, vi.fn().mockResolvedValue(new Response(new Uint8Array(2))))).rejects.toThrow("incomplete");
  });
});
