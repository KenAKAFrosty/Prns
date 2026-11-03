use prns_core::interfaces::lora::LORA_MAX_PAYLOAD;

const PACKET_LENGTH_BYTES: usize = size_of::<u16>();
const HEADER_BYTES: usize = PACKET_LENGTH_BYTES + size_of::<u64>();
const MAX_RECORD_BYTES: usize = HEADER_BYTES + LORA_MAX_PAYLOAD;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TransmitQueueError {
    Full,
    PacketTooLarge,
}

pub(super) struct TransmitQueue<'a> {
    storage: &'a mut [u8],
    head: usize,
    used: usize,
}

impl<'a> TransmitQueue<'a> {
    pub(super) fn new(storage: &'a mut [u8]) -> Self {
        Self {
            storage,
            head: 0,
            used: 0,
        }
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.used == 0
    }

    pub(super) const fn can_push_max_packet(&self) -> bool {
        self.storage.len().saturating_sub(self.used) >= MAX_RECORD_BYTES
    }

    #[cfg(test)]
    pub(super) fn push(&mut self, packet: &[u8]) -> Result<(), TransmitQueueError> {
        self.push_at(packet, 0)
    }

    pub(super) fn push_at(
        &mut self,
        packet: &[u8],
        enqueued_at_ms: u64,
    ) -> Result<(), TransmitQueueError> {
        if packet.len() > LORA_MAX_PAYLOAD {
            return Err(TransmitQueueError::PacketTooLarge);
        }
        let record_bytes = HEADER_BYTES + packet.len();
        if record_bytes > self.storage.len().saturating_sub(self.used) {
            return Err(TransmitQueueError::Full);
        }

        let write = (self.head + self.used) % self.storage.len();
        let packet_length = u16::try_from(packet.len())
            .map_err(|_| TransmitQueueError::PacketTooLarge)?
            .to_le_bytes();
        Self::copy_into_ring(self.storage, write, &packet_length);
        Self::copy_into_ring(
            self.storage,
            (write + PACKET_LENGTH_BYTES) % self.storage.len(),
            &enqueued_at_ms.to_le_bytes(),
        );
        Self::copy_into_ring(
            self.storage,
            (write + HEADER_BYTES) % self.storage.len(),
            packet,
        );
        self.used += record_bytes;
        Ok(())
    }

    pub(super) const fn used_bytes(&self) -> usize {
        self.used
    }

    /// Diagnostic prefix only; never dequeues or refreshes the stored age.
    pub(super) fn front_prefix(&self, output: &mut [u8]) -> Option<(usize, usize)> {
        if self.is_empty() {
            return None;
        }
        let mut length = [0; PACKET_LENGTH_BYTES];
        Self::copy_from_ring(self.storage, self.head, &mut length);
        let length = usize::from(u16::from_le_bytes(length));
        let copied = length.min(output.len());
        Self::copy_from_ring(
            self.storage,
            (self.head + HEADER_BYTES) % self.storage.len(),
            &mut output[..copied],
        );
        Some((length, copied))
    }

    pub(super) fn front_enqueued_at(&self) -> Option<u64> {
        if self.is_empty() {
            return None;
        }
        let mut bytes = [0; 8];
        Self::copy_from_ring(
            self.storage,
            (self.head + PACKET_LENGTH_BYTES) % self.storage.len(),
            &mut bytes,
        );
        Some(u64::from_le_bytes(bytes))
    }

    pub(super) fn discard_front(&mut self) {
        if self.is_empty() {
            return;
        }
        let mut bytes = [0; PACKET_LENGTH_BYTES];
        Self::copy_from_ring(self.storage, self.head, &mut bytes);
        let record_bytes = HEADER_BYTES + usize::from(u16::from_le_bytes(bytes));
        assert!(record_bytes <= self.used);
        self.head = (self.head + record_bytes) % self.storage.len();
        self.used -= record_bytes;
    }

    pub(super) fn pop(&mut self, packet: &mut [u8; LORA_MAX_PAYLOAD]) -> Option<usize> {
        if self.is_empty() {
            return None;
        }

        let mut encoded_length = [0u8; PACKET_LENGTH_BYTES];
        Self::copy_from_ring(self.storage, self.head, &mut encoded_length);
        let packet_length = usize::from(u16::from_le_bytes(encoded_length));
        let record_bytes = HEADER_BYTES + packet_length;
        assert!(packet_length <= LORA_MAX_PAYLOAD);
        assert!(record_bytes <= self.used);
        Self::copy_from_ring(
            self.storage,
            (self.head + HEADER_BYTES) % self.storage.len(),
            &mut packet[..packet_length],
        );
        self.head = (self.head + record_bytes) % self.storage.len();
        self.used -= record_bytes;
        Some(packet_length)
    }

    fn copy_into_ring(storage: &mut [u8], start: usize, bytes: &[u8]) {
        let first_len = bytes.len().min(storage.len() - start);
        storage[start..start + first_len].copy_from_slice(&bytes[..first_len]);
        storage[..bytes.len() - first_len].copy_from_slice(&bytes[first_len..]);
    }

