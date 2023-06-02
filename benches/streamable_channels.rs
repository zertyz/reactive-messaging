//! Compares the performance of channels that produce `Streams` (`tokio` & `futures`, but not `std` nor `crossbeam`)
//! with `reactive-mutiny`'s `Uni`s
//!
//! # Analysis 2023-06-01
//!
//!   `reactive-mutiny`'s Atomic is the winner on all tests, for a variety of Intel, AMD and ARM cpus -- sometimes winning by 2x.
//!
//! Out of the results here, it was decided that the `reactive-mutiny`'s Atomic Channel will be used instead of Tokio's
//!

use std::future::Future;
use criterion::{
    criterion_group,
    criterion_main,
    Criterion,
    BenchmarkGroup,
    measurement::WallTime,
};
use std::{
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            AtomicU8,
            Ordering::Relaxed,
        }
    },
};
use once_cell::sync::Lazy;
use reactive_mutiny::prelude::{ChannelCommon, ChannelUni, ChannelProducer};
use tokio_stream::{Stream, StreamExt};


/// Represents a reasonably sized message, similar to production needs
#[derive(Debug)]
struct MessageType {
    _data:  [u8; 128],
}
impl Default for MessageType {
    fn default() -> Self {
        MessageType { _data: [0; 128] }
    }
}

type ItemType = MessageType;
const BUFFER_SIZE: usize = 1<<14;


/// Benchmarks the same-thread latency, which is measured by the time it takes to send a single element + time to receive that one element
fn bench_same_thread_latency(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Same-thread LATENCY");

    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);

    let bench_id = format!("reactive-mutiny's Atomic Stream");
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        while !atomic_sender.try_send(|slot| *slot = ItemType::default()) {}
        while futures::executor::block_on(atomic_stream.next()).is_none() {}
    }));

    let bench_id = format!("Tokio MPSC Stream");
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        while tokio_sender.try_send(ItemType::default()).is_err() {}
        while futures::executor::block_on(tokio_stream.next()).is_none() {}
    }));

    group.finish();
}


/// Benchmarks the inter-thread latency, which is measured by the receiver thread, which signals the sender thread to produce an item, then
/// waits for the element. At any given time, there are only 2 threads running and the measured times are:\
///   - the time it takes for a thread to signal another one (this is the same for everybody)
///   - + the time for the first thread to receive the element.
/// The "Baseline" measurement is an attempt to determine how much time is spent signaling a thread -- so
/// the real latency would be the measured values - Baseline
fn bench_inter_thread_latency(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Inter-thread LATENCY");

    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);

    fn baseline_it(group: &mut BenchmarkGroup<WallTime>) {
        let bench_id = format!("Baseline -- thread signaling time");
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
                bench_id:       String,
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

    bench_it(&mut group,
             format!("reactive-mutiny's Atomic Stream"),
             || while !atomic_sender.try_send(|slot| *slot = ItemType::default()) {std::hint::spin_loop()},
             || while futures::executor::block_on(atomic_stream.next()).is_none() {std::hint::spin_loop()});

    bench_it(&mut group,
             format!("Tokio MPSC Stream"),
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

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);

    let bench_id = format!("reactive-mutiny's Atomic Stream");
    group.bench_function(bench_id, |bencher| bencher.iter(|| {
        for _ in 0..BUFFER_SIZE {
            while !atomic_sender.try_send(|slot| *slot = ItemType::default()) {};
        }
        for _ in 0..BUFFER_SIZE {
            while futures::executor::block_on(atomic_stream.next()).is_none() {};
        }
    }));

    let bench_id = format!("Tokio MPSC Stream");
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

/// Benchmarks the inter-thread throughput, which is measured by the receiver thread, which consumes the events that are produced -- non-stop --
/// by the producer thread, simulating a high contention scenario.
fn bench_inter_thread_throughput(criterion: &mut Criterion) {

    let mut group = criterion.benchmark_group("Inter-thread THROUGHPUT");

    let atomic_channel = reactive_mutiny::uni::channels::movable::atomic::Atomic::<ItemType, BUFFER_SIZE, 1>::new("ItemType Uni Channel for benchmarks");
    let (mut atomic_stream, _) = atomic_channel.create_stream();
    let atomic_sender = atomic_channel;

    let (tokio_sender, tokio_receiver) = tokio::sync::mpsc::channel::<ItemType>(BUFFER_SIZE);
    let mut tokio_stream = tokio_stream::wrappers::ReceiverStream::new(tokio_receiver);

    fn bench_it(group:          &mut BenchmarkGroup<WallTime>,
                bench_id:       String,
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

    bench_it(&mut group,
             format!("reactive-mutiny's Atomic Stream"),
             || for _ in 0..BUFFER_SIZE {
                            if !atomic_sender.try_send(|slot| *slot = ItemType::default()) {std::hint::spin_loop();std::hint::spin_loop();std::hint::spin_loop()}
                        },
             || for _ in 0..(BUFFER_SIZE>>5) { while futures::executor::block_on(atomic_stream.next()).is_none() {std::hint::spin_loop()} });

    bench_it(&mut group,
             format!("Tokio MPSC Stream"),
             || for _ in 0..BUFFER_SIZE {
                            if tokio_sender.try_send(ItemType::default()).is_err() {std::hint::spin_loop();std::hint::spin_loop();std::hint::spin_loop()};
                        },
             || for _ in 0..(BUFFER_SIZE>>5) { while futures::executor::block_on(tokio_stream.next()).is_none() {std::hint::spin_loop()} });

    group.finish();
}

criterion_group!(benches, bench_same_thread_latency, bench_same_thread_throughput, bench_inter_thread_latency, bench_inter_thread_throughput);
criterion_main!(benches);