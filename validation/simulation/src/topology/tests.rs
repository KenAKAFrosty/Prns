use super::*;

fn isolated_nodes() -> Topology<u16> {
    let mut topology = Topology::new(TopologyConfig::Explicit {
        max_neighbors: NonZeroUsize::MIN,
    });
    for node in 0..4 {
        topology.attach(node);
    }
    topology
}

#[test]
fn links_are_symmetric_canonical_and_bounded_at_both_ends() {
    for (first, second, full) in [(0, 2, 0), (2, 1, 1)] {
        let mut actual = isolated_nodes();
        assert_eq!(
            actual.set_reachability(0, 1, Reachability::Reachable),
            Ok(TopologyMutation::Applied)
        );
        assert_eq!(
            actual.set_reachability(1, 0, Reachability::Reachable),
            Ok(TopologyMutation::Unchanged)
        );
        let mut expected = isolated_nodes();
        expected.neighbors.insert(0, BTreeSet::from([1]));
        expected.neighbors.insert(1, BTreeSet::from([0]));
        assert_eq!(actual, expected);
        assert_eq!(
            actual.set_reachability(first, second, Reachability::Reachable),
            Err(TopologyError::NeighborCapacityReached {
                node: full,
                maximum: 1
            })
        );
        assert_eq!(actual, expected);
        assert_eq!(
            actual.set_reachability(1, 0, Reachability::Isolated),
            Ok(TopologyMutation::Applied)
        );
        assert_eq!(
            actual.set_reachability(0, 1, Reachability::Isolated),
            Ok(TopologyMutation::Unchanged)
        );
        assert_eq!(actual, isolated_nodes());
    }
}

#[test]
fn refusal_and_duplicate_attachment_leave_the_whole_graph_unchanged() {
    let mut topology = isolated_nodes();
    for (first, second, error) in [
        (0, 0, TopologyError::SelfLink(0)),
        (0, 4, TopologyError::UnknownNode(4)),
        (4, 0, TopologyError::UnknownNode(4)),
    ] {
        assert_eq!(
            topology.set_reachability(first, second, Reachability::Reachable),
            Err(error)
        );
        assert_eq!(topology, isolated_nodes());
    }
    assert_eq!(
        topology.set_reachability(0, 1, Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    topology.attach(0);
    assert_eq!(topology.neighbors(0).collect::<Vec<_>>(), vec![1]);
    topology.detach(0);
    topology.detach(0);
    let mut expected = isolated_nodes();
    let _ = expected.neighbors.remove(&0);
    assert_eq!(topology, expected);
    topology.attach(0);
    assert_eq!(topology, isolated_nodes());
}

#[test]
fn fully_connected_mode_has_no_stored_edges_or_implicit_mutations() {
    let mut topology = Topology::new(TopologyConfig::FullyConnected);
    for node in [3, 1, 2] {
        topology.attach(node);
    }
    assert_eq!(topology.neighbors(2).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(topology.neighbors(4).collect::<Vec<_>>(), Vec::<u16>::new());
    assert_eq!(
        topology.set_reachability(1, 2, Reachability::Isolated),
        Err(TopologyError::ImplicitTopology)
    );
    assert!(topology.reaches(1, 3));
    assert!(!topology.reaches(1, 1));
    assert!(!topology.reaches(1, 4));
    topology.detach(3);
    assert_eq!(
        topology,
        Topology {
            config: TopologyConfig::FullyConnected,
            neighbors: BTreeMap::from([(1, BTreeSet::new()), (2, BTreeSet::new())]),
        }
    );
}
