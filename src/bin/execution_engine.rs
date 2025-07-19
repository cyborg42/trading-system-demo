use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::Parser;
use tracing::{debug, error, info};
use trading_system_demo::{
    market::{Market, MarketUpdateRequest},
    ring_buffer::RingBuffer,
    utils::init_log,
};

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, short, default_value = "tcp://127.0.0.1:5555")]
    zmq_address: String,
    #[clap(long, short, default_value = "1000")]
    channel_size: usize,
    /// If not provided, the log will be printed to the console
    #[clap(long, short)]
    log_dir: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let _guard = init_log(args.log_dir);
    info!("Starting trading service");
    info!("Trading service listening on {}", args.zmq_address);

    let mut rb = RingBuffer::<MarketUpdateRequest>::new(args.channel_size);
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

            let mut market = Market::new();
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
        s.spawn(|| {
            // read market update from ring buffer and record to database

            let mut lost_count = 0;
            for (_request, lost) in subscriber_clone.spinning_iter() {
                if lost > 0 {
                    lost_count += lost;
                    if lost_count % 100 == 0 {
                        error!("Database record lost {} messages", lost_count);
                    }
                }
                // TODO: record to database
            }
        });
    });
}