    fn copy_from_ring(storage: &[u8], start: usize, bytes: &mut [u8]) {
        let bytes_len = bytes.len();
        let first_len = bytes_len.min(storage.len() - start);
        bytes[..first_len].copy_from_slice(&storage[start..start + first_len]);
        bytes[first_len..].copy_from_slice(&storage[..bytes_len - first_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lora::LORA_TX_QUEUE_BYTES;

    fn storage() -> [u8; LORA_TX_QUEUE_BYTES] {
        [0; LORA_TX_QUEUE_BYTES]
    }

    fn packet(length: usize, value: u8) -> std::vec::Vec<u8> {
        std::vec![value; length]
    }

    #[test]
    fn caller_sized_storage_sets_the_bounded_capacity() {
        let mut storage = [0; MAX_RECORD_BYTES];
        let mut queue = TransmitQueue::new(&mut storage);
        let packet = packet(LORA_MAX_PAYLOAD, 0xA5);

        assert!(queue.can_push_max_packet());
        queue.push(&packet).unwrap();
        assert!(!queue.can_push_max_packet());
        assert_eq!(queue.push(&[0x01]), Err(TransmitQueueError::Full));

        let mut output = [0; LORA_MAX_PAYLOAD];
        assert_eq!(queue.pop(&mut output), Some(LORA_MAX_PAYLOAD));
        assert_eq!(&output[..], packet.as_slice());
        assert!(queue.can_push_max_packet());
    }

    #[test]
    fn packets_leave_in_insertion_order_across_mixed_sizes() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        let expected = [packet(1, 1), packet(250, 2), packet(508, 3), packet(17, 4)];

        for packet in &expected {
            queue.push(packet).unwrap();
        }

        let mut output = [0; LORA_MAX_PAYLOAD];
        for packet in expected {
            let length = queue.pop(&mut output).unwrap();
            assert_eq!(&output[..length], packet);
        }
        assert!(queue.pop(&mut output).is_none());
    }

    #[test]
    fn packet_and_length_prefix_wrap_across_the_storage_end() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        for value in 0..11 {
            queue.push(&packet(LORA_MAX_PAYLOAD, value)).unwrap();
        }
        queue.push(&packet(435, 11)).unwrap();

        let mut output = [0; LORA_MAX_PAYLOAD];
        for _ in 0..12 {
            queue.pop(&mut output).unwrap();
        }
        assert_eq!(queue.head, LORA_TX_QUEUE_BYTES - 1);

        let wrapped = packet(508, 0xA5);
        queue.push_at(&wrapped, u64::MAX - 1).unwrap();
        assert_eq!(queue.front_enqueued_at(), Some(u64::MAX - 1));
        let mut prefix = [0; 102];
        assert_eq!(queue.front_prefix(&mut prefix), Some((508, 102)));
        assert_eq!(prefix, [0xA5; 102]);
        assert_eq!(queue.used_bytes(), 518);
        assert_eq!(queue.front_enqueued_at(), Some(u64::MAX - 1));
        assert_eq!(queue.storage[LORA_TX_QUEUE_BYTES - 1], 0xFC);
        assert_eq!(queue.storage[0], 0x01);
        let length = queue.pop(&mut output).unwrap();
        assert_eq!(&output[..length], wrapped);
    }

    #[test]
    fn exact_capacity_is_usable_and_one_more_record_is_rejected_without_mutation() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        for value in 0..11 {
            queue.push(&packet(LORA_MAX_PAYLOAD, value)).unwrap();
        }
        queue.push(&packet(436, 11)).unwrap();
        assert_eq!(queue.used, LORA_TX_QUEUE_BYTES);
        let before = queue.storage.to_vec();
        let before_head = queue.head;
        let before_used = queue.used;

        assert_eq!(queue.push(&[0xFF]), Err(TransmitQueueError::Full));
        assert_eq!(queue.storage, before.as_slice());
        assert_eq!((queue.head, queue.used), (before_head, before_used));
    }

    #[test]
    fn dequeue_reuses_capacity_after_wraparound() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        let mut output = [0; LORA_MAX_PAYLOAD];

        for value in 0..11 {
            queue.push(&packet(LORA_MAX_PAYLOAD, value)).unwrap();
        }
        for value in 0..6 {
            let length = queue.pop(&mut output).unwrap();
            assert_eq!(&output[..length], packet(LORA_MAX_PAYLOAD, value));
        }
        for value in 11..17 {
            queue.push(&packet(LORA_MAX_PAYLOAD, value)).unwrap();
        }
        for value in 6..17 {
            let length = queue.pop(&mut output).unwrap();
            assert_eq!(&output[..length], packet(LORA_MAX_PAYLOAD, value));
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn eleven_maximum_packets_fit_but_twelve_do_not() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        let maximum = [0x5A; LORA_MAX_PAYLOAD];

        for _ in 0..11 {
            queue.push(&maximum).unwrap();
        }
        assert_eq!(queue.push(&maximum), Err(TransmitQueueError::Full));
        assert!(!queue.can_push_max_packet());

        let mut output = [0; LORA_MAX_PAYLOAD];
        assert_eq!(queue.pop(&mut output), Some(LORA_MAX_PAYLOAD));
        assert!(queue.can_push_max_packet());
        assert_eq!(queue.push(&maximum), Ok(()));
    }

    #[test]
    fn twenty_three_250_byte_packets_fit() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        let packet = [0xA5; 250];

        for _ in 0..23 {
            assert_eq!(queue.push(&packet), Ok(()));
        }
        assert_eq!(queue.used, 23 * (HEADER_BYTES + packet.len()));
    }
    #[test]
    fn oversized_packet_is_rejected_without_consuming_queue_capacity() {
        let mut storage = storage();
        let mut queue = TransmitQueue::new(&mut storage);
        assert_eq!(
            queue.push(&[0; LORA_MAX_PAYLOAD + 1]),
            Err(TransmitQueueError::PacketTooLarge)
        );
        assert!(queue.is_empty());
        assert!(queue.can_push_max_packet());
    }
}
