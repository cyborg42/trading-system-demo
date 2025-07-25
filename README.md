# Trading System Demo

A high-performance market data ingestion and order-execution pipeline for futures trading systems built in Rust. This project demonstrates low-latency, high-throughput design with lock-free data structures and concurrent processing.

## Features

- **Real-time market data ingestion** via ZMQ with microsecond-level processing
- **Lock-free ring buffer** for high-speed data transfer between threads
- **In-memory order book** with efficient bid/ask level management
- **High-performance data storage** using RocksDB with batch writes
- **Execution engine** that processes market updates and maintains order book state

## Quick Start

### 1. Market Data Mock Generator

Simulates market data feeds for BTCUSDT, ETHUSDT, and SOLUSDT:

```bash
cargo run --bin market_update_mock -- --interval-ms 100 --zmq-address tcp://127.0.0.1:5555
```

Options:

- `--interval-ms`: Update interval in milliseconds (default: 100)
- `--zmq-address`: ZMQ endpoint (default: tcp://127.0.0.1:5555)

### 2. Execution Engine

Processes market updates and maintains order book:

```bash
cargo run --bin execution_engine -- --buffer-size 1000 --db-path ./rocksdb
```

Options:

- `--buffer-size`: Ring buffer size (default: 1000)
- `--zmq-address`: ZMQ endpoint (default: tcp://127.0.0.1:5555)
- `--db-path`: RocksDB storage path (optional)
- `--log-dir`: Log directory (optional)
- `--snapshot-log`: Whether to log order book snapshots (default: true)

### 3. Database Reader

Reads and displays stored market data:

```bash
cargo run --bin db_reader -- --db-path ./rocksdb
```

Options:

- `--db-path`: RocksDB path to read from (default: ./rocksdb)

## Ring Buffer Benchmark

The custom lock-free ring buffer outperforms Crossbeam channels:

```bash
cargo bench --bench bench_ring_buffer
```

**Performance Results:**

- **Custom Ring Buffer**: 1.08ms (100k messages)
- **Crossbeam Unbounded**: 2.07ms (1.9x slower)
- **Crossbeam Bounded**: 2.05ms (1.9x slower)

See [BENCHMARK_REPORT.md](BENCHMARK_REPORT.md) for detailed performance analysis.
