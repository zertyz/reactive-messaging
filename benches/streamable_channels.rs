//! Compares the performance of channels that produce `Streams` (`tokio` & `futures`, but not `std` nor `crossbeam`)
//! with `reactive-mutiny`'s `Uni`s
//!
//! # Analysis 2023-06-01
//!
//!   `reactive-mutiny`'s Atomic is the winner on all tests, for a variety of Intel, AMD and ARM cpus -- sometimes winning by 2x.
//!
//! Out of the results here, it was decided that the `reactive-mutiny`'s Atomic Channel will be used instead of Tokio's
//!

use criterion::{
    criterion_group,
    criterion_main,
    Criterion,
    BenchmarkGroup,
    measurement::WallTime,
};
use std::sync::{
        Arc,
        atomic::{
            AtomicBool,
            AtomicU8,
            Ordering::Relaxed,
        }
    };
use reactive_mutiny::prelude::{ChannelCommon, ChannelUni, ChannelProducer};


// Low level measurements on how fast Stream items can be produced
//////////////////////////////////////////////////////////////////

/// Represents a reasonably sized message, similar to production needs
#[derive(Debug)]
struct MessageType {
    _data:  [u8; 255],
}
impl Default for MessageType {
    fn default() -> Self {
        MessageType { _data: [0; 255] }
    }
}

type ItemType = MessageType;
const BUFFER_SIZE: usize = 1<<14;


/// Benchmarks the same-thread latency, which is measured by the time it takes to send and receive back a single element
fn bench_same_thread_latency(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Same-thread LATENCY");

    let bench_id = "reactive-mutiny's Atomic Stream";
    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        while !atomic_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {}
        while futures::executor::block_on(atomic_stream.next()).is_none() {}
    }));

    let bench_id = "reactive-mutiny's FullSync Stream";
    let full_sync_channel = reactive_mutiny::uni::channels::movable::full_sync::FullSync::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut full_sync_stream, _) = full_sync_channel.create_stream();
    let full_sync_sender = full_sync_channel;
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        while !full_sync_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {}
        while futures::executor::block_on(full_sync_stream.next()).is_none() {}
    }));

    let bench_id = "Tokio MPSC Stream";
    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        while tokio_sender.try_send(ItemType::default()).is_err() {}
        while futures::executor::block_on(tokio_stream.next()).is_none() {}
    }));

    group.finish();
}


/// Benchmarks the inter-thread latency as measured by the receiver thread, which signals the sender thread to produce an item, then
/// waits for the element. At any given time, there are only 2 threads running and the measured times are:\
///   - the time it takes for a thread to signal another one (this is the same for everybody)
///   - + the time for the first thread to receive the element.
/// The "Baseline" measurement is an attempt to determine how much time is spent signaling a thread -- so
/// the real latency would be "the measured values minus the baseline value"
fn bench_inter_thread_latency(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Inter-thread LATENCY");

    fn baseline_it(group: &mut BenchmarkGroup<WallTime>) {
        let bench_id = "Baseline -- thread signaling time";
        crossbeam::scope(|scope| {
            let keep_running = Arc::new(AtomicBool::new(true));
            let keep_running_ref = keep_running.clone();
            let counter = Arc::new(AtomicU8::new(0));
            let counter_ref = counter.clone();
            scope.spawn(move |_| {
                while keep_running.load(Relaxed) {
                    counter.fetch_add(1, Relaxed);
                }
            });
            group.bench_function(bench_id, |bencher| bencher.iter(|| {
                let last_count = counter_ref.load(Relaxed);
                loop {
                    let current_count = counter_ref.load(Relaxed);
                    if current_count != last_count {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }));
            keep_running_ref.store(false, Relaxed);
        }).expect("Spawn baseline threads");
    }

    fn bench_it(group:          &mut BenchmarkGroup<WallTime>,
                bench_id:       &str,
                mut send_fn:    impl FnMut() + Send,
                mut receive_fn: impl FnMut()) {
        crossbeam::scope(move |scope| {
            let keep_running = Arc::new(AtomicBool::new(true));
            let keep_running_ref = keep_running.clone();
            let send = Arc::new(AtomicBool::new(false));
            let send_ref = send.clone();
            scope.spawn(move |_| {
                while keep_running.load(Relaxed) {
                    while !send.swap(false, Relaxed) {}
                    send_fn();
                }
            });
            group.bench_function(bench_id, |bencher| bencher.iter(|| {
                send_ref.store(true, Relaxed);
                receive_fn();
            }));
            keep_running_ref.store(false, Relaxed);
            send_ref.store(true, Relaxed);
        }).expect("Spawn benchmarking threads");
    }

    baseline_it(&mut group);

    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;
    bench_it(&mut group,
             "reactive-mutiny's Atomic Stream",
             || while !atomic_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {std::hint::spin_loop()},
             || while futures::executor::block_on(atomic_stream.next()).is_none() {std::hint::spin_loop()});

    let full_sync_channel = reactive_mutiny::uni::channels::movable::full_sync::FullSync::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut full_sync_stream, _) = full_sync_channel.create_stream();
    let full_sync_sender = full_sync_channel;
    bench_it(&mut group,
             "reactive-mutiny's FullSync Stream",
             || while !full_sync_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {std::hint::spin_loop()},
             || while futures::executor::block_on(full_sync_stream.next()).is_none() {std::hint::spin_loop()});

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);
    bench_it(&mut group,
             "Tokio MPSC Stream",
             || while tokio_sender.try_send(ItemType::default()).is_err() {std::hint::spin_loop()},
             || while futures::executor::block_on(tokio_stream.next()).is_none() {std::hint::spin_loop()});

    group.finish();
}

