import { FlashBridgeError } from "./core.js";

// A bounded polling window, followed by an explicit pause that retains ownership
// of the connection. Resuming never submits another authorization.
export const ENROLLMENT_WAIT_MILLIS = 120_000;
export const ENROLLMENT_POLL_MILLIS = 250;

export function createEnrollmentWait(dependencies = {}) {
  const now = dependencies.now ?? (() => performance.now());
  const duration = dependencies.setupTimeoutMs ?? ENROLLMENT_WAIT_MILLIS;
  let deadline, started, lastProgress, wake;
  let pending = false, paused = false, stopped = false, closed = false;
  const deliveryFailed = () => {
    if (closed) return;
    stopped = true;
    wake?.();
  };
  const emit = (force = false) => {
    const second = Math.floor((now() - started) / 1000);
    if (!force && second === lastProgress) return;
    lastProgress = second;
    // A presentation failure cannot change the board's authorization outcome.
    try {
      const delivered = dependencies.onProgress?.({
        status: "waiting", paused,
        canContinue: paused || now() - started >= 30_000,
        message: paused
          ? "The board last reported that setup was pending. Checking is paused and this page is keeping the USB connection open. Continue checking, or stop checking to release it."
          : "Waiting for the board to finish starting and save access. Keep it connected; this can take a minute or more.",
      });
      Promise.resolve(delivered).catch(deliveryFailed);
    } catch { deliveryFailed(); }
  };
  return {
    continueChecking() {
      if (closed || !pending || stopped) return false;
      started = now();
      deadline = started + duration;
      paused = false;
      wake?.();
      emit(true);
      return true;
    },
    stopChecking() {
      if (closed) return false;
      stopped = true;
      wake?.();
      return true;
    },
    throwIfStopped() {
      if (stopped) throw new FlashBridgeError("cancelled", "The setup check was stopped.");
    },
    async afterPending() {
      pending = true;
      if (deadline === undefined) {
        started = now();
        deadline = started + duration;
      }
      emit();
      if (!stopped && now() >= deadline) {
        paused = true;
        await new Promise((resolve) => { wake = resolve; emit(true); });
        wake = undefined;
      }
      return stopped ? {
        status: "pending",
        message: "Checking stopped. The board last reported that access was still being saved. The setup request was not cancelled and may finish. Keep the board powered; check the same attempt’s status before authorizing again.",
      } : null;
    },
    close() { closed = true; },
  };
}
