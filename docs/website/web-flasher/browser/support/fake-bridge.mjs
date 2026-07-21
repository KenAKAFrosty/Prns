export async function installFakeBridge(page, overrides = {}) {
  const configuration = {
    supported: true,
    failureCode: null,
    pauseAtWriting: false,
    ...overrides,
  };

  await page.addInitScript((config) => {
    const state = {
      active: false,
      cancelled: false,
      cleanupCount: 0,
      clearPreparedCount: 0,
      lastRequest: null,
      phaseLog: [],
      preparedBoardSlug: null,
      preparationSettledCount: 0,
      provisioningWasCleared: false,
      readyCount: 0,
      resumeWriting: null,
    };
    let prepared = null;
    let preparingRequest = null;
    let preparationGeneration = 0;

    Object.defineProperty(navigator, "serial", {
      configurable: true,
      value: config.supported ? { requestPort: async () => ({}) } : undefined,
    });

    const navigationGuard = (event) => {
      if (!state.active) return;
      event.preventDefault();
      event.returnValue = "";
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
    const failMessage = (code) => {
      if (code === "wrong_chip") {
        return "Wrong chip family. Re-check the printed board label before retrying.";
      }
      if (code === "reset_failure") {
        return "Firmware verified, but reset failed. Press RESET and check the next boot.";
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
          transport: request.transport,
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
            const bytes = new Uint8Array(await response.arrayBuffer());
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
            parts: request.parts.map(({ kind, size }) => ({ kind, size })),
            transport: request.transport,
          };
          state.preparedBoardSlug = request.boardSlug;
          const provisioningBytes =
            request.provisioning && request.provisioning.action !== "preserve"
              ? request.provisioning.size
              : 0;
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
        window.addEventListener("beforeunload", navigationGuard);
        try {
          if (prepared.transport === "uf2-mass-storage") {
            const total = prepared.parts[0].size;
            await emitEvent(emit, {
              phase: "success",
              current: total,
              total,
              message: "Verified UF2 downloaded. Copy it to TECHOBOOT; the drive disappears when the device reboots.",
            });
            return;
          }

          await emitEvent(emit, { phase: "requesting_port" });
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
          state.resumeWriting = null;
          prepared = null;
          state.cleanupCount += 1;
          window.removeEventListener("beforeunload", navigationGuard);
        }
      },

      cancel() {
        preparationGeneration += 1;
        clearPreparingRequest();
        state.cancelled = true;
        state.resumeWriting?.();
      },

      clearPrepared() {
        preparationGeneration += 1;
        clearPreparingRequest();
        state.clearPreparedCount += 1;
        if (state.active) {
          state.cancelled = true;
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
