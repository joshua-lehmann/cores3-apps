import { SerialPort as UsbSerialPort } from "web-serial-polyfill";

/** Only the serial service operations used by Autoflash. Port I/O stays in esptool-js. */
export interface SerialBackend {
  kind: "native" | "webusb";
  getPorts(): Promise<SerialPort[]>;
  requestPort(options?: SerialPortRequestOptions): Promise<SerialPort>;
  on(type: "connect" | "disconnect", callback: (port: SerialPort) => void): void;
}

/** Prefer USB on Android even when Chrome advertises an unusable native serial API. */
export function chooseSerialKind(android: boolean, native: boolean, usb: boolean): "native" | "webusb" | undefined {
  if (android && usb) return "webusb";
  if (native) return "native";
  if (usb) return "webusb";
  return undefined;
}

/** Log interface numbers/classes/endpoints, without device serial numbers. */
function describeUsb(device: USBDevice): string {
  const interfaces = device.configurations.flatMap((config) => config.interfaces.flatMap((iface) => iface.alternates.map((alt) =>
    `${iface.interfaceNumber}:class=${alt.interfaceClass.toString(16)} endpoints=${alt.endpoints.map((ep) => `${ep.direction}/${ep.type}`).join(",")}`)));
  return `USB ${device.vendorId.toString(16)}:${device.productId.toString(16)} interfaces=[${interfaces.join("; ")}]`;
}

/** Adapt USB connection events and keep one polyfill port per USBDevice across sweeps. */
export function usbBackend(usb: USB, log: (message: string) => void, wrap: (device: USBDevice) => SerialPort = (device) =>
  // Google supplies streams/open/close/getInfo/setSignals used by esptool-js 0.6.1.
  // It lacks native SerialPort EventTarget/getSignals, which our I/O path does not use.
  new UsbSerialPort(device) as unknown as SerialPort): SerialBackend {
  const ports = new WeakMap<USBDevice, SerialPort>();
  const described = new WeakSet<USBDevice>();
  const portFor = (device: USBDevice): SerialPort => {
    let port = ports.get(device);
    if (!port) {
      if (!described.has(device)) { log(describeUsb(device)); described.add(device); }
      port = wrap(device); ports.set(device, port);
    }
    return port;
  };
  return {
    kind: "webusb",
    async getPorts() {
      const ports: SerialPort[] = [];
      for (const device of await usb.getDevices()) {
        try { ports.push(portFor(device)); } catch { /* Other previously authorized USB devices are not necessarily CDC serial devices. */ }
      }
      return ports;
    },
    async requestPort(options) {
      const filters = options?.filters?.map((filter) => ({ vendorId: filter.usbVendorId, ...(filter.usbProductId === undefined ? {} : { productId: filter.usbProductId }) })) ?? [];
      const device = await usb.requestDevice({ filters });
      try { return portFor(device); }
      catch (error) { throw new Error(`Selected USB device has no compatible CDC serial interface. ${describeUsb(device)}. ${String(error)}`); }
    },
    on(type, callback) {
      usb.addEventListener(type, (event) => {
        try { callback(portFor(event.device)); } catch { /* Ignore non-serial devices. */ }
      });
    },
  };
}

/** Select a service without modifying navigator.serial. */
export function createSerialBackend(nav: Pick<Navigator, "userAgent"> & Partial<Pick<Navigator, "serial" | "usb">>, log: (message: string) => void): SerialBackend | undefined {
  const kind = chooseSerialKind(/Android/i.test(nav.userAgent), !!nav.serial, !!nav.usb);
  if (kind === "webusb") return usbBackend(nav.usb!, log);
  if (kind === "native") {
    const serial = nav.serial!;
    return { kind, getPorts: () => serial.getPorts(), requestPort: (options) => serial.requestPort(options), on(type, callback) {
      serial.addEventListener(type, (event) => callback((event as Event & { port?: SerialPort }).port ?? event.target as SerialPort));
    } };
  }
  return undefined;
}

/** Actionable help also for chooser cancellation, since browsers do not distinguish an empty list. */
export const USB_HELP = "Use a USB-C data cable and a phone with USB host/OTG enabled. Close other USB/serial apps, check Chrome's USB site permission, then reconnect and authorize again. If access/claim fails, the USB interface may be reserved by Android or Chrome. Expected CoreS3 native USB: 303a:1001, CDC control/data interfaces. Download the Autoflash log for diagnostics.";
