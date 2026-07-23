"""Race-safe wakeups for RNS packet receipts.

RNS receipts can settle before a caller installs their callbacks, and callbacks
are not replayed.  Arm both callbacks first, then inspect the status: an early
settlement is observed by the status check, a later settlement invokes a
callback, and the event safely coalesces the overlap between both paths.
"""

import threading
import time


class ReceiptSettlementWake:
    def __init__(self):
        self._event = threading.Event()

    def arm(self, receipt, pending_status):
        if receipt is None:
            self._event.set()
            return
        receipt.set_delivery_callback(self.notify)
        receipt.set_timeout_callback(self.notify)
        if receipt.status != pending_status:
            self._event.set()

    def notify(self, _receipt=None):
        self._event.set()

    def clear_before_scan(self):
        self._event.clear()

    def wait_until(self, deadline):
        return self._event.wait(max(0.0, deadline - time.monotonic()))

    def is_set(self):
        return self._event.is_set()
