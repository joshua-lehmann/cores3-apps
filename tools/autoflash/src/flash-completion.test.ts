import { expect, it, vi } from "vitest";
import { finishFlash, isDownloadMode } from "./flash-completion";

it("does not reset or close before writeFlash settles", async () => {
  let complete!: () => void;
  const events: string[] = [];
  const task = finishFlash(() => new Promise<void>((resolve) => { complete = resolve; }), async () => { events.push("reset"); }, async () => { events.push("close"); }, async () => { events.push("monitor"); }, async () => { events.push("delay"); });
  await Promise.resolve(); expect(events).toEqual([]);
  complete(); expect(await task).toBe(true);
  expect(events).toEqual(["reset", "close", "delay", "monitor"]);
});
it("does not report a partial or failed write as successful", async () => {
  const reset = vi.fn(); const close = vi.fn(); const monitor = vi.fn();
  await expect(finishFlash(async () => { throw new Error("Last block failed"); }, reset, close, monitor, vi.fn())).rejects.toThrow("Last block failed");
  expect(close).toHaveBeenCalledOnce(); expect(reset).not.toHaveBeenCalled(); expect(monitor).not.toHaveBeenCalled();
});
it("preserves successful flashing when reset or monitor reconnect fails", async () => {
  const monitor = vi.fn(); const close = vi.fn();
  expect(await finishFlash(vi.fn(), async () => { throw new Error("USB reset disconnected"); }, close, monitor, vi.fn())).toBe(false);
  expect(close).toHaveBeenCalledOnce(); expect(monitor).toHaveBeenCalledOnce();
  expect(await finishFlash(vi.fn(), vi.fn(), vi.fn(), async () => { throw new Error("USB absent"); }, vi.fn())).toBe(false);
});
it("recognizes the ROM staying in download mode", () => {
  expect(isDownloadMode("waiting for download")).toBe(true);
  expect(isDownloadMode("boot:0x0 (DOWNLOAD(USB/UART0))")).toBe(true);
  expect(isDownloadMode("SPI_FAST_FLASH_BOOT\nCoreS3 started")).toBe(false);
});
