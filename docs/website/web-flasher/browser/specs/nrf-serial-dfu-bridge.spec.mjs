import { expect, test } from "@playwright/test";

test("the production browser bridge completes exact T1000-E serial DFU through typed Rust state", async ({
  page,
}) => {
  await page.goto("/");
  const evidence = await page.evaluate(async () => {
    const bridge = await import("/assets/flasher/prns-flash.js");
    const application = Uint8Array.of(1, 2, 3, 4);
    const initPacket = Uint8Array.of(5, 6);
    async function sha256(bytes) {
      const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
      return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
    }
    const request = {
      schema: 1,
      boardSlug: "t1000-e",
      displayName: "Seeed Studio SenseCAP Card Tracker T1000-E",
      transport: "nrf-serial-dfu",
      expectedChip: null,
      flashSize: null,
      flashMode: null,
      flashFrequency: null,
      beforeReset: null,
      afterReset: null,
      mountLabel: null,
      uf2Compatibility: null,
      provisioning: null,
      serialFilters: [{ usbVendorId: 0x2886, usbProductId: 0x0057 }],
      nrfSerialDfu: {
        entry: "touch-application-or-bootloader",
        touchApplicationAndBootloaderUsb: { vendorId: 0x2886, productId: 0x0057 },
        touchBaudRate: 1_200,
        transferBaudRate: 115_200,
        managedApplication: {
          usb: { vendorId: 0x1209, productId: 0x0001 },
          manufacturer: "Stay Personal",
          product: "Personal Hopspot (T1000-E)",
          serialNumber: "PERSONAL-RNS-T1000E-HOP",
          interfaceNumber: 0,
          request: 0x50,
          value: 0x5052,
          index: 0x4e53,
        },
        compatibility: {
          softdeviceFamily: "s140",
          softdeviceVersion: "7.3.0",
          softdeviceFwids: [0x0123],
          deviceType: 0x0052,
          deviceRevision: 52840,
          applicationVersion: "not-enforced",
          applicationBase: 0x27000,
          applicationEndExclusive: 0xea000,
          bankLayout: "single",
        },
      },
      parts: [
        {
          kind: "dfu-application",
          path: "firmware/hopspot/t1000-e/0.3.7/app.bin",
          url: "/releases/0.3.7/firmware/hopspot/t1000-e/0.3.7/app.bin",
          offset: null,
          size: application.length,
          sha256: await sha256(application),
        },
        {
          kind: "dfu-init-packet",
          path: "firmware/hopspot/t1000-e/0.3.7/app.dat",
          url: "/releases/0.3.7/firmware/hopspot/t1000-e/0.3.7/app.dat",
          offset: null,
          size: initPacket.length,
          sha256: await sha256(initPacket),
        },
      ],
    };
    const kinds = {
      AwaitingMore: 0,
      RetryRequired: 1,
      FrameAccepted: 2,
      TransferComplete: 3,
    };
    const core = {
      NrfDfuBankLayout: { Single: 0, Dual: 1 },
      NrfDfuAcknowledgementTransitionKind: kinds,
      NrfDfuCompatibility: {
        notEnforcedApplication() { return { free() {} }; },
      },
      NrfDfuSession: class {
        nextFrame() { return { bytes: Uint8Array.of(1, 2), free() {} }; }
        retryFrame() { throw new Error("retry was not expected"); }
        pushAcknowledgement() {
          return {
            kind: kinds.TransferComplete,
            writtenBytes: application.length,
            totalBytes: application.length,
            waitMilliseconds: 0,
            free() {},
          };
        }
        free() {}
      },
    };
    const preparationEvents = [];
    const payloads = new Map([
      [request.parts[0].url, application],
      [request.parts[1].url, initPacket],
    ]);
    await bridge.prepare(request, (event) => preparationEvents.push(event), {
      nrfDfuCore: core,
      cryptoImpl: crypto,
      fetchImpl: async (url) => new Response(payloads.get(url), {
        status: payloads.has(url) ? 200 : 404,
      }),
    });

    const state = { baudRates: [], writes: 0, closes: 0 };
    const port = {
      readable: null,
      writable: null,
      getInfo: () => ({ usbVendorId: 0x2886, usbProductId: 0x0057 }),
      async open(options) {
        state.baudRates.push(options.baudRate);
        this.readable = {
          getReader: () => ({
            read: async () => ({ done: false, value: Uint8Array.of(0xc0) }),
            cancel: async () => {},
            releaseLock() {},
          }),
        };
        this.writable = {
          getWriter: () => ({
            write: async () => { state.writes += 1; },
            releaseLock() {},
          }),
        };
      },
      async setSignals() {},
      async close() {
        state.closes += 1;
        this.readable = null;
        this.writable = null;
      },
    };
    const serial = {
      requestPort: async () => port,
      getPorts: async () => [port],
      addEventListener() {},
      removeEventListener() {},
    };
    let milliseconds = 0;
    const deviceEvents = [];
    const result = await bridge.flash((event) => deviceEvents.push(event), {
      serial,
      environment: {
        isSecureContext: true,
        navigator: { serial },
        addEventListener() {},
        removeEventListener() {},
      },
      nowImpl: () => milliseconds,
      sleepImpl: async (duration) => { milliseconds += Math.max(1, duration); },
    });
    return {
      result,
      preparationPhases: preparationEvents.map(({ phase }) => phase),
      devicePhases: deviceEvents.map(({ phase }) => phase),
      baudRates: state.baudRates,
      writes: state.writes,
      closes: state.closes,
    };
  });

  expect(evidence.result).toEqual({ success: true });
  expect(evidence.preparationPhases).toEqual([
    "validating_manifest",
    "downloading",
    "verifying_artifacts",
    "downloading",
    "verifying_artifacts",
    "ready",
  ]);
  expect(evidence.devicePhases).toEqual([
    "requesting_port",
    "connecting",
    "verifying_target",
    "writing",
    "writing",
    "verifying_flash",
    "resetting",
    "success",
  ]);
  expect(evidence.baudRates).toEqual([1_200, 115_200, 115_200]);
  expect(evidence.writes).toBe(1);
  expect(evidence.closes).toBeGreaterThanOrEqual(3);
});
