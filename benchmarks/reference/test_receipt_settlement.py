import time
import unittest

try:
    from .receipt_settlement import ReceiptSettlementQueue
except ImportError:
    from receipt_settlement import ReceiptSettlementQueue


PENDING = 1
DELIVERED = 2
FAILED = 0


class FakeReceipt:
    def __init__(self, status=PENDING, settle_while_arming=False):
        self.status = status
        self.delivery_callback = None
        self.timeout_callback = None
        self.settle_while_arming = settle_while_arming

    def set_delivery_callback(self, callback):
        self.delivery_callback = callback
        if self.settle_while_arming:
            self.status = DELIVERED
            callback(self)

    def set_timeout_callback(self, callback):
        self.timeout_callback = callback


class ReceiptSettlementQueueTests(unittest.TestCase):
    def pop(self, settlements):
        return settlements.pop_until(time.monotonic() + 0.1)

    def test_completion_before_callback_registration_is_queued(self):
        settlements = ReceiptSettlementQueue()
        armed = settlements.arm(FakeReceipt(DELIVERED), PENDING, 60)

        self.assertIs(self.pop(settlements), armed)
        self.assertEqual(armed.context, 60)

    def test_completion_after_registration_is_queued(self):
        settlements = ReceiptSettlementQueue()
        receipt = FakeReceipt()
        armed = settlements.arm(receipt, PENDING)

        receipt.status = DELIVERED
        receipt.delivery_callback(receipt)

        self.assertIs(self.pop(settlements), armed)

    def test_completion_during_registration_is_queued_once(self):
        settlements = ReceiptSettlementQueue()
        receipt = FakeReceipt(settle_while_arming=True)
        armed = settlements.arm(receipt, PENDING)

        self.assertIs(self.pop(settlements), armed)
        self.assertIsNone(settlements.pop_until(time.monotonic()))

    def test_duplicate_callbacks_are_coalesced(self):
        settlements = ReceiptSettlementQueue()
        receipt = FakeReceipt()
        armed = settlements.arm(receipt, PENDING)

        receipt.status = DELIVERED
        receipt.delivery_callback(receipt)
        receipt.timeout_callback(receipt)

        self.assertIs(self.pop(settlements), armed)
        self.assertIsNone(settlements.pop_until(time.monotonic()))

    def test_timeout_callback_is_queued(self):
        settlements = ReceiptSettlementQueue()
        receipt = FakeReceipt()
        armed = settlements.arm(receipt, PENDING)

        receipt.status = FAILED
        receipt.timeout_callback(receipt)

        self.assertIs(self.pop(settlements), armed)

    def test_missing_receipt_is_immediately_queued(self):
        settlements = ReceiptSettlementQueue()
        armed = settlements.arm(None, PENDING)

        self.assertIs(self.pop(settlements), armed)

    def test_wait_deadline_returns_none(self):
        settlements = ReceiptSettlementQueue()

        self.assertIsNone(settlements.pop_until(time.monotonic() + 0.001))


if __name__ == "__main__":
    unittest.main()
