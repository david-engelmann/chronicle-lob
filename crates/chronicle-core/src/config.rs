//! Book configuration. Every capacity and the whole tick grid are fixed
//! at construction (ARCHITECTURE.md section 3.2); nothing resizes later,
//! which is what makes the zero steady-state allocation contract
//! possible.

use crate::error::BookError;
use crate::types::StpPolicy;

/// Static configuration for one [`crate::Book`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BookConfig {
    /// Arena capacity: the maximum number of simultaneously live orders.
    /// At least 1 and at most 2^31 - 2, so slot indices can reserve
    /// `u32::MAX` as NIL.
    pub max_orders: u32,
    /// Number of price ticks on the grid. A multiple of 64, at least 64,
    /// at most 2^22.
    pub grid_len: u32,
    /// Quote units per tick, at least 1.
    pub tick_size: u64,
    /// Tick value of grid index 0. A price whose tick is `t` maps to grid
    /// index `t - grid_start_tick`.
    pub grid_start_tick: u64,
    /// Self-trade prevention policy applied when a command does not name
    /// one (executes from Milestone 2).
    pub default_stp: StpPolicy,
}

impl BookConfig {
    /// The tick value one past the last tick on the grid.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::InvalidConfig`] if the grid end overflows
    /// `u64` (the m9a construction bound, ARCHITECTURE.md section 3.2).
    pub fn grid_end_tick(&self) -> Result<u64, BookError> {
        self.grid_start_tick
            .checked_add(u64::from(self.grid_len))
            .ok_or(BookError::InvalidConfig {
                field: "grid_start_tick + grid_len",
                rule: "the grid end must fit in u64 (checked_add at construction)",
            })
    }

    /// Validate every construction rule.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::InvalidConfig`] naming the first field and
    /// rule that fail.
    pub fn validate(&self) -> Result<(), BookError> {
        if self.max_orders == 0 || self.max_orders > (1 << 31) - 2 {
            return Err(BookError::InvalidConfig {
                field: "max_orders",
                rule: "must be at least 1 and at most 2^31 - 2",
            });
        }
        if self.grid_len < 64 || !self.grid_len.is_multiple_of(64) || self.grid_len > 1 << 22 {
            return Err(BookError::InvalidConfig {
                field: "grid_len",
                rule: "must be a multiple of 64, at least 64, at most 2^22",
            });
        }
        if self.tick_size == 0 {
            return Err(BookError::InvalidConfig {
                field: "tick_size",
                rule: "must be at least 1",
            });
        }
        let end = self.grid_end_tick()?;
        // The largest raw price on the grid is (end - 1) * tick_size and
        // must be representable, so price_of_tick can never overflow.
        let highest_tick = end - 1;
        if highest_tick.checked_mul(self.tick_size).is_none() {
            return Err(BookError::InvalidConfig {
                field: "grid end * tick_size",
                rule: "the largest raw price on the grid must fit in u64",
            });
        }
        Ok(())
    }

    /// Raw price (quote units) of an absolute tick value. Callers pass
    /// only ticks inside the validated grid, so the multiplication
    /// cannot overflow (see [`BookConfig::validate`]).
    #[must_use]
    pub(crate) const fn price_of_tick(&self, tick: u64) -> u64 {
        tick * self.tick_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> BookConfig {
        BookConfig {
            max_orders: 1024,
            grid_len: 4096,
            tick_size: 1,
            grid_start_tick: 1,
            default_stp: StpPolicy::CancelIncoming,
        }
    }

    #[test]
    fn valid_config_passes() {
        assert!(base().validate().is_ok());
    }

    #[test]
    fn grid_end_overflow_is_a_config_error() {
        let mut cfg = base();
        cfg.grid_start_tick = u64::MAX - 10;
        cfg.grid_len = 64;
        assert!(matches!(
            cfg.validate(),
            Err(BookError::InvalidConfig { .. })
        ));
    }

    #[test]
    fn grid_len_must_be_a_multiple_of_64() {
        let mut cfg = base();
        cfg.grid_len = 100;
        assert!(matches!(
            cfg.validate(),
            Err(BookError::InvalidConfig { .. })
        ));
    }
}
