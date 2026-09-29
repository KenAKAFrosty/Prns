use super::*;

#[test]
fn early_sender_and_unselected_peer_receive_until_the_last_send_settles() {
    let (mut fleet, status, notify) = fixture();
    let ready = Rc::new(Cell::new(false));
    let mut members = [
        Some(member(0, Incoming::Frame(vec![1, 2]), Sending::Ready)),
        Some(member(1, Incoming::Frame(vec![3]), Sending::Failed)),
        Some(member(2, Incoming::Pending, Sending::Gated(ready.clone()))),
    ];
    let states = [
        MemberSelection::Send,
        MemberSelection::ReceiveOnly,
        MemberSelection::Send,
    ]
    .map(MemberTransferState::new);
    let mut bufs = [[0; BLE_HW_MTU]; PEERS];
    {
        let mut running = pin!(send_members(
            &mut members,
            &states,
            b"send",
            &mut bufs,
            &mut fleet,
            &status
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(running.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            core::array::from_fn::<_, PEERS, _>(|i| (
                status.member(i).tx_bytes(),
                status.member(i).rx_bytes()
            )),
            [(4, 2), (0, 1), (0, 0)]
        );
        assert_eq!(notify.try_receive(), Ok(InterfaceId::new([0; 8])));
        assert_eq!(notify.try_receive(), Ok(InterfaceId::new([1; 8])));
        assert!(notify.try_receive().is_err());
        ready.set(true);
        // Earlier slots must observe the final slot settling without another external wake.
        assert!(running.as_mut().poll(&mut context).is_ready());
    }
    assert_eq!(
        members
            .each_ref()
            .map(|m| m.as_ref().unwrap().sink.starts.get()),
        [1, 0, 1]
    );
    assert_eq!(
        states.each_ref().map(|s| s.send.get()),
        [SendState::Sent, SendState::NotSelected, SendState::Sent]
    );
    assert_eq!(
        states.each_ref().map(MemberTransferState::needs_retirement),
        [false; PEERS]
    );
    let mut expected = [[0; BLE_HW_MTU]; PEERS];
    expected[0][..2].copy_from_slice(&[1, 2]);
    expected[1][0] = 3;
    assert_eq!(bufs, expected);
}

#[test]
fn fanout_completion_waits_for_unselected_and_early_sender_forwarding() {
    for finish_forwarding in [false, true] {
        let (mut fleet, status, notify) = fixture();
        let placeholder = InterfaceId::new([99; 8]);
        for _ in 0..PEERS {
            notify.try_send(placeholder).unwrap();
        }
        let ready = Rc::new(Cell::new(false));
        let mut members = [
            Some(member(0, Incoming::Frame(vec![1]), Sending::Ready)),
            Some(member(1, Incoming::Frame(vec![2]), Sending::Failed)),
            Some(member(2, Incoming::Pending, Sending::Gated(ready.clone()))),
        ];
        let states = [
            MemberSelection::Send,
            MemberSelection::ReceiveOnly,
            MemberSelection::Send,
        ]
        .map(MemberTransferState::new);
        let mut bufs = [[0; BLE_HW_MTU]; PEERS];
        {
            let mut running = pin!(send_members(
                &mut members,
                &states,
                b"send",
                &mut bufs,
                &mut fleet,
                &status
            ));
            let mut context = Context::from_waker(Waker::noop());
            assert!(running.as_mut().poll(&mut context).is_pending());
            ready.set(true);
            assert!(running.as_mut().poll(&mut context).is_pending());
            assert_eq!(
                states.each_ref().map(|s| s.send.get()),
                [SendState::Sent, SendState::NotSelected, SendState::Sent]
            );
            assert_eq!(
                states.each_ref().map(MemberTransferState::needs_retirement),
                [true, true, false]
            );
            if finish_forwarding {
                for _ in 0..PEERS {
                    assert_eq!(notify.try_receive(), Ok(placeholder));
                }
                assert!(running.as_mut().poll(&mut context).is_ready());
            }
        }
        assert_eq!(
            core::array::from_fn::<_, PEERS, _>(|i| status.member(i).tx_bytes()),
            [4, 0, 4]
        );
        if finish_forwarding {
            assert_eq!(
                states.each_ref().map(MemberTransferState::needs_retirement),
                [false; PEERS]
            );
            assert_eq!(
                core::array::from_fn::<_, PEERS, _>(|i| status.member(i).rx_bytes()),
                [1, 1, 0]
            );
            assert_eq!(notify.try_receive(), Ok(InterfaceId::new([0; 8])));
            assert_eq!(notify.try_receive(), Ok(InterfaceId::new([1; 8])));
            assert!(notify.try_receive().is_err());
        } else {
            assert_eq!(
                states.each_ref().map(MemberTransferState::needs_retirement),
                [true, true, false]
            );
            assert_eq!(
                core::array::from_fn::<_, PEERS, _>(|i| status.member(i).rx_bytes()),
                [0; PEERS]
            );
        }
    }
}

#[test]
fn receive_failure_retires_unselected_or_finished_sender_without_poisoning_other_sends() {
    for selection in [MemberSelection::ReceiveOnly, MemberSelection::Send] {
        for incoming in [
            Incoming::InvalidLength,
            Incoming::Closed,
            Incoming::Frame(vec![7; FRAME + 1]),
        ] {
            let (mut fleet, status, notify) = fixture();
            let mut members = [
                Some(member(0, incoming, Sending::Ready)),
                Some(member(1, Incoming::Frame(vec![2]), Sending::AfterReceive)),
                None,
            ];
            let states = [
                selection,
                MemberSelection::Send,
                MemberSelection::ReceiveOnly,
            ]
            .map(MemberTransferState::new);
            let mut bufs = [[0; BLE_HW_MTU]; PEERS];
            let mut running = pin!(send_members(
                &mut members,
                &states,
                b"send",
                &mut bufs,
                &mut fleet,
                &status
            ));
            assert!(running
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready());
            assert_eq!(
                states.each_ref().map(MemberTransferState::needs_retirement),
                [true, false, false]
            );
            let expected_tx = if selection == MemberSelection::Send {
                4
            } else {
                0
            };
            assert_eq!(
                core::array::from_fn::<_, PEERS, _>(|i| (
                    status.member(i).tx_bytes(),
                    status.member(i).rx_bytes()
                )),
                [(expected_tx, 0), (4, 1), (0, 0)]
            );
            assert_eq!(notify.try_receive(), Ok(InterfaceId::new([1; 8])));
            assert!(notify.try_receive().is_err());
        }
    }
}

#[test]
fn absent_last_recipient_settles_and_empty_selection_does_not_read_or_send() {
    for selection in [MemberSelection::Send, MemberSelection::ReceiveOnly] {
        let (mut fleet, status, notify) = fixture();
        let mut members = [
            Some(member(0, Incoming::Frame(vec![1]), Sending::Ready)),
            None,
            None,
        ];
        let states =
            [selection, MemberSelection::ReceiveOnly, selection].map(MemberTransferState::new);
        let mut bufs = [[0; BLE_HW_MTU]; PEERS];
        {
            let mut running = pin!(send_members(
                &mut members,
                &states,
                b"send",
                &mut bufs,
                &mut fleet,
                &status
            ));
            assert!(running
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready());
        }
        match selection {
            MemberSelection::Send => {
                assert_eq!(
                    states.each_ref().map(|s| s.send.get()),
                    [SendState::Sent, SendState::NotSelected, SendState::Failed]
                );
                assert_eq!(notify.try_receive(), Ok(InterfaceId::new([0; 8])));
            }
            MemberSelection::ReceiveOnly => {
                assert_eq!(
                    (members[0].as_ref().unwrap().sink.starts.get(), bufs),
                    (0, [[0; BLE_HW_MTU]; PEERS])
                );
                assert_eq!(
                    states.each_ref().map(MemberTransferState::needs_retirement),
                    [false; PEERS]
                );
            }
        }
        assert!(notify.try_receive().is_err());
    }
}
