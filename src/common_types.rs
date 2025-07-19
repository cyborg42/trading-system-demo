use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tinystr::TinyAsciiStr;
use zerocopy::{FromBytes, Immutable, IntoBytes, TryFromBytes};

pub type SymbolStr = TinyAsciiStr<32>;
pub type Price = Decimal;
pub type Size = Decimal;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    IntoBytes,
    TryFromBytes,
    Immutable,
)]
#[repr(u8)]
pub enum Direction {
    Bid,
    Ask,
}

#[derive(IntoBytes, FromBytes, Immutable)]
#[repr(C)]
pub struct DecimalZerocopy {
    flags: u32,
    hi: u32,
    lo: u32,
    mid: u32,
}

#[derive(IntoBytes, FromBytes, Immutable)]
#[repr(transparent)]
pub struct SymbolStrZerocopy {
    bytes: [u8; 32],
}
