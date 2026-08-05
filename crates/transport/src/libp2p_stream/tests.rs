// The first two tests come from upstream's `protocols/stream/tests/lib.rs`; the remaining tests
// cover the local reliability patches described in `mod.rs`.

use std::{
    future::Future as _,
    io,
    task::{Context, Poll},
    time::Duration,
};

use futures::{
    future::join_all, task::noop_waker, AsyncReadExt as _, AsyncWriteExt as _, StreamExt as _,
};
use libp2p_identity::PeerId;
use libp2p_swarm::{
    behaviour::DialFailure, ConnectionId, DialError, FromSwarm, NetworkBehaviour, StreamProtocol,
    Swarm, ToSwarm,
};
use libp2p_swarm_test::SwarmExt as _;

use crate::{libp2p_stream as stream, libp2p_stream::OpenStreamError};

const PROTOCOL: StreamProtocol = StreamProtocol::new("/test");

#[tokio::test]
async fn dropping_incoming_streams_deregisters() {
    let mut swarm1 = Swarm::new_ephemeral_tokio(|_| stream::Behaviour::new());
    let mut swarm2 = Swarm::new_ephemeral_tokio(|_| stream::Behaviour::new());

    let mut control = swarm1.behaviour().new_control();
    let mut incoming = swarm2.behaviour().new_control().accept(PROTOCOL).unwrap();

    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let swarm2_peer_id = *swarm2.local_peer_id();

    let handle = tokio::spawn(async move {
        while let Some((_, mut stream)) = incoming.next().await {
            stream.write_all(&[42]).await.unwrap();
            stream.close().await.unwrap();
        }
    });
    tokio::spawn(swarm1.loop_on_next());
    tokio::spawn(swarm2.loop_on_next());

    let mut stream = control.open_stream(swarm2_peer_id, PROTOCOL).await.unwrap();

    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!([42], buf);

    handle.abort();
    let _ = handle.await;

    let error = control.open_stream(swarm2_peer_id, PROTOCOL).await.unwrap_err();
    assert!(matches!(error, OpenStreamError::UnsupportedProtocol(_)));
}

#[tokio::test]
async fn dial_errors_are_propagated() {
    let swarm1 = Swarm::new_ephemeral_tokio(|_| stream::Behaviour::new());

    let mut control = swarm1.behaviour().new_control();
    tokio::spawn(swarm1.loop_on_next());

    let error = control.open_stream(PeerId::random(), PROTOCOL).await.unwrap_err();

    let OpenStreamError::Io(e) = error else {
        panic!("Unexpected error: {error}")
    };

    assert_eq!(e.kind(), io::ErrorKind::NotConnected);
    assert_eq!("Dial error: no addresses for peer.", e.to_string());
}

#[test]
fn concurrent_open_streams_queue_every_distinct_dial() {
    let mut behaviour = stream::Behaviour::new();
    let mut control1 = behaviour.new_control();
    let mut control2 = behaviour.new_control();
    let mut open1 = Box::pin(control1.open_stream(PeerId::random(), PROTOCOL));
    let mut open2 = Box::pin(control2.open_stream(PeerId::random(), PROTOCOL));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(open1.as_mut().poll(&mut cx).is_pending());
    assert!(open2.as_mut().poll(&mut cx).is_pending());

    assert!(matches!(
        NetworkBehaviour::poll(&mut behaviour, &mut cx),
        Poll::Ready(ToSwarm::Dial { .. })
    ));
    assert!(matches!(
        NetworkBehaviour::poll(&mut behaviour, &mut cx),
        Poll::Ready(ToSwarm::Dial { .. })
    ));
    assert!(NetworkBehaviour::poll(&mut behaviour, &mut cx).is_pending());
}

