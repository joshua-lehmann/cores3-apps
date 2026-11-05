import { describe, expect, it, vi } from "vitest";
import { chooseSerialKind, createSerialBackend, usbBackend } from "./serial-backend";

function device(): USBDevice {
  return { vendorId: 0x303a, productId: 0x1001, configurations: [{ configurationValue: 1, interfaces: [
    { interfaceNumber: 0, alternates: [{ interfaceClass: 2, endpoints: [] }] },
    { interfaceNumber: 1, alternates: [{ interfaceClass: 10, endpoints: [{ direction: "in", type: "bulk", endpointNumber: 1, packetSize: 64 }, { direction: "out", type: "bulk", endpointNumber: 1, packetSize: 64 }] }] },
  ] }] } as unknown as USBDevice;
}

describe("serial backend selection", () => {
  it("keeps desktop native and prefers Android USB even when native exists", () => {
    expect(chooseSerialKind(false, true, true)).toBe("native");
    expect(chooseSerialKind(true, true, true)).toBe("webusb");
    expect(chooseSerialKind(true, false, true)).toBe("webusb");
    expect(chooseSerialKind(true, true, false)).toBe("native");
    expect(chooseSerialKind(false, false, true)).toBe("webusb");
    expect(chooseSerialKind(false, false, false)).toBeUndefined();
  });
  it("retains the native service and forwards filters without overriding navigator", async () => {
    const serial = { requestPort: vi.fn(), getPorts: vi.fn().mockResolvedValue([]), addEventListener: vi.fn() };
    const nav = { userAgent: "Chrome Linux", serial } as unknown as Navigator;
    const backend = createSerialBackend(nav, vi.fn())!;
    await backend.requestPort({ filters: [{ usbVendorId: 0x303a }] });
    expect(serial.requestPort).toHaveBeenCalledWith({ filters: [{ usbVendorId: 0x303a }] });
    expect(nav.serial).toBe(serial);
  });
});

describe("USB service", () => {
  it("retains port identity across enumeration, chooser and disconnect/reconnect events", async () => {
    const selected = device();
    const usb = { getDevices: vi.fn().mockResolvedValue([selected]), requestDevice: vi.fn().mockResolvedValue(selected), addEventListener: vi.fn() };
    const backend = usbBackend(usb as unknown as USB, vi.fn());
    const [first] = await backend.getPorts();
    expect((await backend.getPorts())[0]).toBe(first);
    expect(await backend.requestPort({ filters: [{ usbVendorId: 0x303a }] })).toBe(first);
    expect(usb.requestDevice).toHaveBeenCalledWith({ filters: [{ vendorId: 0x303a }] });
    const disconnected = vi.fn(); backend.on("disconnect", disconnected);
    usb.addEventListener.mock.calls[0][1]({ device: selected });
    expect(disconnected).toHaveBeenCalledWith(first);
    expect(first.getInfo()).toEqual({ usbVendorId: 0x303a, usbProductId: 0x1001 });
  });
  it("ignores unrelated authorized USB devices and explains unsupported selected interfaces", async () => {
    const invalid = { ...device(), configurations: [] };
    const usb = { getDevices: vi.fn().mockResolvedValue([invalid, device()]), requestDevice: vi.fn().mockResolvedValue(invalid) };
    const backend = usbBackend(usb as unknown as USB, vi.fn());
    expect(await backend.getPorts()).toHaveLength(1);
    await expect(backend.requestPort()).rejects.toThrow("no compatible CDC serial interface");
  });
  it("does not retry or fall back to a second permission dialog after cancellation", async () => {
    const usb = { requestDevice: vi.fn().mockRejectedValue(new DOMException("Cancelled", "NotFoundError")) };
    await expect(usbBackend(usb as unknown as USB, vi.fn()).requestPort()).rejects.toThrow("Cancelled");
    expect(usb.requestDevice).toHaveBeenCalledTimes(1);
  });
  it("uses the pinned polyfill for opening CDC interfaces, writes, signals and closing", async () => {
    const selected = device();
    Object.assign(selected, { opened: false, configuration: null,
      open: vi.fn(async () => { Object.assign(selected, { opened: true }); }),
      selectConfiguration: vi.fn(async () => { Object.assign(selected, { configuration: selected.configurations[0] }); }),
      claimInterface: vi.fn().mockResolvedValue(undefined),
      controlTransferOut: vi.fn().mockResolvedValue({ status: "ok" }),
      transferOut: vi.fn().mockResolvedValue({ status: "ok" }),
      close: vi.fn(async () => { Object.assign(selected, { opened: false }); }),
    });
    const backend = usbBackend({ requestDevice: vi.fn().mockResolvedValue(selected) } as unknown as USB, vi.fn());
    const port = await backend.requestPort();
    await port.open({ baudRate: 115200 });
    await port.setSignals({ dataTerminalReady: true, requestToSend: false });
    const writer = port.writable!.getWriter();
    await writer.write(new Uint8Array([0xc0, 1, 0xc0])); writer.releaseLock();
    await port.close();
    expect(selected.claimInterface).toHaveBeenCalledWith(0);
    expect(selected.claimInterface).toHaveBeenCalledWith(1);
    expect(selected.transferOut).toHaveBeenCalledWith(1, new Uint8Array([0xc0, 1, 0xc0]));
    expect(selected.close).toHaveBeenCalled();
  });
});
