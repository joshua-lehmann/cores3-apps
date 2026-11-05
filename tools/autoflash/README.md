# Autoflash

Autoflash connects CoreS3 devices through Web Serial, flashes merged ESP images at `0x0`, and displays serial logs. It supports published GitHub releases and a watched local file.

## Flash a release

1. Open [Autoflash on GitHub Pages](https://joshua-lehmann.github.io/cores3-apps/) in desktop Chrome/Edge or Android Chrome.
2. Connect the CoreS3 over USB, click **Authorize device**, and select it.
3. Click **Load releases**, choose an application and stable version, and click **Download and flash release**.
4. Keep the tab open until flashing completes and the serial monitor reconnects.

No local server or toolchain is needed. Desktop uses native Web Serial; Android Chrome prefers a WebUSB-backed serial adapter even when it advertises native Web Serial. HTTPS or localhost is required. Firefox, Safari, and iOS are unsupported. GitHub API rate limits or network errors appear in the page. The release list can be empty if no stable release contains compatible `.bin` assets.

The page reads release metadata from GitHub. GitHub Pages serves a same-origin mirror of the published assets because GitHub's release-download endpoint does not permit cross-origin browser downloads. The page checks the download size, ESP image header, and GitHub SHA-256 digest when available. If a new release is listed but its image is unavailable, wait for the Pages deployment and retry. Selecting or discovering a release never flashes it automatically.

## Local development and automatic flashing

From the repository root, build an image and start the local server:

```bash
cargo dist --package timer --bin timer
cd tools/autoflash
cargo run --release
```

Open <http://localhost:8080> in desktop Chrome or Edge, authorize the device, and choose the root-level `firmware.bin`. The **Automatic flashing** switch watches that file; a stable change flashes all connected devices and reopens their serial monitors. **Flash latest now** manually flashes the selected file. The browser stores a file handle and may request permission again after a reload.

Use `cargo dist --package simon_says --bin simon_says` or `cargo dist --package demo --bin demo` for the other main apps. `cargo dist` creates a merged bootloader, partition table, and application image. Do not flash an application-only image at `0x0`.

The local server can also relay selected release assets. File watching remains independent of release selection, and remote firmware is flashed only after the release flash button is pressed. If several devices are connected, a flash targets them in parallel; each has a separate serial log tab. Web Serial cannot distinguish identical devices by serial number, so reconnecting identical devices can reuse each other's tabs.

## Panic backtraces and configuration

The local Rust server matches the firmware's ELF hash to a local build and uses the ESP toolchain's `addr2line` to decode panic addresses. Keep the matching ELF file available and run the server where it can access the build output. GitHub Pages and the Vite/Worker hosts show serial output but cannot perform server-side decoding.

The default USB filter is `303a:*`. Set `ESP_AUTOFLASH_SERIAL_PORT_SEARCH` for the local server to a hexadecimal `vvvv:pppp`, `vvvv:*`, or `*`. See [self-hosting](SELF_HOSTING.md) for bind address, ELF directories, `addr2line`, network access, and troubleshooting.

## Development and tests

Run in `tools/autoflash/`:

```bash
npm ci
npm test
npm run build                 # client and Worker
npm run build:selfhost        # regenerate site/ for the Rust server
cargo +stable test --locked  # local server tests
```

`site/` is checked in because `build.rs` embeds it in the local server; rebuild it after frontend changes. `npm run dev` runs Vite with the default device filter and without server-side backtrace decoding. The GitHub Pages workflow builds the static client and mirrors release assets; the Worker build is an alternative host and also does not decode backtraces.

Third-party licenses for the bundled page are in [THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES/).

## Android USB-C setup and manual test

Android support is implemented but **has not been verified on a physical Pixel 9 Pro and CoreS3**. WebUSB must expose the board's CDC serial interface and allow Chrome to claim it. Use a USB-C **data** cable, enable USB host/OTG if your phone requires it, and close other USB apps. Use an appropriate powered USB hub if the phone cannot reliably power the board.

1. Open the HTTPS Autoflash site in current Android Chrome. Reload after the Pages deployment; the Autoflash log should say `Connection backend: webusb`.
2. Connect the CoreS3. Tap **Authorize device** and select the Espressif USB device. The previously observed CoreS3 ID is `303a:1001`; `303a:*` includes it. The log reports VID/PID, interface numbers/classes and endpoints without serial numbers. Expect CDC control class `2` and data class `a`, plus a separate JTAG interface.
3. Confirm serial output, then select an application and version from GitHub Releases. Tap **Download and flash release** once. Confirm validation, progress to completion, reset, and output from the selected app. Keep the page visible; a screen wake lock is requested where supported but may be refused.
4. Unplug/reconnect and check automatic reattachment. Test **Reset selected**, then flash a different app. A second tap during flashing should not start another flash.
5. Check desktop Chrome/Edge still supports release flashing, multiple devices, serial monitoring, and automatic local reflashing. Android does not require a local file picker or server.

If the chooser is empty, check Chrome's USB site permissions, the cable, USB host mode, and power. Browsers do not distinguish an empty chooser from cancellation; the log provides setup guidance for both. If opening/claiming fails, disconnect the board, close other USB apps and retry. Android or Chrome may reserve an interface; changing the vendor filter cannot bypass that restriction. If automatic bootloader entry fails, use the board's documented download-mode procedure and authorize again if it re-enumerates.

For follow-up, download the **Autoflash** and device logs. Include Android and Chrome versions, cable/hub details, the logged VID/PID and interface summary, whether discovery or claiming failed, and the last flash message. Do not include tokens or device serial numbers.

The adapter pins Google's archived [`web-serial-polyfill` 1.0.15](https://github.com/google/web-serial-polyfill) and retains `esptool-js` 0.6.1. Its CDC streams and control methods feed the existing `EventDrivenTransport` and `ESPLoader`; the adapter adds connection events and stable port wrappers. It does not replace `navigator.serial` globally. USB-UART bridges needing vendor-specific protocols are outside this CDC path. The polyfill's Apache-2.0 license is bundled under `THIRD_PARTY_LICENSES/`.

References: [esptool-js](https://github.com/espressif/esptool-js), [API docs](https://espressif.github.io/esptool-js/docs/), [WebUSB](https://developer.mozilla.org/en-US/docs/Web/API/WebUSB_API), [Web Serial](https://developer.mozilla.org/en-US/docs/Web/API/Web_Serial_API), [Chrome USB permissions](https://support.google.com/chrome/answer/12576972), and [Espressif USB Serial/JTAG](https://docs.espressif.com/projects/esp-idf/en/release-v5.3/esp32s3/api-guides/usb-serial-jtag-console.html).

### After flashing: reboot and serial monitor

Autoflash waits for the complete esptool-js write transaction, then releases DTR, asserts RTS, and uses the loader's hard-reset operation before closing the transport. It waits briefly before reopening the serial monitor. A printed 100% line alone is not treated as completion.

If reset/reconnection fails, the ROM reports DOWNLOAD mode, or no startup output arrives during the short observation window, the page displays: **Firmware flashed successfully. Disconnect and reconnect USB-C to start the new firmware.** A silent application may also produce this conservative notice. A failure during the write itself remains a flash error. On the Pixel, verify both that the new app starts without reconnecting and that serial monitoring resumes; this reset change still needs physical-device validation.