#[test]
fn concurrent_open_streams_to_same_peer_queue_one_dial() {
    let mut behaviour = stream::Behaviour::new();
    let peer = PeerId::random();
    let mut control1 = behaviour.new_control();
    let mut control2 = behaviour.new_control();
    let mut open1 = Box::pin(control1.open_stream(peer, PROTOCOL));
    let mut open2 = Box::pin(control2.open_stream(peer, PROTOCOL));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(open1.as_mut().poll(&mut cx).is_pending());
    assert!(open2.as_mut().poll(&mut cx).is_pending());

    assert!(matches!(
        NetworkBehaviour::poll(&mut behaviour, &mut cx),
        Poll::Ready(ToSwarm::Dial { .. })
    ));
    assert!(NetworkBehaviour::poll(&mut behaviour, &mut cx).is_pending());
}

#[tokio::test]
async fn accept_with_capacity_buffers_unpolled_inbound_streams() {
    const STREAM_COUNT: usize = 4;

    let mut client = Swarm::new_ephemeral_tokio(|_| stream::Behaviour::new());
    let mut server = Swarm::new_ephemeral_tokio(|_| stream::Behaviour::new());
    let client_peer = *client.local_peer_id();
    let server_peer = *server.local_peer_id();
    let control = client.behaviour().new_control();
    let mut incoming = server
        .behaviour()
        .new_control()
        .accept_with_capacity(PROTOCOL, STREAM_COUNT)
        .unwrap();

    server.listen().with_memory_addr_external().await;
    client.connect(&mut server).await;

    tokio::spawn(client.loop_on_next());
    tokio::spawn(server.loop_on_next());

    let opens = (0..STREAM_COUNT).map(|_| {
        let mut control = control.clone();
        async move { control.open_stream(server_peer, PROTOCOL).await }
    });
    let opened = tokio::time::timeout(Duration::from_secs(5), join_all(opens))
        .await
        .expect("opening buffered streams timed out");
    let _streams: Vec<_> = opened.into_iter().map(Result::unwrap).collect();

    tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..STREAM_COUNT {
            let (peer, _stream) = incoming.next().await.expect("incoming streams ended");
            assert_eq!(peer, client_peer);
        }
    })
    .await
    .expect("buffered inbound streams were dropped");
}

#[tokio::test]
async fn aborted_dial_is_propagated() {
    assert_terminal_dial_error_is_propagated(DialError::Aborted).await;
}

#[tokio::test]
async fn local_peer_dial_error_is_propagated() {
    assert_terminal_dial_error_is_propagated(DialError::LocalPeerId {
        address: "/memory/1".parse().unwrap(),
    })
    .await;
}

#[tokio::test]
async fn control_outliving_behaviour_returns_error() {
    let behaviour = stream::Behaviour::new();
    let mut control = behaviour.new_control();
    drop(behaviour);

    let error = tokio::time::timeout(
        Duration::from_secs(1),
        control.open_stream(PeerId::random(), PROTOCOL),
    )
    .await
    .expect("open_stream hung after its behaviour was dropped")
    .unwrap_err();

    let OpenStreamError::Io(error) = error else {
        panic!("unexpected error: {error}")
    };
    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    assert_eq!(error.to_string(), "stream behaviour is no longer running");
}

async fn assert_terminal_dial_error_is_propagated(dial_error: DialError) {
    let mut behaviour = stream::Behaviour::new();
    let peer = PeerId::random();
    let mut control = behaviour.new_control();
    let mut open = Box::pin(control.open_stream(peer, PROTOCOL));
    let expected_reason = dial_error.to_string();

    {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(open.as_mut().poll(&mut cx).is_pending());
    }

    NetworkBehaviour::on_swarm_event(
        &mut behaviour,
        FromSwarm::DialFailure(DialFailure {
            peer_id: Some(peer),
            error: &dial_error,
            connection_id: ConnectionId::new_unchecked(1),
        }),
    );

    let error = tokio::time::timeout(Duration::from_secs(1), open)
        .await
        .expect("terminal dial error was not propagated")
        .unwrap_err();
    let OpenStreamError::Io(error) = error else {
        panic!("unexpected error: {error}")
    };

    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    assert_eq!(error.to_string(), expected_reason);
}
