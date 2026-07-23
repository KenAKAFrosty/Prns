"""Race-safe, callback-driven settlement delivery for RNS packet receipts."""

import threading
import time
from collections import deque


class _ArmedReceipt:
    def __init__(self, owner, receipt, context):
        self.owner = owner
        self.receipt = receipt
        self.context = context
        self._queued = False
        self._lock = threading.Lock()

    def notify(self, _receipt=None):
        with self._lock:
            if self._queued:
                return
            self._queued = True
        self.owner._settled(self)


class ReceiptSettlementQueue:
    """Delivers each settled receipt once, including registration races.

    RNS does not replay callbacks when a receipt settles before callback
    registration. Both callbacks are installed first and the status is then
    inspected. A token-local once gate coalesces a callback racing that status
    check, so consumers never rescan the whole outstanding window.
    """

    def __init__(self):
        self._condition = threading.Condition()
        self._ready = deque()

    def arm(self, receipt, pending_status, context=None):
        armed = _ArmedReceipt(self, receipt, context)
        if receipt is None:
            armed.notify()
            return armed
        receipt.set_delivery_callback(armed.notify)
        receipt.set_timeout_callback(armed.notify)
        if receipt.status != pending_status:
            armed.notify()
        return armed

    def _settled(self, armed):
        with self._condition:
            self._ready.append(armed)
            self._condition.notify()

    def pop_until(self, deadline):
        with self._condition:
            while not self._ready:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return None
                self._condition.wait(remaining)
            return self._ready.popleft()
