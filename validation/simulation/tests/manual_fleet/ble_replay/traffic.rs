use super::*;
use personal_rns::{routing::links::LinkId, wire::DestinationHash};

pub(super) fn establish(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::LiveNode],
    targets: impl IntoIterator<Item = (usize, DestinationHash)>,
) -> BTreeMap<usize, LinkId> {
    let tasks: BTreeMap<_, _> = targets
        .into_iter()
        .map(|(node, remote)| {
            let handle = nodes[node].control.handle.clone();
            let task = runner
                .insert(async move {
                    let link = handle
                        .establish_link(remote)
                        .await
                        .unwrap_or_else(|error| unreachable!("fleet link: {error:?}"));
                    Completion::Linked { node, link }
                })
                .unwrap_or_else(|error| unreachable!("link actor: {error}"));
            (task, node)
        })
        .collect();
    assert_eq!(runner.task_count(), nodes.len() + tasks.len());
    let completed = settle(runner);
    assert_eq!(completed.len(), tasks.len());
    let mut links = BTreeMap::new();
    for (task, completion) in completed {
        let Completion::Linked { node, link } = completion else {
            unreachable!("only link actors finish")
        };
        assert_eq!(tasks.get(&task), Some(&node));
        assert!(links.insert(node, link).is_none());
    }
    links
}

pub(super) fn exchange(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::LiveNode],
    links: &BTreeMap<usize, LinkId>,
    marker: u8,
) -> Vec<(usize, Vec<u8>)> {
    let mut expected = BTreeMap::new();
    for (&node, &link) in links {
        let mut bytes = vec![marker; 256];
        bytes[0] = node as u8;
        let task = request(
            runner,
            node,
            nodes[node].control.handle.clone(),
            link,
            bytes.clone(),
        );
        expected.insert(
            task,
            Completion::Response {
                node,
                bytes: bytes.clone(),
            },
        );
    }
    assert_eq!(runner.task_count(), nodes.len() + links.len());
    let completed: BTreeMap<_, _> = settle(runner).into_iter().collect();
    assert_eq!(completed, expected);
    let mut responses: Vec<_> = completed
        .into_values()
        .map(|completion| {
            let Completion::Response { node, bytes } = completion else {
                unreachable!("only response actors finish")
            };
            (node, bytes)
        })
        .collect();
    responses.sort_by_key(|(node, _)| *node);
    responses
}
