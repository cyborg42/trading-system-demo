use std::cmp::Ordering;

use rust_decimal::Decimal;

use crate::{
    common_types::{Direction, Price, Size},
    error::Error,
    market::MarketUpdate,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PriceSize {
    price: Price,
    size: Size,
}

/// Represents the result of an order book update operation.
///
/// This struct contains information about what happened when an order was processed:
/// - Which orders were executed (matched against existing orders)
/// - How much of the order was placed in the order book
#[derive(Debug, Clone, Default)]
pub struct OrderBookUpdate {
    /// List of executed trades, each containing (price, size) of the executed portion.
    /// Orders are executed at the price of the resting order (the order already in the book).
    /// For example, if a bid at $100 matches an ask at $99, the trade executes at $99.
    pub executed: Vec<PriceSize>,

    /// The total size that was placed in the order book after matching.
    /// This represents the portion of the order that could not be immediately matched
    /// and was added to the appropriate side of the order book (bids or asks).
    /// If the entire order was matched, this will be None.
    pub placed: Option<PriceSize>,
}

/// Order book for a single symbol.
///
/// Based on Optiver's optimization experience, we use a sorted array where the best price is at the end.
/// Linear search from the tail is the fastest for updates.
/// Reference: https://github.com/CppCon/CppCon2024/blob/main/Presentations/When_Nanoseconds_Matter.pdf
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    /// Bids sorted from lowest to highest price (e.g., $85, $90, $92)
    bids: Vec<PriceSize>,
    /// Asks sorted from highest to lowest price (e.g., $100, $95, $94)
    asks: Vec<PriceSize>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_best_bid(&self) -> Option<&PriceSize> {
        self.bids.last()
    }
    pub fn get_best_ask(&self) -> Option<&PriceSize> {
        self.asks.last()
    }
    pub fn get_bids(&self) -> &[PriceSize] {
        &self.bids
    }
    pub fn get_asks(&self) -> &[PriceSize] {
        &self.asks
    }

    /// Insert a new bid at the correct position to maintain sorted order
    /// Bids are sorted from lowest to highest price, so we search from the end
    fn insert_bid(&mut self, price: Price, size: Size) {
        debug_assert!(size.is_sign_positive());
        // Search from the end (best prices) and insert at the correct position
        for (i, level) in self.bids.iter_mut().enumerate().rev() {
            match level.price.cmp(&price) {
                Ordering::Equal => {
                    level.size += size;
                    return;
                }
                Ordering::Less => {
                    // Found the correct position, insert here
                    self.bids.insert(i + 1, PriceSize { price, size });
                    return;
                }
                Ordering::Greater => {}
            }
        }
        // Insert at the beginning if no suitable position found
        self.bids.insert(0, PriceSize { price, size });
    }

    /// Insert a new ask at the correct position to maintain sorted order
    /// Asks are sorted from highest to lowest price, so we search from the end
    fn insert_ask(&mut self, price: Price, size: Size) {
        debug_assert!(size.is_sign_positive());
        // Search from the end (best prices) and insert at the correct position
        for (i, level) in self.asks.iter_mut().enumerate().rev() {
            match level.price.cmp(&price) {
                Ordering::Equal => {
                    level.size += size;
                    return;
                }
                Ordering::Greater => {
                    // Found the correct position, insert here
                    self.asks.insert(i + 1, PriceSize { price, size });
                    return;
                }
                Ordering::Less => {}
            }
        }
        // Insert at the beginning if no suitable position found
        self.asks.insert(0, PriceSize { price, size });
    }

    /// Match a bid against available asks
    fn match_bid_against_asks(
        &mut self,
        bid_price: Price,
        mut remaining_size: Size,
        executed: &mut Vec<PriceSize>,
    ) -> Size {
        // Bid trying to match against asks (from lowest ask to highest)
        while remaining_size > Decimal::ZERO {
            let Some(ask_level) = self.asks.last_mut() else {
                break;
            };
            if ask_level.price <= bid_price {
                if ask_level.size <= remaining_size {
                    executed.push(ask_level.clone());
                    remaining_size -= ask_level.size;
                    self.asks.pop();
                } else {
                    executed.push(PriceSize {
                        price: ask_level.price,
                        size: remaining_size,
                    });
                    ask_level.size -= remaining_size;
                    remaining_size = Decimal::ZERO;
                    break;
                }
            } else {
                break;
            }
        }
        remaining_size
    }

    /// Match an ask against available bids
    fn match_ask_against_bids(
        &mut self,
        ask_price: Price,
        mut remaining_size: Size,
        executed: &mut Vec<PriceSize>,
    ) -> Size {
        // Ask trying to match against bids (from highest bid to lowest)
        while remaining_size > Decimal::ZERO {
            let Some(bid_level) = self.bids.last_mut() else {
                break;
            };
            if bid_level.price >= ask_price {
                if bid_level.size <= remaining_size {
                    executed.push(bid_level.clone());
                    remaining_size -= bid_level.size;
                    self.bids.pop();
                } else {
                    executed.push(PriceSize {
                        price: bid_level.price,
                        size: remaining_size,
                    });
                    bid_level.size -= remaining_size;
                    remaining_size = Decimal::ZERO;
                    break;
                }
            } else {
                break;
            }
        }
        remaining_size
    }

    /// Cancel an order (negative size)
    fn cancel_order(&mut self, update: MarketUpdate) -> Result<OrderBookUpdate, Error> {
        debug_assert!(update.size.is_sign_negative());
        match update.direction {
            Direction::Bid => self.cancel_from_bids(update.price, update.size),
            Direction::Ask => self.cancel_from_asks(update.price, update.size),
        }
    }

    /// Cancel order from bids
    fn cancel_from_bids(&mut self, price: Price, size: Size) -> Result<OrderBookUpdate, Error> {
        debug_assert!(size.is_sign_negative());
        // Search from the end (best prices) since bids are sorted from lowest to highest
        for (i, level) in self.bids.iter_mut().enumerate().rev() {
            match level.price.cmp(&price) {
                Ordering::Equal => {
                    if level.size >= -size {
                        level.size += size; // size is negative, so this is subtraction
                        // Remove the level if size becomes zero
                        if level.size == Decimal::ZERO {
                            self.bids.remove(i);
                        }
                        return Ok(OrderBookUpdate {
                            executed: vec![],
                            placed: Some(PriceSize { price, size }),
                        });
                    } else {
                        return Err(Error::InsufficientSize(price));
                    }
                }
                Ordering::Less => {
                    // Price not found, break early since bids are sorted
                    break;
                }
                Ordering::Greater => {
                    // Continue searching lower prices
                }
            }
        }
        Err(Error::PriceLevelNotFound(price))
    }

    /// Cancel order from asks
    fn cancel_from_asks(&mut self, price: Price, size: Size) -> Result<OrderBookUpdate, Error> {
        debug_assert!(size.is_sign_negative());
        // Search from the end (best prices) since asks are sorted from highest to lowest
        for (i, level) in self.asks.iter_mut().enumerate().rev() {
            match level.price.cmp(&price) {
                Ordering::Equal => {
                    if level.size >= -size {
                        level.size += size; // size is negative, so this is subtraction
                        // Remove the level if size becomes zero
                        if level.size == Decimal::ZERO {
                            self.asks.remove(i);
                        }
                        return Ok(OrderBookUpdate {
                            executed: vec![],
                            placed: Some(PriceSize { price, size }),
                        });
                    } else {
                        return Err(Error::InsufficientSize(price));
                    }
                }
                Ordering::Greater => {
                    // Price not found, break early since asks are sorted
                    break;
                }
                Ordering::Less => {
                    // Continue searching higher prices
                }
            }
        }
        Err(Error::PriceLevelNotFound(price))
    }

    pub fn update(&mut self, update: MarketUpdate) -> Result<OrderBookUpdate, Error> {
        if update.size.is_zero() {
            return Ok(OrderBookUpdate {
                executed: Vec::new(),
                placed: None,
            });
        }
        if update.size.is_sign_negative() {
            return self.cancel_order(update);
        }

        // Positive size - try to match first, then place remaining
        let mut executed = Vec::new();
        let mut remaining_size = update.size;
        let mut placed = None;

        match update.direction {
            Direction::Bid => {
                remaining_size =
                    self.match_bid_against_asks(update.price, remaining_size, &mut executed);
                // Place remaining size in bids if any
                if remaining_size > Decimal::ZERO {
                    self.insert_bid(update.price, remaining_size);
                    placed = Some(PriceSize {
                        price: update.price,
                        size: remaining_size,
                    });
                }
            }
            Direction::Ask => {
                remaining_size =
                    self.match_ask_against_bids(update.price, remaining_size, &mut executed);
                // Place remaining size in asks if any
                if remaining_size > Decimal::ZERO {
                    self.insert_ask(update.price, remaining_size);
                    placed = Some(PriceSize {
                        price: update.price,
                        size: remaining_size,
                    });
                }
            }
        }

        Ok(OrderBookUpdate { executed, placed })
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::dec;

    use super::*;

    #[test]
    fn test_new_order_book() {
        let order_book = OrderBook::new();
        assert!(order_book.bids.is_empty());
        assert!(order_book.asks.is_empty());
        assert!(order_book.get_best_bid().is_none());
        assert!(order_book.get_best_ask().is_none());
    }

    #[test]
    fn test_insert_bid() {
        let mut order_book = OrderBook::new();

        // Insert first bid
        order_book.insert_bid(dec!(100), dec!(10));
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(100),
                size: dec!(10),
            }
        );

        // Insert lower price bid (should be first)
        order_book.insert_bid(dec!(90), dec!(5));
        assert_eq!(order_book.bids.len(), 2);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(90),
                size: dec!(5),
            }
        );
        assert_eq!(
            order_book.bids[1],
            PriceSize {
                price: dec!(100),
                size: dec!(10),
            }
        );

        // Insert same price (should add to existing)
        order_book.insert_bid(dec!(100), dec!(3));
        assert_eq!(order_book.bids.len(), 2);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(90),
                size: dec!(5),
            }
        );
        assert_eq!(
            order_book.bids[1],
            PriceSize {
                price: dec!(100),
                size: dec!(13),
            }
        );
    }

    #[test]
    fn test_insert_ask() {
        let mut order_book = OrderBook::new();

        // Insert first ask
        order_book.insert_ask(dec!(110), dec!(10));
        assert_eq!(order_book.asks.len(), 1);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(110),
                size: dec!(10),
            }
        );

        // Insert higher price ask (should be first)
        order_book.insert_ask(dec!(120), dec!(5));
        assert_eq!(order_book.asks.len(), 2);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(120),
                size: dec!(5),
            }
        );
        assert_eq!(
            order_book.asks[1],
            PriceSize {
                price: dec!(110),
                size: dec!(10),
            }
        );

        // Insert same price (should add to existing)
        order_book.insert_ask(dec!(110), dec!(3));
        assert_eq!(order_book.asks.len(), 2);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(120),
                size: dec!(5),
            }
        );
        assert_eq!(
            order_book.asks[1],
            PriceSize {
                price: dec!(110),
                size: dec!(13),
            }
        );
    }

    #[test]
    fn test_get_best_prices() {
        let mut order_book = OrderBook::new();

        // Empty order book
        assert!(order_book.get_best_bid().is_none());
        assert!(order_book.get_best_ask().is_none());

        // Add some orders
        order_book.insert_bid(dec!(100), dec!(10));
        order_book.insert_bid(dec!(95), dec!(5));
        order_book.insert_ask(dec!(110), dec!(8));
        order_book.insert_ask(dec!(105), dec!(3));

        // Check best prices
        assert_eq!(
            order_book.get_best_bid(),
            Some(&PriceSize {
                price: dec!(100),
                size: dec!(10),
            })
        );
        assert_eq!(
            order_book.get_best_ask(),
            Some(&PriceSize {
                price: dec!(105),
                size: dec!(3),
            })
        );
    }

    #[test]
    fn test_zero_size_update() {
        let mut order_book = OrderBook::new();
        let update = MarketUpdate::new(dec!(100), dec!(0), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(result.placed, None);
    }

    #[test]
    fn test_cancel_bid() {
        let mut order_book = OrderBook::new();
        order_book.insert_bid(dec!(100), dec!(10));
        order_book.insert_bid(dec!(95), dec!(5));

        // Cancel partial bid
        let update = MarketUpdate::new(dec!(100), dec!(-3), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(100),
                size: dec!(-3),
            })
        );
        assert_eq!(
            order_book.bids[1],
            PriceSize {
                price: dec!(100),
                size: dec!(7),
            }
        );

        // Cancel full bid
        let update = MarketUpdate::new(dec!(100), dec!(-7), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(100),
                size: dec!(-7),
            })
        );
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(95),
                size: dec!(5),
            }
        );
    }

    #[test]
    fn test_cancel_ask() {
        let mut order_book = OrderBook::new();
        order_book.insert_ask(dec!(110), dec!(10));
        order_book.insert_ask(dec!(105), dec!(5));

        // Cancel partial ask
        let update = MarketUpdate::new(dec!(110), dec!(-3), Direction::Ask);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(110),
                size: dec!(-3),
            })
        );
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(110),
                size: dec!(7),
            }
        );

        // Cancel full ask
        let update = MarketUpdate::new(dec!(110), dec!(-7), Direction::Ask);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(110),
                size: dec!(-7),
            })
        );
        assert_eq!(order_book.asks.len(), 1);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(105),
                size: dec!(5),
            }
        );
    }

    #[test]
    fn test_cancel_insufficient_size() {
        let mut order_book = OrderBook::new();
        order_book.insert_bid(dec!(100), dec!(5));

        let update = MarketUpdate::new(dec!(100), dec!(-10), Direction::Bid);

        let result = order_book.update(update);
        assert!(matches!(result, Err(Error::InsufficientSize(_))));
    }

    #[test]
    fn test_cancel_price_not_found() {
        let mut order_book = OrderBook::new();
        order_book.insert_bid(dec!(100), dec!(5));

        let update = MarketUpdate::new(dec!(95), dec!(-3), Direction::Bid);

        let result = order_book.update(update);
        assert!(matches!(result, Err(Error::PriceLevelNotFound(_))));
    }

    #[test]
    fn test_bid_matches_asks() {
        let mut order_book = OrderBook::new();
        order_book.insert_ask(dec!(105), dec!(5));
        order_book.insert_ask(dec!(110), dec!(3));
        order_book.insert_ask(dec!(100), dec!(2));

        // Bid that matches multiple asks
        let update = MarketUpdate::new(dec!(108), dec!(8), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert_eq!(result.executed.len(), 2);
        assert_eq!(
            result.executed[0],
            PriceSize {
                price: dec!(100),
                size: dec!(2),
            }
        ); // Best ask first
        assert_eq!(
            result.executed[1],
            PriceSize {
                price: dec!(105),
                size: dec!(5),
            }
        );
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(108),
                size: dec!(1),
            })
        );
        // Check remaining ask
        assert_eq!(order_book.asks.len(), 1);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(110),
                size: dec!(3),
            }
        );

        // Check placed bid
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(108),
                size: dec!(1),
            }
        );
    }

    #[test]
    fn test_ask_matches_bids() {
        let mut order_book = OrderBook::new();
        order_book.insert_bid(dec!(95), dec!(5));
        order_book.insert_bid(dec!(90), dec!(3));
        order_book.insert_bid(dec!(100), dec!(2));

        // Ask that matches multiple bids
        let update = MarketUpdate::new(dec!(92), dec!(8), Direction::Ask);

        let result = order_book.update(update).unwrap();
        assert_eq!(result.executed.len(), 2);
        assert_eq!(
            result.executed[0],
            PriceSize {
                price: dec!(100),
                size: dec!(2),
            }
        ); // Best bid first
        assert_eq!(
            result.executed[1],
            PriceSize {
                price: dec!(95),
                size: dec!(5),
            }
        );
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(92),
                size: dec!(1),
            })
        );
        // Check remaining bid
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(90),
                size: dec!(3),
            }
        );

        // Check placed ask
        assert_eq!(order_book.asks.len(), 1);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(92),
                size: dec!(1),
            }
        );
    }

    #[test]
    fn test_bid_no_match() {
        let mut order_book = OrderBook::new();
        order_book.insert_ask(dec!(110), dec!(5));

        // Bid too low to match
        let update = MarketUpdate::new(dec!(105), dec!(10), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(105),
                size: dec!(10),
            })
        );
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(105),
                size: dec!(10),
            }
        );
        assert_eq!(order_book.asks.len(), 1);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(110),
                size: dec!(5),
            }
        );
    }

    #[test]
    fn test_ask_no_match() {
        let mut order_book = OrderBook::new();
        order_book.insert_bid(dec!(95), dec!(5));

        // Ask too high to match
        let update = MarketUpdate::new(dec!(100), dec!(10), Direction::Ask);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(100),
                size: dec!(10),
            })
        );
        assert_eq!(order_book.asks.len(), 1);
        assert_eq!(
            order_book.asks[0],
            PriceSize {
                price: dec!(100),
                size: dec!(10),
            }
        );
        assert_eq!(order_book.bids.len(), 1);
        assert_eq!(
            order_book.bids[0],
            PriceSize {
                price: dec!(95),
                size: dec!(5),
            }
        );
    }

    #[test]
    fn test_exact_match() {
        let mut order_book = OrderBook::new();
        order_book.insert_ask(dec!(100), dec!(5));
        order_book.insert_bid(dec!(100), dec!(3));

        // Bid at exact price
        let update = MarketUpdate::new(dec!(100), dec!(2), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert_eq!(result.executed.len(), 1);
        assert_eq!(
            result.executed[0],
            PriceSize {
                price: dec!(100),
                size: dec!(2),
            }
        );
        assert_eq!(result.placed, None);

        // Ask at exact price
        let update = MarketUpdate::new(dec!(100), dec!(1), Direction::Ask);

        let result = order_book.update(update).unwrap();
        assert_eq!(result.executed.len(), 1);
        assert_eq!(
            result.executed[0],
            PriceSize {
                price: dec!(100),
                size: dec!(1),
            }
        );
        assert_eq!(result.placed, None);
    }

    #[test]
    fn test_complex_scenario() {
        let mut order_book = OrderBook::new();

        // Setup initial state
        order_book.insert_bid(dec!(95), dec!(10));
        order_book.insert_bid(dec!(90), dec!(5));
        order_book.insert_ask(dec!(105), dec!(8));
        order_book.insert_ask(dec!(110), dec!(3));

        // Large bid that matches and places
        let update = MarketUpdate::new(dec!(108), dec!(15), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert_eq!(result.executed.len(), 1);
        assert_eq!(
            result.executed[0],
            PriceSize {
                price: dec!(105),
                size: dec!(8),
            }
        );
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(108),
                size: dec!(7),
            })
        );
        assert_eq!(
            order_book.bids,
            vec![
                PriceSize {
                    price: dec!(90),
                    size: dec!(5),
                },
                PriceSize {
                    price: dec!(95),
                    size: dec!(10),
                },
                PriceSize {
                    price: dec!(108),
                    size: dec!(7),
                },
            ]
        );
        assert_eq!(
            order_book.asks,
            vec![PriceSize {
                price: dec!(110),
                size: dec!(3),
            }]
        );
    }

    #[test]
    fn test_edge_cases() {
        let mut order_book = OrderBook::new();

        // Very large order
        order_book.insert_ask(dec!(100), dec!(1000));
        let update = MarketUpdate::new(dec!(101), dec!(999), Direction::Bid);

        let result = order_book.update(update).unwrap();
        assert_eq!(result.executed.len(), 1);
        assert_eq!(
            result.executed[0],
            PriceSize {
                price: dec!(100),
                size: dec!(999),
            }
        );
        assert_eq!(result.placed, None);
        assert_eq!(
            order_book.asks,
            vec![PriceSize {
                price: dec!(100),
                size: dec!(1),
            }]
        );

        // Cancel to zero
        let update = MarketUpdate::new(dec!(100), dec!(-1), Direction::Ask);

        let result = order_book.update(update).unwrap();
        assert!(result.executed.is_empty());
        assert_eq!(
            result.placed,
            Some(PriceSize {
                price: dec!(100),
                size: dec!(-1),
            })
        );
        assert!(order_book.asks.is_empty());
    }

    #[test]
    fn test_debug_matching() {
        let mut order_book = OrderBook::new();
        order_book.insert_ask(dec!(110), dec!(3));
        order_book.insert_ask(dec!(105), dec!(8));

        println!("Initial asks: {:?}", order_book.asks);

        let update = MarketUpdate::new(dec!(108), dec!(15), Direction::Bid);

        let result = order_book.update(update).unwrap();
        println!("Executed: {:?}", result.executed);
        println!("Placed: {:?}", result.placed);
        println!("Remaining asks: {:?}", order_book.asks);
        println!("Bids: {:?}", order_book.bids);
    }
}
