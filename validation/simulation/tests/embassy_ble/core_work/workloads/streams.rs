use super::*;

pub fn byte_stream(triple: &mut Triple<'_, '_>, marker: u8) {
    triple.reconnect(PRIMARY);
    let (Handle::Tokio(controller), Handle::Tokio(target)) =
        (&triple.nodes[PRIMARY].handle, &triple.nodes[TARGET].handle)
    else {
        overlap(triple, marker);
        return;
    };
    let controller = controller.clone();
    let target = target.clone();
    let link = triple.app_links[0];
    triple.complete(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let stream = StreamId::new(7).expect("data stream");
        let mut reader = target
            .try_byte_stream_reader(link, stream)
            .await
            .expect("reader registration");
        assert!(matches!(
            target.try_byte_stream_reader(link, stream).await,
            Err(StreamReaderRegistrationError::AlreadyRegistered)
        ));
        let mut writer = controller.byte_stream_writer(link, stream);
        let payload = vec![marker; 768];
        let (write, read) = tokio::join!(
            async {
                writer.write_all(&payload).await?;
                writer.shutdown().await
            },
            async {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes).await?;
                Ok::<_, std::io::Error>(bytes)
            }
        );
        write.expect("stream sent");
        assert_eq!(read.expect("stream read"), payload);
    });
    healthy(triple);
}
