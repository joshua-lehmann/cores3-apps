/** Hold a best-effort screen wake lock for one flash, reacquiring after visibility returns. */
export function keepScreenAwake(report: (message: string) => void): () => void {
  let stopped = false;
  let pending = false;
  let lock: WakeLockSentinel | undefined;
  const acquire = async (): Promise<void> => {
    if (stopped || pending || (lock && !lock.released) || document.visibilityState !== "visible" || !navigator.wakeLock) return;
    pending = true;
    try {
      const acquired = await navigator.wakeLock.request("screen");
      if (stopped) await acquired.release();
      else lock = acquired;
    } catch { report("Screen wake lock unavailable; keep the screen on and Autoflash visible while flashing."); }
    finally { pending = false; }
  };
  const visible = (): void => { void acquire(); };
  document.addEventListener("visibilitychange", visible);
  void acquire();
  return () => {
    stopped = true;
    document.removeEventListener("visibilitychange", visible);
    void lock?.release().catch(() => {});
  };
}
