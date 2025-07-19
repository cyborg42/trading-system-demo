use thiserror::Error;

use crate::common_types::Price;

#[derive(Error, Debug)]
pub enum Error {
    #[error("insufficient size at price level {0}")]
    InsufficientSize(Price),
    #[error("price level {0} not found")]
    PriceLevelNotFound(Price),
}
