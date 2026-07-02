use crate::screen::Card;

pub trait CardStore {
    fn clear(&mut self);

    fn try_push(&mut self, card: Card) -> Result<(), Card>;

    fn as_slice(&self) -> &[Card];

    fn as_mut_slice(&mut self) -> &mut [Card];

    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

impl<const N: usize> CardStore for heapless::Vec<Card, N> {
    fn clear(&mut self) {
        heapless::Vec::clear(self);
    }

    fn try_push(&mut self, card: Card) -> Result<(), Card> {
        heapless::Vec::push(self, card)
    }

    fn as_slice(&self) -> &[Card] {
        self
    }

    fn as_mut_slice(&mut self) -> &mut [Card] {
        self
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{card_label, CardKind, Liveness};
    use personal_rns::interfaces::InterfaceId;

    fn card(tag: u8) -> Card {
        Card {
            id: InterfaceId::new([tag, 0, 0, 0, 0, 0, 0, 0]),
            kind: CardKind::LoRa,
            label: card_label("test"),
            selected: false,
            liveness: Liveness::Live,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        }
    }

    #[test]
    fn bounded_store_hands_the_card_back_when_full() {
        let mut store: heapless::Vec<Card, 2> = heapless::Vec::new();
        assert!(store.try_push(card(1)).is_ok());
        assert!(store.try_push(card(2)).is_ok());
        assert_eq!(store.len(), 2);

        match store.try_push(card(3)) {
            Err(rejected) => assert_eq!(rejected.id, card(3).id),
            Ok(()) => panic!("a full store must reject the card, not accept it"),
        }
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn clear_empties_the_store() {
        let mut store: heapless::Vec<Card, 2> = heapless::Vec::new();
        let _ = store.try_push(card(1));
        assert!(!store.is_empty());

        store.clear();
        assert!(store.is_empty());
        assert!(store.as_slice().is_empty());
    }
}
