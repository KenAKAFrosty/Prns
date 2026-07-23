import unittest

try:
    from .receipt_settlement import ReceiptSettlementWake
except ImportError:
    from receipt_settlement import ReceiptSettlementWake


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


class ReceiptSettlementWakeTests(unittest.TestCase):
    def test_completion_before_callback_registration_wakes_from_status(self):
        wake = ReceiptSettlementWake()

        wake.arm(FakeReceipt(DELIVERED), PENDING)

        self.assertTrue(wake.is_set())

    def test_completion_after_registration_wakes_from_callback(self):
        wake = ReceiptSettlementWake()
        receipt = FakeReceipt()
        wake.arm(receipt, PENDING)
        self.assertFalse(wake.is_set())

        receipt.status = DELIVERED
        receipt.delivery_callback(receipt)

        self.assertTrue(wake.is_set())

    def test_completion_during_registration_is_coalesced(self):
        wake = ReceiptSettlementWake()

        wake.arm(FakeReceipt(settle_while_arming=True), PENDING)

        self.assertTrue(wake.is_set())

    def test_timeout_callback_wakes(self):
        wake = ReceiptSettlementWake()
        receipt = FakeReceipt()
        wake.arm(receipt, PENDING)

        receipt.status = FAILED
        receipt.timeout_callback(receipt)

        self.assertTrue(wake.is_set())

    def test_missing_receipt_wakes_for_immediate_failure(self):
        wake = ReceiptSettlementWake()

        wake.arm(None, PENDING)

        self.assertTrue(wake.is_set())


if __name__ == "__main__":
    unittest.main()
