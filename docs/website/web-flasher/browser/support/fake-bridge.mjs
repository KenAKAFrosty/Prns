export async function installFakeBridge(page, overrides = {}) {
  const configuration = {
    supported: true,
    supportDetectionFailure: false,
    androidPlatform: null,
    failureCode: null,
    pauseAtWriting: false,
    preparationProtocolViolation: false,
    deviceProtocolViolation: false,
    ...overrides,
  };

  await page.addInitScript((config) => {
    const state = {
      active: false,
      cancelled: false,
      cancellationLocked: false,
      cleanupCount: 0,
      clearPreparedCount: 0,
      lastRequest: null,
      phaseLog: [],
      preparedBoardSlug: null,
      preparationSettledCount: 0,
      provisioningWasCleared: false,
      readyCount: 0,
      completedPartCount: 0,
      eraseCount: 0,
      resumePreparation: null,
      resumeWriting: null,
    };
    let prepared = null;
    let preparingRequest = null;
    let preparationGeneration = 0;

    Object.defineProperty(navigator, "serial", config.supportDetectionFailure
      ? {
          configurable: true,
          get() {
            throw new Error("injected Web Serial capability detection failure");
          },
        }
      : {
          configurable: true,
          value: config.supported ? { requestPort: async () => ({}) } : undefined,
        });

    if (config.androidPlatform) {
      Object.defineProperty(navigator, "userAgentData", {
        configurable: true,
        value: config.androidPlatform === "client-hints" ? { platform: "Android" } : undefined,
      });
      if (config.androidPlatform === "legacy-ua") {
        Object.defineProperty(navigator, "userAgent", {
          configurable: true,
          value:
            "Mozilla/5.0 (Linux; Android 16; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Mobile Safari/537.36",
        });
      }
    }

    const navigationGuard = (event) => {
      if (!state.active) return;
      event.preventDefault();
      event.returnValue = "";
    };
    const internalNavigationGuard = (event) => {
      if (!state.active || event.defaultPrevented || event.button > 0) return;
      const link = event.target?.closest?.("a[href]");
      if (!link || link.download || (link.target && link.target !== "_self")) return;
      const destination = new URL(link.href, window.location.href);
      if (destination.origin !== window.location.origin || destination.href === window.location.href) return;
      event.preventDefault();
      event.stopImmediatePropagation();
    };
    const emitEvent = async (emit, event) => {
      const value = { schema: 1, ...event };
      state.phaseLog.push(value.phase);
      emit(value);
      await new Promise((resolve) => setTimeout(resolve, 0));
    };
    const digest = async (bytes) => {
      const value = await crypto.subtle.digest(
        "SHA-256",
        bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
      );
      return Array.from(new Uint8Array(value), (byte) =>
        byte.toString(16).padStart(2, "0"),
      ).join("");
    };
    const readExactResponse = async (response, expectedSize) => {
      const declared = response.headers.get("content-length");
      if (declared !== null && BigInt(declared) > BigInt(expectedSize)) {
        throw Object.assign(new Error("fixture artifact size mismatch"), {
          code: "artifact_size_mismatch",
        });
      }
      const reader = response.body?.getReader?.();
      if (!reader) {
        throw Object.assign(new Error("fixture artifact stream unavailable"), {
          code: "artifact_fetch",
        });
      }
      const output = new Uint8Array(expectedSize);
      let current = 0;
      try {
        while (true) {
          let result;
          try {
            result = await reader.read();
          } catch (error) {
            throw Object.assign(new Error("fixture artifact stream failed", { cause: error }), {
              code: "artifact_fetch",
            });
          }
          if (result.done) break;
          if (!(result.value instanceof Uint8Array) || result.value.byteLength > expectedSize - current) {
            await reader.cancel?.().catch(() => {});
            throw Object.assign(new Error("fixture artifact size mismatch"), {
              code: "artifact_size_mismatch",
            });
          }
          output.set(result.value, current);
          current += result.value.byteLength;
        }
      } finally {
        reader.releaseLock?.();
      }
      if (current !== expectedSize) {
        throw Object.assign(new Error("fixture artifact size mismatch"), {
          code: "artifact_size_mismatch",
        });
      }
      return output;
    };
    const failMessage = (code) => {
      if (code === "permission_denied") {
        return "No serial port was selected. Review the board and choose its port when you try again.";
      }
      if (code === "wrong_chip") {
        return "Wrong chip family. Re-check the printed board label before retrying.";
      }
      if (code === "reset_failure") {
        return "Firmware verified, but reset failed. Press RESET and check the next boot.";
      }
      if (code === "erase_failure") {
        return "Full-chip erase failed. The device may be blank; confirm and retry the complete fresh-install plan.";
      }
      return "The device operation stopped. Re-enter BOOT mode, press RESET, and restart the complete sparse plan.";
    };

    window.__prnsFlashTest = {
      state,
      resume() {
        state.resumeWriting?.();
      },
    };

    window.__prnsFlash = {
      async prepare(request, emit) {
        const generation = ++preparationGeneration;
        clearPreparingRequest();
        preparingRequest = request;
        prepared = null;
        state.preparedBoardSlug = null;
        state.cancelled = false;
        state.lastRequest = {
          boardSlug: request.boardSlug,
          displayName: request.displayName,
          expectedChip: request.expectedChip,
          mountLabel: request.mountLabel,
          transport: request.transport,
          installMode: request.installMode ?? null,
          eraseConfirmed: request.eraseConfirmed ?? null,
          provisioningAction: request.provisioning?.action ?? null,
          ssidBytes: new TextEncoder().encode(request.provisioning?.ssid ?? "").length,
          passwordBytes: new TextEncoder().encode(request.provisioning?.password ?? "").length,
          partKinds: request.parts.map((part) => part.kind),
        };
        try {
          await emitEvent(emit, { phase: "validating_manifest" });
          requireCurrentPreparation(generation);
          const total = request.parts.reduce((sum, part) => sum + part.size, 0);
          let current = 0;
          for (const [partIndex, part] of request.parts.entries()) {
            await emitEvent(emit, {
              phase: "downloading",
              part: part.kind,
              partIndex,
              partCount: request.parts.length,
              current,
              total,
            });
            const response = await fetch(part.url, {
              cache: "no-store",
              credentials: "omit",
              redirect: "error",
            });
            requireCurrentPreparation(generation);
            if (!response.ok) {
              await emitEvent(emit, {
                phase: "failed",
                code: "artifact_fetch",
                message: "The signed fixture artifact could not be downloaded.",
              });
              throw new Error("fixture artifact fetch failed");
            }
            let bytes;
            try {
              bytes = await readExactResponse(response, part.size);
            } catch (error) {
              const code = error?.code === "artifact_size_mismatch"
                ? "artifact_size_mismatch"
                : "artifact_fetch";
              await emitEvent(emit, {
                phase: "failed",
                code,
                message: code === "artifact_size_mismatch"
                  ? "The signed fixture artifact size did not match. Reload and prepare again."
                  : "The signed fixture artifact could not be streamed safely. Reload and prepare again.",
              });
              throw error;
            }
            requireCurrentPreparation(generation);
            if (bytes.byteLength !== part.size) {
              await emitEvent(emit, {
                phase: "failed",
                code: "artifact_size_mismatch",
                message: "The signed fixture artifact size did not match.",
              });
              throw new Error("fixture artifact size mismatch");
            }
            if ((await digest(bytes)) !== part.sha256) {
              await emitEvent(emit, {
                phase: "failed",
                code: "artifact_hash_mismatch",
                message: "The signed fixture artifact hash did not match.",
              });
              throw new Error("fixture artifact hash mismatch");
            }
            requireCurrentPreparation(generation);
            current += bytes.byteLength;
            await emitEvent(emit, {
              phase: "verifying_artifacts",
              part: part.kind,
              partIndex,
              partCount: request.parts.length,
              current,
              total,
            });
          }
          requireCurrentPreparation(generation);
          prepared = {
            expectedChip: request.expectedChip,
            installMode: request.installMode,
            mountLabel: request.mountLabel,
            parts: request.parts.map(({ kind, size }) => ({ kind, size })),
            transport: request.transport,
          };
          state.preparedBoardSlug = request.boardSlug;
          const provisioningBytes =
            request.provisioning && request.provisioning.action !== "preserve"
              ? request.provisioning.size
              : 0;
          if (config.preparationProtocolViolation) {
            const stopped = new Promise((resolve) => {
              state.resumePreparation = resolve;
            });
            await emitEvent(emit, {
              phase: "ready",
              current: total - 1,
              total,
              bytes: total + provisioningBytes,
            });
            await stopped;
            state.resumePreparation = null;
            requireCurrentPreparation(generation);
          }
          await emitEvent(emit, {
            phase: "ready",
            current: total,
            total,
            bytes: total + provisioningBytes,
          });
          state.readyCount += 1;
          return { ready: true };
        } finally {
          clearProvisioning(request);
          if (preparingRequest === request) {
            preparingRequest = null;
          }
          state.preparationSettledCount += 1;
        }
      },

      async flash(emit) {
        if (!prepared) {
          await emitEvent(emit, {
            phase: "failed",
            code: "not_prepared",
            message: "Prepare the signed fixture before flashing.",
          });
          return;
        }
        state.active = true;
        state.cancelled = false;
        state.cancellationLocked = false;
        let retainPreparedPlan = false;
        window.addEventListener("beforeunload", navigationGuard);
        document.addEventListener("click", internalNavigationGuard, true);
        try {
          if (prepared.transport === "uf2-mass-storage") {
            const total = prepared.parts[0].size;
            await emitEvent(emit, {
              phase: "download_requested",
              current: total,
              total,
              message: `Verified UF2 download requested. Check the browser's downloads, then copy it to ${prepared.mountLabel}; the drive disappears when the device reboots.`,
            });
            return;
          }

          await emitEvent(emit, { phase: "requesting_port" });
          if (config.failureCode === "permission_denied") {
            await emitEvent(emit, {
              phase: "failed",
              code: config.failureCode,
              message: failMessage(config.failureCode),
            });
            retainPreparedPlan = prepared.installMode === "preserve-data";
            return;
          }
          await emitEvent(emit, { phase: "connecting" });
          await emitEvent(emit, {
            phase: "verifying_target",
            detectedChip: prepared.expectedChip,
          });
          if (config.failureCode === "wrong_chip") {
            await emitEvent(emit, {
              phase: "failed",
              code: config.failureCode,
              message: failMessage(config.failureCode),
            });
            return;
          }

          if (prepared.installMode === "erase-all") {
            state.cancellationLocked = true;
            await emitEvent(emit, { phase: "erasing" });
            state.eraseCount += 1;
            if (config.failureCode === "erase_failure") {
              await emitEvent(emit, {
                phase: "failed",
                code: config.failureCode,
                message: failMessage(config.failureCode),
              });
              return;
            }
          }

          const total = prepared.parts.reduce((sum, part) => sum + part.size, 0);
          let current = 0;
          for (const [partIndex, part] of prepared.parts.entries()) {
            await emitEvent(emit, {
              phase: "writing",
              part: part.kind,
              partIndex,
              partCount: prepared.parts.length,
              current,
              total,
            });
            if (config.deviceProtocolViolation && partIndex === 0) {
              const stopped = new Promise((resolve) => {
                state.resumeWriting = resolve;
              });
              await emitEvent(emit, {
                phase: "writing",
                part: part.kind,
                partIndex,
                partCount: prepared.parts.length,
                current: total + 1,
                total,
              });
              await stopped;
              state.resumeWriting = null;
            }
            if (config.pauseAtWriting && partIndex === 0) {
              await new Promise((resolve) => {
                state.resumeWriting = resolve;
              });
              state.resumeWriting = null;
            }
            if (state.cancelled) {
              await emitEvent(emit, {
                phase: "cancelled",
                code: "cancelled",
                message: "Flashing stopped at a safe part boundary; no success was reported.",
              });
              return;
            }
            if (["device_lost", "write_failure", "verification_failure"].includes(config.failureCode)) {
              await emitEvent(emit, {
                phase: "failed",
                code: config.failureCode,
                message: failMessage(config.failureCode),
              });
              return;
            }
            current += part.size;
            state.completedPartCount += 1;
          }
          await emitEvent(emit, { phase: "verifying_flash", current: total, total });
          await emitEvent(emit, { phase: "resetting" });
          if (config.failureCode === "reset_failure") {
            await emitEvent(emit, {
              phase: "failed",
              code: config.failureCode,
              message: failMessage(config.failureCode),
            });
            return;
          }
          await emitEvent(emit, { phase: "success", current: total, total });
        } finally {
          state.active = false;
          state.cancellationLocked = false;
          state.resumeWriting = null;
          if (!retainPreparedPlan) {
            state.preparedBoardSlug = null;
            prepared = null;
          }
          state.cleanupCount += 1;
          window.removeEventListener("beforeunload", navigationGuard);
          document.removeEventListener("click", internalNavigationGuard, true);
        }
      },

      cancel() {
        preparationGeneration += 1;
        clearPreparingRequest();
        if (!state.cancellationLocked) {
          state.cancelled = true;
        }
        state.resumePreparation?.();
        state.resumeWriting?.();
      },

      clearPrepared() {
        preparationGeneration += 1;
        clearPreparingRequest();
        state.clearPreparedCount += 1;
        state.resumePreparation?.();
        if (state.active) {
          if (!state.cancellationLocked) {
            state.cancelled = true;
          }
          state.resumeWriting?.();
        } else {
          prepared = null;
          state.preparedBoardSlug = null;
        }
      },
    };

    function requireCurrentPreparation(generation) {
      if (generation !== preparationGeneration) {
        throw new Error("fixture preparation was invalidated");
      }
    }

    function clearProvisioning(request) {
      if (request?.provisioning) {
        request.provisioning.ssid = "";
        request.provisioning.password = "";
        state.provisioningWasCleared = true;
      }
    }

    function clearPreparingRequest() {
      clearProvisioning(preparingRequest);
      preparingRequest = null;
    }
  }, configuration);
}
