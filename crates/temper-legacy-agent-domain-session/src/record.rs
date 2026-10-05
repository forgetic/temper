//! Concrete session history (domain/agent.md, section 5). Encoding these
//! versioned values into bytes belongs to the protocol layer, never the domain.

use crate::{
    Spec,
    llm::{Endpoint, Message, Usage},
};
use alloc::boxed::Box;

pub const VERSION: u16 = 2;

/// Charter prices per `unit` tokens. Cache writes are new input, cache reads
/// use `cached`. Each completion rounds its exact combined charge upwards.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Prices {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub unit: u32,
}

impl Prices {
    #[must_use]
    pub fn price(self, usage: Usage) -> Option<u64> {
        if self.unit == 0 {
            return None;
        }
        let input = u128::from(usage.input_tokens).checked_add(u128::from(usage.cache_write_tokens))?;
        let input = input.checked_mul(u128::from(self.input))?;
        let cached = u128::from(usage.cache_read_tokens).checked_mul(u128::from(self.cached))?;
        let output = u128::from(usage.output_tokens).checked_mul(u128::from(self.output))?;
        let sum = input.checked_add(cached)?.checked_add(output)?;
        let unit = u128::from(self.unit);
        let rounded = sum.checked_div(unit)?.checked_add(u128::from(sum.checked_rem(unit)? != 0))?;
        u64::try_from(rounded).ok()
    }
}

/// Version two is selected by the event, even with `transcript: None`.
#[derive(PartialEq, Eq, Debug)]
pub struct Opening {
    pub spec: Spec,
    /// Provider dialect/schema identity, paired with the configured endpoint.
    pub dialect: u32,
    pub prices: Prices,
    pub budget: u64,
    pub transcript: Option<Transcript>,
}

/// A settled completion and its surrounding messages. Assistant call names,
/// provider ids and input bytes are concrete; delegated result bytes are too.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Turn {
    pub version: u16,
    pub endpoint: Endpoint,
    pub dialect: u32,
    pub sequence: u32,
    pub usage: Usage,
    /// Cumulative spend of this activation, children included.
    pub spent: u64,
    pub messages: Box<[Message]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Transcript {
    pub version: u16,
    pub endpoint: Endpoint,
    pub dialect: u32,
    pub turns: Box<[Turn]>,
    /// Concrete user results committed after the last told turn.
    pub after: Box<[Message]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    Version,
    Endpoint,
    Dialect,
    Malformed,
    Unresolved,
    TooLarge,
}
