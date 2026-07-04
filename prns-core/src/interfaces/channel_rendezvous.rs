#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WifiChannel(u16);

impl WifiChannel {
    pub const DEFAULT_SOCIAL: Self = Self(2_437);

    const TWO_POINT_FOUR_LO: u16 = 2_400;
    const TWO_POINT_FOUR_HI: u16 = 2_500;
    const FIVE_LO: u16 = 5_150;
    const FIVE_HI: u16 = 5_895;
    const SIX_LO: u16 = 5_925;
    const SIX_HI: u16 = 7_125;
    const DFS_LO: u16 = 5_250;
    const UNII3_LO: u16 = 5_730;

    #[must_use]
    pub const fn new(mhz: u16) -> Option<Self> {
        let in_two_point_four = mhz >= Self::TWO_POINT_FOUR_LO && mhz <= Self::TWO_POINT_FOUR_HI;
        let in_five = mhz >= Self::FIVE_LO && mhz <= Self::FIVE_HI;
        let in_six = mhz >= Self::SIX_LO && mhz <= Self::SIX_HI;
        if in_two_point_four || in_five || in_six {
            Some(Self(mhz))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_mhz(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn band(self) -> Band {
        if self.0 < Self::FIVE_LO {
            Band::TwoPointFour
        } else if self.0 < Self::SIX_LO {
            Band::Five
        } else {
            Band::Six
        }
    }

    #[must_use]
    pub const fn supports_colocated_group(self) -> bool {
        match self.band() {
            Band::TwoPointFour => true,
            Band::Five => self.0 < Self::DFS_LO || self.0 >= Self::UNII3_LO,
            Band::Six => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    TwoPointFour,
    Five,
    Six,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocialChannel(WifiChannel);

impl SocialChannel {
    pub const DEFAULT: Self = Self(WifiChannel::DEFAULT_SOCIAL);

    #[must_use]
    pub const fn new(channel: WifiChannel) -> Option<Self> {
        if channel.supports_colocated_group() {
            Some(Self(channel))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn channel(self) -> WifiChannel {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelCommitment {
    Anchored(WifiChannel),
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendezvousOutcome {
    StayOn(WifiChannel),
    RetuneTo(WifiChannel),
    SeekPeer,
    Incompatible,
}

#[must_use]
pub fn decide(
    mine: ChannelCommitment,
    peer: Option<ChannelCommitment>,
    default: SocialChannel,
) -> RendezvousOutcome {
    use ChannelCommitment::{Anchored, Free};
    use RendezvousOutcome::{Incompatible, RetuneTo, SeekPeer, StayOn};

    match (mine, peer) {
        (Free, None) => SeekPeer,
        (Free, Some(Free)) => RetuneTo(default.channel()),
        (Free, Some(Anchored(peer_channel))) => {
            if peer_channel.supports_colocated_group() {
                RetuneTo(peer_channel)
            } else {
                Incompatible
            }
        }
        (Anchored(my_channel), None) => StayOn(my_channel),
        (Anchored(my_channel), Some(Free)) => {
            if my_channel.supports_colocated_group() {
                StayOn(my_channel)
            } else {
                Incompatible
            }
        }
        (Anchored(my_channel), Some(Anchored(peer_channel))) => {
            if my_channel.as_mhz() == peer_channel.as_mhz() && my_channel.supports_colocated_group()
            {
                StayOn(my_channel)
            } else {
                Incompatible
            }
        }
    }
}

#[cfg(any(test, kani))]
#[must_use]
fn rendezvous_channel(outcome: RendezvousOutcome) -> Option<WifiChannel> {
    match outcome {
        RendezvousOutcome::StayOn(channel) | RendezvousOutcome::RetuneTo(channel) => Some(channel),
        RendezvousOutcome::SeekPeer | RendezvousOutcome::Incompatible => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChannelCommitment::{Anchored, Free};
    use RendezvousOutcome::{Incompatible, RetuneTo, SeekPeer, StayOn};

    fn channel(mhz: u16) -> WifiChannel {
        WifiChannel::new(mhz).unwrap()
    }

    #[test]
    fn the_rendezvous_table_resolves_every_commitment_pairing() {
        let social = SocialChannel::DEFAULT;
        let unii1 = channel(5_180);
        let dfs = channel(5_300);
        let six = channel(5_955);

        let cases = [
            (Free, None, SeekPeer),
            (Free, Some(Free), RetuneTo(social.channel())),
            (Free, Some(Anchored(unii1)), RetuneTo(unii1)),
            (Free, Some(Anchored(dfs)), Incompatible),
            (Free, Some(Anchored(six)), Incompatible),
            (Anchored(unii1), None, StayOn(unii1)),
            (Anchored(dfs), None, StayOn(dfs)),
            (Anchored(unii1), Some(Free), StayOn(unii1)),
            (Anchored(dfs), Some(Free), Incompatible),
            (Anchored(unii1), Some(Anchored(unii1)), StayOn(unii1)),
            (Anchored(six), Some(Anchored(six)), Incompatible),
            (Anchored(unii1), Some(Anchored(dfs)), Incompatible),
        ];

        for (mine, peer, expected) in cases {
            assert_eq!(
                decide(mine, peer, social),
                expected,
                "mine={mine:?} peer={peer:?}",
            );
        }
    }

    #[test]
    fn an_anchored_radio_only_ever_stays_or_declines_incompatible() {
        let social = SocialChannel::DEFAULT;
        let anchor = channel(5_180);
        let peers = [
            None,
            Some(Free),
            Some(Anchored(anchor)),
            Some(Anchored(channel(5_240))),
            Some(Anchored(channel(5_300))),
        ];
        for peer in peers {
            let outcome = decide(Anchored(anchor), peer, social);
            assert!(
                matches!(outcome, StayOn(_) | Incompatible),
                "an anchored radio never retunes or seeks: got {outcome:?} for peer {peer:?}",
            );
        }
    }

    #[test]
    fn a_free_radio_only_ever_retunes_seeks_or_declines_incompatible() {
        let social = SocialChannel::DEFAULT;
        let peers = [
            None,
            Some(Free),
            Some(Anchored(channel(5_180))),
            Some(Anchored(channel(5_300))),
            Some(Anchored(channel(5_955))),
        ];
        for peer in peers {
            let outcome = decide(Free, peer, social);
            assert!(
                matches!(outcome, RetuneTo(_) | SeekPeer | Incompatible),
                "a free radio never stays on a channel it does not hold: got {outcome:?}",
            );
        }
    }

    #[test]
    fn both_radios_independently_converge_on_one_channel_or_both_decline() {
        let social = SocialChannel::DEFAULT;
        let commitments = [
            Free,
            Anchored(channel(2_412)),
            Anchored(channel(2_437)),
            Anchored(channel(5_180)),
            Anchored(channel(5_240)),
            Anchored(channel(5_300)),
            Anchored(channel(5_745)),
            Anchored(channel(5_955)),
        ];
        for mine in commitments {
            for peer in commitments {
                let from_me = decide(mine, Some(peer), social);
                let from_peer = decide(peer, Some(mine), social);
                assert_ne!(from_me, SeekPeer, "a learned peer never leaves us seeking");
                assert_ne!(
                    from_peer, SeekPeer,
                    "a learned peer never leaves us seeking"
                );
                assert_eq!(
                    rendezvous_channel(from_me),
                    rendezvous_channel(from_peer),
                    "mine={mine:?} peer={peer:?} must converge or both decline",
                );
            }
        }
    }

    #[test]
    fn an_unknown_peer_makes_a_free_radio_seek_rather_than_assume_the_default() {
        assert_eq!(decide(Free, None, SocialChannel::DEFAULT), SeekPeer);
    }

    #[test]
    fn a_dfs_or_six_gigahertz_anchor_cannot_host_a_colocated_group() {
        let social = SocialChannel::DEFAULT;
        for mhz in [5_300, 5_500, 5_700, 5_955, 6_175] {
            let anchor = channel(mhz);
            assert_eq!(
                decide(Anchored(anchor), Some(Free), social),
                Incompatible,
                "{mhz} MHz cannot host a co-located group",
            );
            assert_eq!(
                decide(Free, Some(Anchored(anchor)), social),
                Incompatible,
                "a free radio cannot follow a peer onto {mhz} MHz",
            );
            assert_eq!(
                decide(Anchored(anchor), Some(Anchored(anchor)), social),
                Incompatible,
                "two radios sharing {mhz} MHz still cannot host a group there",
            );
        }
    }

    #[test]
    fn channel_frequencies_classify_into_their_bands() {
        assert_eq!(channel(2_412).band(), Band::TwoPointFour);
        assert_eq!(channel(2_484).band(), Band::TwoPointFour);
        assert_eq!(channel(5_180).band(), Band::Five);
        assert_eq!(channel(5_895).band(), Band::Five);
        assert_eq!(channel(5_955).band(), Band::Six);
        assert_eq!(channel(7_115).band(), Band::Six);
    }

    #[test]
    fn dfs_and_six_gigahertz_channels_report_no_colocated_group() {
        assert!(channel(2_437).supports_colocated_group());
        assert!(channel(5_180).supports_colocated_group());
        assert!(channel(5_240).supports_colocated_group());
        assert!(!channel(5_260).supports_colocated_group());
        assert!(!channel(5_700).supports_colocated_group());
        assert!(channel(5_745).supports_colocated_group());
        assert!(channel(5_825).supports_colocated_group());
        assert!(!channel(5_955).supports_colocated_group());
    }

    #[test]
    fn channel_rejects_frequencies_outside_the_wifi_bands() {
        for mhz in [0, 2_399, 2_501, 5_000, 5_149, 5_896, 5_924, 7_126, u16::MAX] {
            assert_eq!(WifiChannel::new(mhz), None, "{mhz} MHz is not a Wi-Fi band");
        }
        for mhz in [2_400, 2_437, 5_150, 5_895, 5_925, 7_125] {
            assert!(WifiChannel::new(mhz).is_some(), "{mhz} MHz is a Wi-Fi band");
        }
    }

    #[test]
    fn the_social_channel_refuses_a_dfs_or_six_gigahertz_default() {
        assert!(SocialChannel::new(channel(2_437)).is_some());
        assert!(SocialChannel::new(channel(5_180)).is_some());
        assert_eq!(SocialChannel::new(channel(5_300)), None);
        assert_eq!(SocialChannel::new(channel(5_955)), None);
        assert_eq!(
            SocialChannel::DEFAULT.channel(),
            WifiChannel::DEFAULT_SOCIAL
        );
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    fn arbitrary_commitment() -> ChannelCommitment {
        if kani::any() {
            ChannelCommitment::Free
        } else {
            let mhz: u16 = kani::any();
            match WifiChannel::new(mhz) {
                Some(channel) => ChannelCommitment::Anchored(channel),
                None => ChannelCommitment::Free,
            }
        }
    }

    fn arbitrary_peer() -> Option<ChannelCommitment> {
        if kani::any() {
            None
        } else {
            Some(arbitrary_commitment())
        }
    }

    #[kani::proof]
    fn an_anchored_radio_never_retunes_and_never_seeks() {
        let mhz: u16 = kani::any();
        if let Some(anchor) = WifiChannel::new(mhz) {
            let outcome = decide(
                ChannelCommitment::Anchored(anchor),
                arbitrary_peer(),
                SocialChannel::DEFAULT,
            );
            assert!(matches!(
                outcome,
                RendezvousOutcome::StayOn(_) | RendezvousOutcome::Incompatible
            ));
        }
    }

    #[kani::proof]
    fn a_free_radio_never_stays() {
        let outcome = decide(
            ChannelCommitment::Free,
            arbitrary_peer(),
            SocialChannel::DEFAULT,
        );
        assert!(matches!(
            outcome,
            RendezvousOutcome::RetuneTo(_)
                | RendezvousOutcome::SeekPeer
                | RendezvousOutcome::Incompatible
        ));
    }

    #[kani::proof]
    fn two_radios_that_have_learned_each_other_always_converge() {
        let mine = arbitrary_commitment();
        let peer = arbitrary_commitment();
        let default = SocialChannel::DEFAULT;
        let from_me = decide(mine, Some(peer), default);
        let from_peer = decide(peer, Some(mine), default);
        assert!(!matches!(from_me, RendezvousOutcome::SeekPeer));
        assert!(!matches!(from_peer, RendezvousOutcome::SeekPeer));
        assert_eq!(rendezvous_channel(from_me), rendezvous_channel(from_peer));
    }

    #[kani::proof]
    fn a_channel_that_cannot_host_a_group_always_yields_incompatible() {
        let mhz: u16 = kani::any();
        if let Some(anchor) = WifiChannel::new(mhz) {
            if !anchor.supports_colocated_group() {
                assert_eq!(
                    decide(
                        ChannelCommitment::Anchored(anchor),
                        Some(ChannelCommitment::Free),
                        SocialChannel::DEFAULT,
                    ),
                    RendezvousOutcome::Incompatible
                );
                assert_eq!(
                    decide(
                        ChannelCommitment::Free,
                        Some(ChannelCommitment::Anchored(anchor)),
                        SocialChannel::DEFAULT,
                    ),
                    RendezvousOutcome::Incompatible
                );
            }
        }
    }
}
