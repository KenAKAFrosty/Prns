use super::*;

impl Lab<'_> {
    pub fn watch(
        &mut self,
        link: LinkId,
        stream_id: personal_rns::runtime::StreamId,
    ) -> RemoteControlInterfaceWatch {
        self.watch_from(CONTROLLER, link, stream_id)
    }
    pub fn watch_from(
        &mut self,
        controller: usize,
        link: LinkId,
        stream_id: personal_rns::runtime::StreamId,
    ) -> RemoteControlInterfaceWatch {
        let handle = self.nodes[controller].handle.clone();
        let task = self.insert(async move {
            let (watch, _) = handle
                .remote_control(link)
                .watch_interfaces(stream_id)
                .await
                .expect("watch admission");
            Event::WatchOpened(watch)
        });
        let mut completed = self.settle();
        assert_eq!(completed.len(), 1, "one watch admission");
        let (found, Event::WatchOpened(watch)) = completed.remove(0) else {
            unreachable!("admitted watch");
        };
        assert_eq!(found, task);
        watch
    }
    pub fn read_watch(&mut self, mut watch: RemoteControlInterfaceWatch) -> ManualTaskId {
        self.insert(async move {
            let result = watch.next_event().await;
            Event::WatchRead { watch, result }
        })
    }
}

pub fn watch_result(
    task: ManualTaskId,
    mut completed: Vec<(ManualTaskId, Event)>,
) -> (
    RemoteControlInterfaceWatch,
    Result<RemoteControlStreamEvent, RemoteControlWatchReadError>,
) {
    assert_eq!(completed.len(), 1, "one watch read completion");
    let (found, Event::WatchRead { watch, result }) = completed.remove(0) else {
        unreachable!("watch read");
    };
    assert_eq!(found, task);
    (watch, result)
}
