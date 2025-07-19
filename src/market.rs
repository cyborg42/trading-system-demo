use std::{alloc::Layout, collections::HashMap};

use crate::{
    common_types::{DecimalZerocopy, Direction, Price, Size, SymbolStr, SymbolStrZerocopy},
    order_book::OrderBook,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[repr(C)]
pub struct MarketUpdate {
    pub price: Price,
    pub size: Size,
    pub direction: Direction,
    _padding: [u8; 3],
}

impl MarketUpdate {
    pub fn new(price: Price, size: Size, direction: Direction) -> Self {
        Self {
            price,
            size,
            direction,
            _padding: [0; 3],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[repr(C)]
pub struct MarketUpdateRequest {
    pub symbol: SymbolStr,
    pub timestamp_ms: u64,
    pub update: MarketUpdate,
    _padding: [u8; 4],
}

impl MarketUpdateRequest {
    pub const fn size() -> usize {
        Layout::new::<MarketUpdateRequest>().size()
    }
    pub fn new(symbol: SymbolStr, timestamp_ms: u64, update: MarketUpdate) -> Self {
        Self {
            symbol,
            timestamp_ms,
            update,
            _padding: [0; 4],
        }
    }
}
#[derive(IntoBytes, TryFromBytes, Immutable)]
#[repr(C)]
struct MarketUpdateZerocopy {
    price: DecimalZerocopy,
    size: DecimalZerocopy,
    direction: Direction,
    _padding: [u8; 3],
}

#[derive(IntoBytes, TryFromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct MarketUpdateRequestZerocopy {
    symbol: SymbolStrZerocopy,
    timestamp_ms: u64,
    update: MarketUpdateZerocopy,
    _padding: [u8; 4],
}

impl From<&MarketUpdateRequest> for &MarketUpdateRequestZerocopy {
    fn from(value: &MarketUpdateRequest) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

impl From<&MarketUpdateRequestZerocopy> for &MarketUpdateRequest {
    fn from(value: &MarketUpdateRequestZerocopy) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}
impl MarketUpdateRequest {
    pub fn as_bytes(&self) -> &[u8] {
        let request_zerocopy: &MarketUpdateRequestZerocopy = self.into();
        request_zerocopy.as_bytes()
    }
}

impl TryFrom<&[u8]> for &MarketUpdateRequest {
    type Error = ();
    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let request_zerocopy =
            MarketUpdateRequestZerocopy::try_ref_from_bytes(bytes).map_err(|_| ())?;
        Ok(request_zerocopy.into())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Market {
    pub order_books: HashMap<SymbolStr, OrderBook>,
}

impl Market {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update_order_book(&mut self, request: MarketUpdateRequest) {
        debug!("Updating order book: {:?}", request);
        let symbol = request.symbol;
        let order_book = self.order_books.entry(symbol).or_default();
        match order_book.update(request.update) {
            Ok(update) => {
                debug!("Order book updated: {:?}", update);
            }
            Err(e) => {
                error!("Error updating order book: {:?}", e);
            }
        }
    }
}
