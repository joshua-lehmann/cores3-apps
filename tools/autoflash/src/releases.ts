/** Public, versioned merged firmware assets published by this repository. */
const API = "https://api.github.com/repos/joshua-lehmann/cores3-apps/releases";
const MAX_BYTES = 16 * 1024 * 1024;

export interface FirmwareAsset { app: string; name: string; url: string; size: number; digest?: string }
export interface FirmwareRelease { version: string; assets: FirmwareAsset[] }

/** Accept only release assets belonging to this repository with the agreed filename. */
export function parseReleases(value: unknown): FirmwareRelease[] {
  if (!Array.isArray(value)) throw new Error("GitHub returned an invalid release list.");
  return value.flatMap((item): FirmwareRelease[] => {
    if (!item || typeof item !== "object" || item.draft !== false || item.prerelease !== false || typeof item.tag_name !== "string" || !/^v\d+\.\d+\.\d+$/.test(item.tag_name) || !Array.isArray(item.assets)) return [];
    const assets: FirmwareAsset[] = [];
    for (const asset of item.assets) {
      if (!asset || typeof asset.name !== "string" || !/^[a-z][a-z0-9_]*\.bin$/.test(asset.name) || typeof asset.browser_download_url !== "string" || !Number.isSafeInteger(asset.size) || asset.size <= 0 || asset.size > MAX_BYTES || asset.state !== "uploaded") continue;
      try {
        const url = new URL(asset.browser_download_url);
        if (url.origin !== "https://github.com" || url.pathname !== `/joshua-lehmann/cores3-apps/releases/download/${encodeURIComponent(item.tag_name)}/${asset.name}`) continue;
        assets.push({ app: asset.name.slice(0, -4), name: asset.name, url: url.href, size: asset.size, ...(typeof asset.digest === "string" && /^sha256:[a-f0-9]{64}$/.test(asset.digest) ? { digest: asset.digest } : {}) });
      } catch { /* Ignore malformed assets. */ }
    }
    return assets.length ? [{ version: item.tag_name, assets }] : [];
  });
}

/** Fetch all release pages, keeping the API's newest-first ordering. */
export async function discoverReleases(fetcher: typeof fetch = fetch): Promise<FirmwareRelease[]> {
  const releases: FirmwareRelease[] = [];
  for (let page = 1; page <= 100; page++) {
    const response = await fetcher(`${API}?per_page=100&page=${page}`, { headers: { Accept: "application/vnd.github+json" } });
    if (!response.ok) throw new Error(`GitHub releases request failed (${response.status}).`);
    const raw: unknown = await response.json();
    if (!Array.isArray(raw)) throw new Error("GitHub returned an invalid release list.");
    releases.push(...parseReleases(raw));
    if (raw.length < 100) return releases.sort((a, b) => {
      const left = a.version.slice(1).split(".").map(Number);
      const right = b.version.slice(1).split(".").map(Number);
      return right[0] - left[0] || right[1] - left[1] || right[2] - left[2];
    });
  }
  throw new Error("Too many GitHub release pages.");
}

/** Versions containing a given application, in discovery order. */
export function versionsForApp(releases: FirmwareRelease[], app: string): string[] {
  return releases.filter((release) => release.assets.some((asset) => asset.app === app)).map((release) => release.version);
}

/** Select the Pages mirror or the local server's release relay. */
export function firmwarePath(asset: FirmwareAsset, pages: boolean, base: string): string {
  const path = new URL(asset.url).pathname.split("/");
  const version = path[path.length - 2];
  return pages ? `${base}firmware/${version}/${asset.name}` : `/api/firmware/${version}/${asset.name}`;
}

/** Download a selected asset, reporting byte progress, and validate its ESP image header. */
export async function downloadFirmware(asset: FirmwareAsset, progress: (received: number, total: number) => void, fetcher: typeof fetch = fetch): Promise<File> {
  const response = await fetcher(firmwarePath(asset, import.meta.env.VITE_GITHUB_PAGES === "true", import.meta.env.BASE_URL));
  if (!response.ok || !response.body) throw new Error(`Firmware download failed (${response.status}). A new release may not have reached the Pages mirror yet.`);
  const reader = response.body.getReader();
  const chunks: Uint8Array<ArrayBuffer>[] = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    received += value.length;
    if (received > asset.size || received > MAX_BYTES) { await reader.cancel(); throw new Error("Firmware download exceeds the expected size."); }
    chunks.push(value); progress(received, asset.size);
  }
  if (received !== asset.size) throw new Error("Firmware download is incomplete.");
  const file = new File(chunks, asset.name, { type: "application/octet-stream" });
  if (asset.digest) {
    const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", await file.arrayBuffer()));
    const actual = [...hash].map((byte) => byte.toString(16).padStart(2, "0")).join("");
    if (`sha256:${actual}` !== asset.digest) throw new Error("Firmware does not match the GitHub release digest. The Pages mirror may be updating.");
  }
  const header = new Uint8Array(await file.slice(0, 24).arrayBuffer());
  if (header.length < 24 || header[0] !== 0xe9) throw new Error("Downloaded asset is not an ESP firmware image.");
  return file;
}
