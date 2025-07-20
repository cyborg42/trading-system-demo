use clap::Parser;
use rocksdb::{ColumnFamilyDescriptor, DB, IteratorMode, Options};
use std::path::PathBuf;
use trading_system_demo::market::MarketUpdateRequest;

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, short, default_value = "./rocksdb")]
    db_path: PathBuf,
}

fn main() {
    let args = Args::parse();
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    let cf_opts = Options::default();
    let cf_descriptor = ColumnFamilyDescriptor::new("market_update", cf_opts);
    let db = DB::open_cf_descriptors(&opts, args.db_path, vec![cf_descriptor]).unwrap();
    let cf = db.cf_handle("market_update").unwrap();
    let iter = db.iterator_cf(cf, IteratorMode::Start);
    for result in iter {
        if let Ok((key, value)) = result {
            let _key = u64::from_be_bytes(key.as_ref().try_into().unwrap_or_default());
            let request: &MarketUpdateRequest = match (&value[..]).try_into() {
                Ok(request) => request,
                Err(e) => {
                    eprintln!("Error parsing market update: {e:?}");
                    continue;
                }
            };
            println!("{request:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
