use std::{
    hint::black_box,
    thread,
    time::{Duration, Instant},
};

use criterion::{Criterion, criterion_group, criterion_main};
use crossbeam::channel::{bounded, unbounded};
use trading_system_demo::ring_buffer::RingBuffer;

// Test data structure that simulates market updates
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct MarketUpdate {
    pub symbol: String,
    pub price: f64,
    pub size: u32,
    pub timestamp: u64,
}

// Benchmark configurations
const BUFFER_SIZE: usize = 10000;
const MESSAGES_PER_TEST: usize = 100000;

// Helper function to create test data
fn create_test_data(count: usize) -> Vec<MarketUpdate> {
    (0..count)
        .map(|i| MarketUpdate {
            symbol: "AAPL".to_string(),
            price: 150.0 + (i as f64 * 0.01),
            size: i as u32,
            timestamp: i as u64,
        })
        .collect()
}

// Benchmark: Crossbeam Bounded Channel SPSC
fn benchmark_crossbeam_bounded(c: &mut Criterion) {
    c.bench_function("Crossbeam Bounded", |b| {
        b.iter(|| {
            let (sender, receiver) = bounded(BUFFER_SIZE);
            let test_data = create_test_data(MESSAGES_PER_TEST);

            // Producer thread
            let producer_handle = thread::spawn(move || {
                for update in test_data {
                    sender.send(update).unwrap();
                }
            });

            // Consumer thread
            let consumer_handle = thread::spawn(move || {
                let mut received_count = 0;
                while received_count < MESSAGES_PER_TEST {
                    if let Ok(update) = receiver.recv() {
                        black_box(update);
                        received_count += 1;
                    }
                }
                received_count
            });

            producer_handle.join().unwrap();
            let received = consumer_handle.join().unwrap();
            assert_eq!(received, MESSAGES_PER_TEST);
        });
    });
}

// Benchmark: Crossbeam Unbounded Channel SPSC
fn benchmark_crossbeam_unbounded(c: &mut Criterion) {
    c.bench_function("Crossbeam Unbounded", |b| {
        b.iter(|| {
            let (sender, receiver) = unbounded();
            let test_data = create_test_data(MESSAGES_PER_TEST);

            // Producer thread
            let producer_handle = thread::spawn(move || {
                for update in test_data {
                    sender.send(update).unwrap();
                }
            });

            // Consumer thread
            let consumer_handle = thread::spawn(move || {
                let mut received_count = 0;
                while received_count < MESSAGES_PER_TEST {
                    if let Ok(update) = receiver.recv() {
                        black_box(update);
                        received_count += 1;
                    }
                }
                received_count
            });

            producer_handle.join().unwrap();
            let received = consumer_handle.join().unwrap();
            assert_eq!(received, MESSAGES_PER_TEST);
        });
    });
}

// Benchmark: Custom Ring Buffer SPSC
fn benchmark_custom_ring_buffer(c: &mut Criterion) {
    c.bench_function("Custom Ring Buffer", |b| {
        b.iter(|| {
            let mut ring_buffer = RingBuffer::new(BUFFER_SIZE);
            let (mut publisher, mut subscriber) = ring_buffer.split();
            let test_data = create_test_data(MESSAGES_PER_TEST);

            thread::scope(|s| {
                // Producer thread
                let producer_handle = s.spawn(move || {
                    for update in test_data {
                        publisher.write(update);
                    }
                });

                // Consumer thread
                let consumer_handle = s.spawn(move || {
                    let mut received_count = 0;
                    while received_count < MESSAGES_PER_TEST {
                        if let Some((update, lost)) = subscriber.read_clone() {
                            black_box(update);
                            received_count += 1;
                            received_count += lost;
                        }
                    }
                    received_count
                });

                producer_handle.join().unwrap();
                let received = consumer_handle.join().unwrap();
                assert_eq!(received, MESSAGES_PER_TEST);
            });
        });
    });
}

// Latency test - measure end-to-end latency using cross-thread measurement
fn benchmark_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("E2E Latency");
    group.sample_size(500); // Increase sample size to handle thread variation

    group.bench_function("Custom Ring Buffer", |b| {
        b.iter_custom(|iters| {
            let mut latency_statistic = Duration::ZERO;
            for _ in 0..iters {
                let mut ring_buffer: RingBuffer<(u32, Instant)> = RingBuffer::new(BUFFER_SIZE);
                let (mut publisher, mut subscriber) = ring_buffer.split();

                thread::scope(|s| {
                    // Start consumer thread to receive and calculate latency
                    let consumer_handle = s.spawn(move || {
                        let mut total_latency = Duration::ZERO;
                        let mut received_count: usize = 0;
                        let mut count: u32 = 0;
                        while received_count < MESSAGES_PER_TEST {
                            if let Some((update, lost)) = subscriber.read_clone() {
                                total_latency += update.1.elapsed();
                                received_count += 1;
                                received_count += lost;
                                count += 1;
                            }
                        }
                        total_latency / count
                    });

                    // Producer sends messages
                    let producer_handle = s.spawn(move || {
                        for _ in 0..MESSAGES_PER_TEST {
                            publisher.write((42, Instant::now()));
                        }
                    });

                    // Wait for both threads to complete
                    producer_handle.join().unwrap();
                    latency_statistic += consumer_handle.join().unwrap();
                })
            }
            latency_statistic
        });
    });

    group.bench_function("Crossbeam Bounded", |b| {
        b.iter_custom(|iters| {
            let mut latency_statistic = Duration::ZERO;
            for _ in 0..iters {
                let (tx, rx) = bounded::<(u32, Instant)>(BUFFER_SIZE);
                let mut total_latency = Duration::ZERO;

                // Start consumer thread to receive and calculate latency
                let handle = thread::spawn(move || {
                    for _ in 0..MESSAGES_PER_TEST {
                        let update = rx.recv().unwrap();
                        total_latency += Instant::now() - update.1;
                    }
                    total_latency / MESSAGES_PER_TEST as u32
                });

                // Producer sends messages
                for _ in 0..MESSAGES_PER_TEST {
                    let sent_time = Instant::now();
                    tx.send(black_box((42, sent_time))).unwrap();
                }

                // Wait for consumer to complete and get total latency
                drop(tx); // Close channel
                latency_statistic += handle.join().unwrap();
            }
            latency_statistic
        });
    });

    group.bench_function("Crossbeam Unbounded", |b| {
        b.iter_custom(|iters| {
            let mut latency_statistic = Duration::ZERO;
            for _ in 0..iters {
                let (tx, rx) = unbounded::<(u32, Instant)>();
                let mut total_latency = Duration::ZERO;

                // Start consumer thread to receive and calculate latency
                let handle = thread::spawn(move || {
                    for _ in 0..MESSAGES_PER_TEST {
                        let update = rx.recv().unwrap();
                        total_latency += Instant::now() - update.1;
                    }
                    total_latency / MESSAGES_PER_TEST as u32
                });

                // Producer sends messages
                for _ in 0..MESSAGES_PER_TEST {
                    let sent_time = Instant::now();
                    tx.send(black_box((42, sent_time))).unwrap();
                }

                // Wait for consumer to complete and get total latency
                drop(tx); // Close channel
                latency_statistic += handle.join().unwrap();
            }
            latency_statistic
        });
    });

    group.finish();
}

pub fn criterion_benchmark(c: &mut Criterion) {
    benchmark_crossbeam_bounded(c);
    benchmark_crossbeam_unbounded(c);
    benchmark_custom_ring_buffer(c);
    benchmark_latency(c);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
