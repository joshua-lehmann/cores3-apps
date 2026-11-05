# Autoflash

Autoflash connects CoreS3 devices through Web Serial, flashes merged ESP images at `0x0`, and displays serial logs. It supports published GitHub releases and a watched local file.

## Flash a release

1. Open [Autoflash on GitHub Pages](https://joshua-lehmann.github.io/cores3-apps/) in desktop Chrome or Edge.
2. Connect the CoreS3 over USB, click **Authorize device**, and select it.
3. Click **Load releases**, choose an application and stable version, and click **Download and flash release**.
4. Keep the tab open until flashing completes and the serial monitor reconnects.

No local server or toolchain is needed. Web Serial requires HTTPS or localhost; Firefox, Safari, and mobile browsers do not support this workflow. GitHub API rate limits or network errors appear in the page. The release list can be empty if no stable release contains compatible `.bin` assets.

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