/// Benchmarks the same-thread throughput, which is measured by the time it takes to fill the backing buffer with elements + the time to receive all of them
fn bench_same_thread_throughput(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Same-thread THROUGHPUT");

    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;
    let bench_id = "reactive-mutiny's Atomic Stream";
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        for _ in 0..BUFFER_SIZE {
            while !atomic_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {};
        }
        for _ in 0..BUFFER_SIZE {
            while futures::executor::block_on(atomic_stream.next()).is_none() {};
        }
    }));

    let full_sync_channel = reactive_mutiny::uni::channels::movable::full_sync::FullSync::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut full_sync_stream, _) = full_sync_channel.create_stream();
    let full_sync_sender = full_sync_channel;
    let bench_id = "reactive-mutiny's FullSync Stream";
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        for _ in 0..BUFFER_SIZE {
            while !full_sync_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {};
        }
        for _ in 0..BUFFER_SIZE {
            while futures::executor::block_on(full_sync_stream.next()).is_none() {};
        }
    }));

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);
    let bench_id = "Tokio MPSC Stream";
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        for _ in 0..BUFFER_SIZE {
            while tokio_sender.try_send(ItemType::default()).is_err() {};
        }
        for _ in 0..BUFFER_SIZE {
            while futures::executor::block_on(tokio_stream.next()).is_none() {};
        }
    }));

    group.finish();
}

/// Benchmarks the inter-thread throughput as measured by the receiver thread, which consumes the events that are produced -- non-stop --
/// by the producer thread, simulating a high contention scenario.
fn bench_inter_thread_throughput(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Inter-thread THROUGHPUT");

    fn bench_it(group:          &mut BenchmarkGroup<WallTime>,
                bench_id:       &str,
                mut send_fn:    impl FnMut() + Send,
                mut receive_fn: impl FnMut()) {
        crossbeam::scope(move |scope| {
            let keep_running = Arc::new(AtomicBool::new(true));
            let keep_running_ref = keep_running.clone();
            scope.spawn(move |_| {
                while keep_running.load(Relaxed) {
                    send_fn();
                }
            });
            group.bench_function(bench_id, |bencher| bencher.iter(|| {
                receive_fn();
            }));
            keep_running_ref.store(false, Relaxed);
        }).expect("Spawn benchmarking threads");
    }

    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;
    bench_it(&mut group,
             "reactive-mutiny's Atomic Stream",
             || for _ in 0..BUFFER_SIZE {
                            if !atomic_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {std::hint::spin_loop();std::hint::spin_loop();std::hint::spin_loop()}
                        },
             || for _ in 0..(BUFFER_SIZE>>5) { while futures::executor::block_on(atomic_stream.next()).is_none() {std::hint::spin_loop()} });

    let full_sync_channel = reactive_mutiny::uni::channels::movable::full_sync::FullSync::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut full_sync_stream, _) = full_sync_channel.create_stream();
    let full_sync_sender = full_sync_channel;
    bench_it(&mut group,
             "reactive-mutiny's Full Sync Stream",
             || for _ in 0..BUFFER_SIZE {
                            if !full_sync_sender.send_with(|slot| *slot = ItemType::default()).is_ok() {std::hint::spin_loop();std::hint::spin_loop();std::hint::spin_loop()}
                        },
             || for _ in 0..(BUFFER_SIZE>>5) { while futures::executor::block_on(full_sync_stream.next()).is_none() {std::hint::spin_loop()} });

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);
    bench_it(&mut group,
             "Tokio MPSC Stream",
             || for _ in 0..BUFFER_SIZE {
                            if tokio_sender.try_send(ItemType::default()).is_err() {std::hint::spin_loop();std::hint::spin_loop();std::hint::spin_loop()};
                        },
             || for _ in 0..(BUFFER_SIZE>>5) { while futures::executor::block_on(tokio_stream.next()).is_none() {std::hint::spin_loop()} });

    group.finish();
}

