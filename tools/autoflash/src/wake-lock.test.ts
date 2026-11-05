import { afterEach, expect, it, vi } from "vitest";
import { keepScreenAwake } from "./wake-lock";

afterEach(() => vi.unstubAllGlobals());
function environment(request: () => Promise<WakeLockSentinel>) {
  const doc = Object.assign(new EventTarget(), { visibilityState: "visible" });
  vi.stubGlobal("document", doc);
  vi.stubGlobal("navigator", { wakeLock: { request } });
  return doc;
}
it("releases a wake lock and removes its visibility listener at flash completion", async () => {
  const release = vi.fn().mockResolvedValue(undefined);
  const request = vi.fn().mockResolvedValue({ release, released: false });
  const doc = environment(request);
  const stop = keepScreenAwake(vi.fn());
  await Promise.resolve();
  stop();
  doc.dispatchEvent(new Event("visibilitychange"));
  expect(release).toHaveBeenCalledTimes(1);
  expect(request).toHaveBeenCalledTimes(1);
});
it("releases a delayed grant when flashing has already ended", async () => {
  let grant!: (lock: WakeLockSentinel) => void;
  environment(() => new Promise((resolve) => { grant = resolve; }));
  const stop = keepScreenAwake(vi.fn()); stop();
  const release = vi.fn().mockResolvedValue(undefined);
  grant({ release } as unknown as WakeLockSentinel);
  await Promise.resolve();
  expect(release).toHaveBeenCalledOnce();
});
it("reports refusal without failing the flash and retries when the page becomes visible", async () => {
  const report = vi.fn();
  const request = vi.fn().mockRejectedValue(new Error("Denied"));
  const doc = environment(request);
  const stop = keepScreenAwake(report);
  await Promise.resolve(); await Promise.resolve();
  expect(report).toHaveBeenCalledOnce();
  doc.dispatchEvent(new Event("visibilitychange"));
  await Promise.resolve(); await Promise.resolve();
  expect(request).toHaveBeenCalledTimes(2); stop();
});
