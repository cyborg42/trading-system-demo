use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::Parser;
use rocksdb::{ColumnFamilyDescriptor, DB, Options, WriteBatch};
use tracing::{debug, error, info};
use trading_system_demo::{
    market::{Market, MarketUpdateRequest},
    ring_buffer::RingBuffer,
    utils::{IDGenerator, init_log},
};

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, short, default_value = "tcp://127.0.0.1:5555")]
    zmq_address: String,
    #[clap(long, short, default_value = "1000")]
    buffer_size: usize,
    /// If not provided, the log will be printed to the console
    #[clap(long, short)]
    log_dir: Option<PathBuf>,
    #[clap(long, short)]
    snapshot_log_interval_sec: Option<u64>,
    #[clap(long, short)]
    db_path: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let _guard = init_log(args.log_dir);
    info!("Starting trading service");
    info!("Trading service listening on {}", args.zmq_address);
    let snapshot_log_interval = args.snapshot_log_interval_sec.map(Duration::from_secs);
    let mut rb = RingBuffer::<MarketUpdateRequest>::new(args.buffer_size);
    let (mut publisher, mut subscriber) = rb.split();
    let mut subscriber_clone = subscriber.clone();
    std::thread::scope(|s| {
        s.spawn(|| {
            // receive market update from zmq and write to ring buffer

            let ctx = zmq::Context::new();
            let socket = ctx.socket(zmq::PULL).unwrap();
            socket.bind(&args.zmq_address).unwrap();
            // constrain the error message to be printed every 5 seconds
            let mut last_error = Instant::now() - Duration::from_secs(100);
            // pre-allocate the buffer, no unnecessary heap allocation and memcpy
            const DATA_SIZE: usize = MarketUpdateRequest::size();
            let mut bytes = [0; DATA_SIZE];
            while let Ok(_n) = socket.recv_into(&mut bytes, 0) {
                let request: &MarketUpdateRequest = match (&bytes[..]).try_into() {
                    Ok(request) => request,
                    Err(e) => {
                        if last_error.elapsed().as_secs() > 5 {
                            error!("Error parsing market update: {:?}", e);
                            last_error = Instant::now();
                        }
                        continue;
                    }
                };
                publisher.write(request.clone());
            }
        });
        s.spawn(|| {
            // read market update from ring buffer and update order book

            let mut market = Market::new(snapshot_log_interval);
            let mut lost_count = 0;
            for (request, lost) in subscriber.spinning_iter() {
                debug!("Received market update from ring buffer: {:?}", request);
                if lost > 0 {
                    lost_count += lost;
                    if lost_count % 100 == 0 {
                        error!("Order book update lost {} messages", lost_count);
                    }
                }
                market.update_order_book(request);
            }
        });

        if let Some(db_path) = args.db_path {
            s.spawn(|| {
                // read market update from ring buffer and record to database
                let mut opts = Options::default();
                opts.create_if_missing(true);
                opts.create_missing_column_families(true);
                let cf_opts = Options::default();
                let cf_descriptor = ColumnFamilyDescriptor::new("market_update", cf_opts);
                let db = DB::open_cf_descriptors(&opts, db_path, vec![cf_descriptor]).unwrap();
                let cf = db.cf_handle("market_update").unwrap();
                const BATCH_SIZE: usize = 1000;
                let mut batch = WriteBatch::new();
                let mut lost_count = 0;
                let mut id_generator = IDGenerator::new();
                for (request, lost) in subscriber_clone.spinning_iter() {
                    if lost > 0 {
                        lost_count += lost;
                        if lost_count % 100 == 0 {
                            error!("Database record lost {} messages", lost_count);
                        }
                    }
                    let id = id_generator.generate(request.timestamp_ms);
                    batch.put_cf(&cf, id.to_be_bytes(), request.as_bytes());
                    if batch.len() >= BATCH_SIZE {
                        db.write(batch).unwrap();
                        batch = WriteBatch::new();
                    }
                }
            });
        }
    });
}