// high level use of our APIs
/////////////////////////////
// measures how fast each serialization strategy is

use std::future;
use std::ops::Deref;
use std::time::Duration;
use tokio::sync::Mutex;
use futures::stream::StreamExt;
use reactive_messaging::prelude::{ConstConfig, MessagingService, ProtocolEvent, ReactiveMessagingConfig, ReactiveMessagingMemoryMappable, ResponsiveStream, RetryingStrategies};
use reactive_messaging::{new_socket_client, new_socket_server, start_client_processor, start_server_processor};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

/// Measures the round-trip latency in each of the serializers, as measured by the client
/// -- with "round-trip latency" being the time it takes to send a message, have it processed remotely, and receive back the answer.
///
/// This benchmark works by setting up a client and server with the following logic:
/// The client sends the first ping upon connection -- Ping(0); the server receives it and acknowledges by sending Pong(0);
/// the client receives it and sends Ping(n+1) and so on...\
///
/// NOTE:
///  1) The sequence will run until `criterion` stop the benchmarks -- no time will be spent trying to shut down the client nor the server
///  2) if you want to estimate the "server leg of the latency", just divide each latency value by 2.
fn round_trip_latencies(criterion: &mut Criterion) {

    const LISTENING_INTERFACE: &str = "127.0.0.1";
    const PORT: u16 = 9755;
    const CONFIG: ConstConfig = ConstConfig {
        retrying_strategy: RetryingStrategies::DoNotRetry,
        ..ConstConfig::default()
    };

    // the models -- able to work with the 3 default serdes of this crate
    /////////////////////////////////////////////////////////////////////


    /// The messages issued by the client
    #[derive(Debug, PartialEq, Serialize, Deserialize, rkyv::Serialize, rkyv::Deserialize, rkyv::Archive)]
    #[archive_attr(derive(Debug, PartialEq))]
    enum ClientMessages {
        Ping(u32)
    }
    impl ReactiveMessagingConfig<ClientMessages> for ClientMessages {}
    impl ReactiveMessagingMemoryMappable for ClientMessages {}
    impl Default for ClientMessages {
        fn default() -> Self {
            Self::Ping(0)
        }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize, rkyv::Serialize, rkyv::Deserialize, rkyv::Archive)]
    #[archive_attr(derive(Debug, PartialEq))]
    enum ServerMessages {
        Pong(u32)
    }
    impl ReactiveMessagingConfig<ServerMessages> for ServerMessages {}
    impl ReactiveMessagingMemoryMappable for ServerMessages {}
    impl Default for ServerMessages {
        fn default() -> Self {
            Self::Pong(0)
        }
    }

    // setup the Tokio Runtime
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    // TODO: if we ever revisit this, is there a way to improve this code so no copy & paste are needed?

    let mut group = criterion.benchmark_group("Client/Server LATENCY");

    let bench_id = "`ron` (textual serde)";
    group.bench_function(bench_id, |bencher| {
        bencher.to_async(&tokio_runtime).iter_custom(|iters| {
            async move {

                // setup the client and server
                let start_server = || async {
                    let mut server = new_socket_server!(CONFIG, LISTENING_INTERFACE, PORT);
                    start_server_processor!(CONFIG, Textual, Atomic, server, ClientMessages, ServerMessages,
            |_| future::ready(()),
            |_, _, peer, client_stream| client_stream
// .inspect(|msg| println!("SERVER received something: {msg:?}"))
                .map(|client_message| match client_message.deref() {
                    ClientMessages::Ping(n) => ServerMessages::Pong(*n),
                })
                .to_responsive_stream(peer, |_, _| ())
        )
                        .expect("Couldn't start the server");
                    server
                };
                let start = Arc::new(Mutex::new(None));   // Instant, set when the connection has been established
                let finish = Arc::new(Mutex::new(None));   // Instant, set when the client asked to close the connection
                let start_clone = start.clone();
                let finish_clone = finish.clone();
                let start_client = |n_limit: u32| async move {
                    let mut client = new_socket_client!(CONFIG, LISTENING_INTERFACE, PORT);
                    start_client_processor!(CONFIG, Textual, Atomic, client, ServerMessages, ClientMessages,
            // the connection events callback
            move |event| {
                let start = start_clone.clone();
                async move {
                    if let ProtocolEvent::PeerArrived{ peer } = event {
// eprintln!("CLIENT: connection established!");
                        start.lock().await.replace(Instant::now());
                        peer.send_async(ClientMessages::Ping(0)).await
                            .expect("CLIENT: Couldn't send the initial message")
                    }
                }
            },
            // the noop messages event stream -- responses will be sent via the criterion bench execution bellow
            move |_, _, peer, server_stream| {
                let finish = finish_clone.clone();
                let peer_clone = peer.clone();
                server_stream
// .inspect(|msg| eprintln!("CLIENT received something: {msg:?}"))
                .map(move |server_message| match server_message.deref() {
                    ServerMessages::Pong(n) => {
                        if *n >= n_limit {
                            finish.try_lock().expect("Couldn't lock `finish` for setting").replace(Instant::now());
                            peer.cancel_and_close();
                        }
                        ClientMessages::Ping(*n+1)
                    }
                })
                .to_responsive_stream(peer_clone, |_, _| ())
            }
         )
                        .expect("Couldn't start the client");
                    client
                };

                // create the client and the server, setting `iters` as the limit
                // (a `start` instant will be available -- set when the connection is stablished)
                let mut server = start_server().await;
                // wait a little bit for the server really to start
                tokio::time::sleep(Duration::from_millis(10)).await;
                let mut client = start_client(iters as u32).await;

                // wait for the work to be done
                client.termination_waiter()().await
                    .expect("Error waiting for the client to finish");

                let server_termination_waiter = server.termination_waiter();
                server.terminate().await
                    .expect("Error waiting for the server to terminate");
                server_termination_waiter().await
                    .expect("Error waiting for the server to terminate");

                // return the measure time
                let start = start.try_lock().expect("Couldn't lock `start` for getting").expect("Starting time wasn't set!");
                let finish = finish.try_lock().expect("Couldn't lock `finish` for getting").expect("Finishing time wasn't set!");

                finish.duration_since(start)

            }
        })
    });

    let bench_id = "`mmap` (fixed size binary -- no serde)";
    group.bench_function(bench_id, |bencher| {
        bencher.to_async(&tokio_runtime).iter_custom(|iters| {
            async move {

                // setup the client and server
                let start_server = || async {
                    let mut server = new_socket_server!(CONFIG, LISTENING_INTERFACE, PORT);
                    start_server_processor!(CONFIG, MmapBinary, Atomic, server, ClientMessages, ServerMessages,
            |_| future::ready(()),
            |_, _, peer, client_stream| client_stream
// .inspect(|msg| println!("SERVER received something: {msg:?}"))
                .map(|client_message| match client_message.deref() {
                    ClientMessages::Ping(n) => ServerMessages::Pong(*n),
                })
                .to_responsive_stream(peer, |_, _| ())
        )
                        .expect("Couldn't start the server");
                    server
                };
                let start = Arc::new(Mutex::new(None));   // Instant, set when the connection has been established
                let finish = Arc::new(Mutex::new(None));   // Instant, set when the client asked to close the connection
                let start_clone = start.clone();
                let finish_clone = finish.clone();
                let start_client = |n_limit: u32| async move {
                    let mut client = new_socket_client!(CONFIG, LISTENING_INTERFACE, PORT);
                    start_client_processor!(CONFIG, MmapBinary, Atomic, client, ServerMessages, ClientMessages,
            // the connection events callback
            move |event| {
                let start = start_clone.clone();
                async move {
                    if let ProtocolEvent::PeerArrived{ peer } = event {
// eprintln!("CLIENT: connection established!");
                        start.lock().await.replace(Instant::now());
                        peer.send_async(ClientMessages::Ping(0)).await
                            .expect("CLIENT: Couldn't send the initial message")
                    }
                }
            },
            // the noop messages event stream -- responses will be sent via the criterion bench execution bellow
            move |_, _, peer, server_stream| {
                let finish = finish_clone.clone();
                let peer_clone = peer.clone();
                server_stream
// .inspect(|msg| eprintln!("CLIENT received something: {msg:?}"))
                .map(move |server_message| match server_message.deref() {
                    ServerMessages::Pong(n) => {
                        if *n >= n_limit {
                            finish.try_lock().expect("Couldn't lock `finish` for setting").replace(Instant::now());
                            peer.cancel_and_close();
                        }
                        ClientMessages::Ping(*n+1)
                    }
                })
                .to_responsive_stream(peer_clone, |_, _| ())
            }
         )
                        .expect("Couldn't start the client");
                    client
                };

                // create the client and the server, setting `iters` as the limit
                // (a `start` instant will be available -- set when the connection is stablished)
                let mut server = start_server().await;
                // wait a little bit for the server really to start
                tokio::time::sleep(Duration::from_millis(10)).await;
                let mut client = start_client(iters as u32).await;

                // wait for the work to be done
                client.termination_waiter()().await
                    .expect("Error waiting for the client to finish");

                let server_termination_waiter = server.termination_waiter();
                server.terminate().await
                    .expect("Error waiting for the server to terminate");
                server_termination_waiter().await
                    .expect("Error waiting for the server to terminate");

                // return the measure time
                let start = start.try_lock().expect("Couldn't lock `start` for getting").expect("Starting time wasn't set!");
                let finish = finish.try_lock().expect("Couldn't lock `finish` for getting").expect("Finishing time wasn't set!");

                finish.duration_since(start)

            }
        })
    });

    let bench_id = "`rkyv` (variable size binary serde)";
    group.bench_function(bench_id, |bencher| {
        bencher.to_async(&tokio_runtime).iter_custom(|iters| {
            async move {

                // setup the client and server
                let start_server = || async {
                    let mut server = new_socket_server!(CONFIG, LISTENING_INTERFACE, PORT);
                    start_server_processor!(CONFIG, VariableBinary, Atomic, server, ClientMessages, ServerMessages,
            |_| future::ready(()),
            |_, _, peer, client_stream| client_stream
// .inspect(|msg| println!("SERVER received something: {msg:?}"))
                .map(|client_message| match client_message.deref().deref() {
                    ArchivedClientMessages::Ping(n) => ServerMessages::Pong(*n),
                })
                .to_responsive_stream(peer, |_, _| ())
        )
                        .expect("Couldn't start the server");
                    server
                };
                let start = Arc::new(Mutex::new(None));   // Instant, set when the connection has been established
                let finish = Arc::new(Mutex::new(None));   // Instant, set when the client asked to close the connection
                let start_clone = start.clone();
                let finish_clone = finish.clone();
                let start_client = |n_limit: u32| async move {
                    let mut client = new_socket_client!(CONFIG, LISTENING_INTERFACE, PORT);
                    start_client_processor!(CONFIG, VariableBinary, Atomic, client, ServerMessages, ClientMessages,
            // the connection events callback
            move |event| {
                let start = start_clone.clone();
                async move {
                    if let ProtocolEvent::PeerArrived{ peer } = event {
// eprintln!("CLIENT: connection established!");
                        start.lock().await.replace(Instant::now());
                        peer.send_async(ClientMessages::Ping(0)).await
                            .expect("CLIENT: Couldn't send the initial message")
                    }
                }
            },
            // the noop messages event stream -- responses will be sent via the criterion bench execution bellow
            move |_, _, peer, server_stream| {
                let finish = finish_clone.clone();
                let peer_clone = peer.clone();
                server_stream
// .inspect(|msg| eprintln!("CLIENT received something: {msg:?}"))
                .map(move |server_message| match server_message.deref().deref() {
                    ArchivedServerMessages::Pong(n) => {
                        if *n >= n_limit {
                            finish.try_lock().expect("Couldn't lock `finish` for setting").replace(Instant::now());
                            peer.cancel_and_close();
                        }
                        ClientMessages::Ping(*n+1)
                    }
                })
                .to_responsive_stream(peer_clone, |_, _| ())
            }
         )
                        .expect("Couldn't start the client");
                    client
                };

                // create the client and the server, setting `iters` as the limit
                // (a `start` instant will be available -- set when the connection is stablished)
                let mut server = start_server().await;
                // wait a little bit for the server really to start
                tokio::time::sleep(Duration::from_millis(10)).await;
                let mut client = start_client(iters as u32).await;

                // wait for the work to be done
                client.termination_waiter()().await
                    .expect("Error waiting for the client to finish");

                let server_termination_waiter = server.termination_waiter();
                server.terminate().await
                    .expect("Error waiting for the server to terminate");
                server_termination_waiter().await
                    .expect("Error waiting for the server to terminate");

                // return the measure time
                let start = start.try_lock().expect("Couldn't lock `start` for getting").expect("Starting time wasn't set!");
                let finish = finish.try_lock().expect("Couldn't lock `finish` for getting").expect("Finishing time wasn't set!");

                finish.duration_since(start)

            }
        })
    });

}



// Criterion configs
////////////////////
criterion_group!(benches, bench_same_thread_latency, bench_same_thread_throughput, bench_inter_thread_latency, bench_inter_thread_throughput, round_trip_latencies);
criterion_main!(benches);