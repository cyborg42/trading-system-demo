use std::{sync::LazyLock, thread, time::Duration};

use clap::Parser;
use fastrand::Rng;
use rust_decimal::{Decimal, dec};
use trading_system_demo::{
    common_types::{Direction, SymbolStr},
    market::{MarketUpdate, MarketUpdateRequest},
};

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, short, default_value = "tcp://127.0.0.1:5555")]
    zmq_address: String,
    #[clap(long, short, default_value = "1000")]
    interval_ms: u64,
}

#[derive(Debug, Clone)]
struct MarketUpdateMockConfig {
    symbol: SymbolStr,
    price: Decimal,
    price_step: Decimal,
    size_step: Decimal,
}

static CONFIGS: LazyLock<Vec<MarketUpdateMockConfig>> = LazyLock::new(|| {
    [
        MarketUpdateMockConfig {
            symbol: "BTCUSDT".parse().unwrap(),
            price: dec!(12000),
            price_step: dec!(1),
            size_step: dec!(0.00001),
        },
        MarketUpdateMockConfig {
            symbol: "ETHUSDT".parse().unwrap(),
            price: dec!(3000),
            price_step: dec!(0.1),
            size_step: dec!(0.001),
        },
        MarketUpdateMockConfig {
            symbol: "SOLUSDT".parse().unwrap(),
            price: dec!(170),
            price_step: dec!(0.01),
            size_step: dec!(0.01),
        },
    ]
    .to_vec()
});

// Generate normally distributed random numbers using Box-Muller transform
fn normal_random(mean: f64, std_dev: f64, rng: &mut Rng) -> f64 {
    let u1 = rng.f64();
    let u2 = rng.f64();
    let z0 = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    mean + std_dev * z0
}

// Generate price offset within specified range using normal distribution (integer multiples of step)
fn generate_price_offset(price_step: &Decimal, rng: &mut Rng) -> Decimal {
    // Generate price offset within ±2000 levels using normal distribution
    let max_steps = 2000;
    let steps_f64 = normal_random(0.0, max_steps as f64 / 3.0, rng);
    let steps = steps_f64.clamp(-max_steps as f64, max_steps as f64) as i32;
    *price_step * Decimal::from(steps)
}

// Generate random size
fn generate_size(size_step: &Decimal, rng: &mut Rng) -> Decimal {
    // Generate size between 1-100 times size_step
    let multiplier = rng.u32(1..=100);
    *size_step * Decimal::from(multiplier)
}

fn main() {
    let args = Args::parse();
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::PUSH).unwrap();
    socket.connect(&args.zmq_address).unwrap();
    println!("Connected to trading service on {}", args.zmq_address);

    let mut rng = Rng::new();

    loop {
        for config in CONFIGS.iter() {
            // Generate price offset
            let price_offset = generate_price_offset(&config.price_step, &mut rng);
            let price = config.price + price_offset;

            // Ensure price is positive
            let price = if price <= dec!(0) { dec!(0.01) } else { price };

            // Generate size
            let size = generate_size(&config.size_step, &mut rng);

            // 10% probability to generate cancellation (negative size)
            let final_size = if rng.f64() < 0.1 { -size } else { size };

            // Randomly choose bid or ask direction
            let direction = if rng.bool() {
                Direction::Bid
            } else {
                Direction::Ask
            };

            let request = MarketUpdateRequest::new(
                config.symbol,
                (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as u64,
                MarketUpdate::new(price, final_size, direction),
            );

            println!("Sending market update: {request:?}");
            let msg = request.as_bytes();
            socket.send(msg, 0).unwrap();
        }

        thread::sleep(Duration::from_millis(args.interval_ms));
    }
}
