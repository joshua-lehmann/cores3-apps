/** Message used only after the complete flash operation has succeeded. */
export const MANUAL_REBOOT = "Firmware flashed successfully. Disconnect and reconnect USB-C to start the new firmware.";

/** Finish the loader transaction before reset/close, then wait before opening the monitor. */
export async function finishFlash(write: () => Promise<void>, reset: () => Promise<void>, close: () => Promise<void>, monitor: () => Promise<void>, delay: (ms: number) => Promise<void>): Promise<boolean> {
  let resetAccepted = true;
  try {
    await write();
    try { await reset(); } catch { resetAccepted = false; }
  } finally {
    try { await close(); } catch { resetAccepted = false; }
  }
  await delay(750);
  try { await monitor(); } catch { resetAccepted = false; }
  return resetAccepted;
}

/** ROM download-mode text may arrive in multiple serial chunks. */
export function isDownloadMode(text: string): boolean {
  return /waiting for download|DOWNLOAD(?:\([^)]*\))?_BOOT|boot:0x[0-9a-f]+\s*\(DOWNLOAD/i.test(text);
}
