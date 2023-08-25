//! Contains constants and other configuration information affecting default & fixed behaviors of this library

use std::{ops::RangeInclusive, num::NonZeroU8};
use std::time::Duration;
use reactive_mutiny::prelude::Instruments;
use strum_macros::FromRepr;


/// Specifies the channels (queues) from `reactive-mutiny` thay may be used to send/receive data.\
/// On different hardware, the performance characteristics may vary.
#[derive(Debug,PartialEq,FromRepr)]
pub enum Channels {
    Atomic,
    FullSync,
    Crossbeam,
}

/// Specifies how to behave when communication failures happen
#[derive(Debug,PartialEq)]
pub enum RetryingStrategies {

    /// Simply ignore full buffer failures denials of sending & receiving messages, without retrying nor dropping the connection.\
    /// This option is acceptable when missing messages don't disrupt the communications and when low latencies / realtime-ish behavior is required.\
    /// Set [ConstConfig::sender_buffer] & [ConstConfig::receiver_buffer] accordingly.
    DoNotRetry,

    /// Drops the connection on "buffer is full" errors, also without retrying
    EndCommunications,

    /// Retries, in case of "buffer is full" errors, ending the communications if success still can't be achieve.\
    /// Starting at 10ms, waits further 10ms on each attempt -- for up to 255 attempts, with 2.55s being the last sleeping time.\
    /// Total retrying time: 5*(n²+n) (milliseconds) -- for up to ~5.5 minutes total retrying time
    RetrySleepingArithmetically(u8),

    /// Retries, in case of "buffer is full" errors, ending the communications if success still can't be achieve
    /// during the specified milliseconds -- during which retrying will be performed in a pool loop, yielding
    /// to tokio before each attempt.\
    /// Use this option if low latency is desirable -- but see also [Self::RetrySleepingArithmetically]
    RetryYieldingForUpToMillis(u8),

    /// Deprecated. Do not use -- to be replaced or removed, as spinning doesn't make sense in this lib
    RetrySpinningForUpToMillis(u8),
    // /// reconnect if dropped? this may go as normal parameter... and on the client only
}
impl RetryingStrategies {
    /// requires 3+8=11 bits to represent the data; reverse of [Self::from_repr()]
    const fn as_repr(&self) -> u16 {
        match self {
            Self::DoNotRetry => 0,
            Self::EndCommunications                    => 1,
            Self::RetrySleepingArithmetically(n) => 2 | (*n as u16) << 3,
            Self::RetryYieldingForUpToMillis(n)  => 3 | (*n as u16) << 3,
            Self::RetrySpinningForUpToMillis(n)  => 4 | (*n as u16) << 3,
        }
    }
    /// reverse of [Self::from_repr()]
    const fn from_repr(repr: u16) -> Self {
        let (variant, n) = (repr & 7, repr >> 3);
        match variant {
            0 => Self::DoNotRetry,
            1 => Self::EndCommunications,
            2 => Self::RetrySleepingArithmetically(n as u8),
            3 => Self::RetryYieldingForUpToMillis(n as u8),
            4 => Self::RetrySpinningForUpToMillis(n as u8),
            _ => unreachable!(),    // If this errors, did a new enum member was added?
        }
    }
}

/// Socket options for the local peer to be set when the connection is stablished
#[derive(Debug,PartialEq)]
pub struct SocketOptions {
    /// Also known as time-to-live (TTL), specifies how many hops may relay an outgoing packet before it being dropped and an error being returned.\
    /// If NonZero, will cause the socket configuration function to be called with that value.
    pub hops_to_live: Option<NonZeroU8>,
    /// If specified, must be a power of 2 with the number of milliseconds to wait for any unsent messages when closing the connection.\
    /// In Linux, defaults to 0.
    pub linger_millis: Option<u32>,
    /// Set this to `true` if lower latency is prefered over throughput; `false` (default on Linux) to use all the available bandwidth
    /// (sending full packets, waiting up to 200ms for fullfilment).\
    /// `None` will leave it as the system's default -- in Linux, false.
    /// Some hints:
    ///   - The peer reporting events may prefer to set it to `true`;
    ///   - The other peer, receiving events from many, many peers and sending messages that won't be used for decision making, may set it to `false`
    pub no_delay: Option<bool>,
}
impl SocketOptions {
    /// requires 8+(1+5)+(1+1)=16 bits to represent the data; reverse of [Self::from_repr()]
    const fn as_repr(&self) -> u32 {
          (self.no_delay.is_some() as u32)                                                                                                 // << 0       // no delay flag
        | (unwrap_bool_or_default(self.no_delay) as u32)                                                                               << 1       // no delay data
        | (self.linger_millis.is_some() as u32)                                                                                               << 2       // linger flag
        | if let Some(linger_millis) = self.linger_millis {
              if linger_millis > 0 {
                set_bits_from_power_of_2_u32(0, 3..=7, linger_millis) as u32                                // 3..=7       // linger data  -- may use set bits
              } else {
                0
              }
          } else {
            0
          }
        | (unwrap_non_zero_u8_or_default(self.hops_to_live) as u32)                                                                    << 8      // hops
    }
    /// reverse of [Self::from_repr()]
    const fn from_repr(repr: u32) -> Self {
        let (no_delay_flag, no_delay_data, linger_flag, linger_data, hops) = (
            (repr & (1 << 0) ) > 0,                                    // no delay flag
            (repr & (1 << 1) ) > 0,                                    // no delay data
            (repr & (1 << 2) ) > 0,                                    // linger flag
            get_power_of_2_u32_bits(repr as u64, 3..=7),   // power of 32 bits linger data
            (repr & (( (!0u8)  as u32) << 8) ) >> 8,                   // raw 8 bits hops data
        );
        Self {
            hops_to_live:  if hops > 0      { NonZeroU8::new(hops as u8) } else { None },
            linger_millis: if linger_flag   { Some(linger_data) }             else { None },
            no_delay:      if no_delay_flag { Some(no_delay_data) }           else { None },
        }
    }
}


/// Implements something that could be called the "Zero-Cost Const Configuration Pattern", that produces a `usize`
/// whose goal is to be the only const parameter of a generic struct (avoiding the alternative of bloating it with several const params).\
/// When using the const "query functions" defined here in `if`s, the compiler will have the opportunity to cancel out any unreachable code (zero-cost abstraction).\
/// Some commonly used combinations may be pre-defined in some enum variants, but you may always build unmapped possibilities through [Self::custom()].\
/// Usage examples:
/// ```nocompile
///     see bellow
#[derive(Debug,PartialEq)]
pub struct ConstConfig {
    /// Pre-allocates the sender/receiver buffers to this value (power of 2).
    /// Setting it wisely may economize some `realloc` calls
    pub msg_size_hint: u32,
    /// How many messages (per peer) may be enqueued for output (power of 2)
    /// before operations start to fail
    pub sender_buffer: u32,
    /// How many messages (per peer) may be enqueued for processing (power of 2)
    /// before operations start to fail
    pub receiver_buffer: u32,
    /// How many milliseconds to wait before giving up waiting for a socket to close.\
    /// Set this taking [SocketOptions::linger_millis] into account
    pub graceful_close_timeout_millis: u16,
    /// Specifies what to do when operations fail (full buffers / connection droppings)
    pub retrying_strategy: RetryingStrategies,
    /// Messes with the low level (system) socket options
    pub socket_options: SocketOptions,
    /// Allows changing the backing queue for the sender/receiver buffers
    pub channel: Channels,
    /// Allows changing the Stream executor options in regard to logging & collected/reported metrics
    pub executor_instruments: /*reactive_mutiny::*/Instruments,
}

#[warn(non_snake_case)]
impl ConstConfig {

    #![allow(non_snake_case)]   // some consts accepts parameters... _/o\_

    // the consts here determine what bits they use
    // and may also specify ranges for store data (rather than just flags)

    /// u32_value = 2^n
    const MSG_SIZE_HINT: RangeInclusive<usize> = 0..=4;
    /// u32_value = 2^n
    const SENDER_BUFFER: RangeInclusive<usize> = 5..=9;
    /// u32_value = 2^n
    const RECEIVER_BUFFER: RangeInclusive<usize> = 10..=14;
    /// u16_value = 2^n
    const GRACEFUL_CLOSE_TIMEOUT_MILLIS: RangeInclusive<usize> = 15..=18;
    /// One of [RetryingStrategies], converted by [RetryingStrategies::as_repr()]
    const RETRYING_STRATEGY: RangeInclusive<usize> = 19..=29;
    /// One of [SocketOptions], converted by [SocketOptions::as_repr()]
    const SOCKET_OPTIONS: RangeInclusive<usize> = 30..=45;
    /// This might be impossible to implement... candidate for removal
    const CHANNEL: RangeInclusive<usize> = 46..=48;
    /// The 8 bits from `reactive-mutiny`
    const EXECUTOR_INSTRUMENTS: RangeInclusive<usize> = 49..=57;


    /// Contains sane & performant defaults.\
    /// Usage example:
    /// ```nocompile
    ///  const CONFIG: ConstConfig = ConstConfig {
    ///     receiver_buffer: 1024,
    ///     ..ConstConfig::default()
    /// };
    pub const fn default() -> ConstConfig {
        ConstConfig {
            msg_size_hint:                  1024,
            sender_buffer:                  1024,
            receiver_buffer:                1024,
            graceful_close_timeout_millis:  256,
            retrying_strategy:              RetryingStrategies::RetrySleepingArithmetically(20),
            socket_options:                 SocketOptions { hops_to_live: NonZeroU8::new(255), linger_millis: Some(128), no_delay: Some(true) },
            channel:                        Channels::Atomic,
            executor_instruments:           Instruments::from(Instruments::NoInstruments.into()),
        }
    }

    /// For use when instantiating a generic struct that uses the "Const Config Pattern"
    /// -- when chosing a pre-defined configuration.\
    /// See also [Self::custom()].\
    /// Example:
    /// ```nocompile
    ///     see bellow
    pub const fn into(self) -> u64 {
        let mut config = 0u64;
        config = set_bits_from_power_of_2_u32(config, Self::MSG_SIZE_HINT,                 self.msg_size_hint);
        config = set_bits_from_power_of_2_u32(config, Self::SENDER_BUFFER,                 self.sender_buffer);
        config = set_bits_from_power_of_2_u32(config, Self::RECEIVER_BUFFER,               self.receiver_buffer);
        config = set_bits_from_power_of_2_u16(config, Self::GRACEFUL_CLOSE_TIMEOUT_MILLIS, self.graceful_close_timeout_millis);
        let retrying_strategy_repr = self.retrying_strategy.as_repr();
        config = set_bits(config, Self::RETRYING_STRATEGY, retrying_strategy_repr as u64);
        let socket_options_repr = self.socket_options.as_repr();
        config = set_bits(config, Self::SOCKET_OPTIONS, socket_options_repr as u64);
        let channel_repr = self.channel as u8;
        config = set_bits(config, Self::CHANNEL, channel_repr as u64);
        let executor_instruments_repr = self.executor_instruments.into();
        config = set_bits(config, Self::EXECUTOR_INSTRUMENTS, executor_instruments_repr as u64);
        config
    }

    /// Builds [Self] from the generic `const CONFIGS: usize` parameter used in structs
    /// by the "Const Config Pattern"
    pub const fn from(config: u64) -> Self {
        let msg_size_hint                 = get_power_of_2_u32_bits(config, Self::MSG_SIZE_HINT);
        let sender_buffer                 = get_power_of_2_u32_bits(config, Self::SENDER_BUFFER);
        let receiver_buffer               = get_power_of_2_u32_bits(config, Self::RECEIVER_BUFFER);
        let graceful_close_timeout_millis = get_power_of_2_u16_bits(config, Self::GRACEFUL_CLOSE_TIMEOUT_MILLIS);
        let retrying_strategy_repr       = get_bits(config, Self::RETRYING_STRATEGY);
        let socket_options_repr          = get_bits(config, Self::SOCKET_OPTIONS);
        let channel_repr                 = get_bits(config, Self::CHANNEL);
        let executor_instruments_repr    = get_bits(config, Self::EXECUTOR_INSTRUMENTS);
        Self {
            msg_size_hint,
            graceful_close_timeout_millis,
            sender_buffer,
            receiver_buffer,
            retrying_strategy:    RetryingStrategies::from_repr(retrying_strategy_repr as u16),
            socket_options:       SocketOptions::from_repr(socket_options_repr as u32),
            channel:              if let Some(channel) = Channels::from_repr(channel_repr as usize) {channel} else {Channels::Atomic},
            executor_instruments: Instruments::from(executor_instruments_repr as usize),
        }
    }

    // query functions for business logic configuration attributes
    //////////////////////////////////////////////////////////////
    // to be used by the struct in which the generic `const CONFIGS: usize` resides

    pub const fn extract_receiver_buffer(config: u64) -> u32 {
        let config = Self::from(config);
        config.receiver_buffer
    }

    pub const fn extract_executor_instruments(config: u64) -> usize {
        let config = Self::from(config);
        config.executor_instruments.into()
    }

    pub const fn extract_msg_size_hint(config: u64) -> u32 {
        let config = Self::from(config);
        config.msg_size_hint
    }

    pub const fn extract_graceful_close_timeout(config: u64) -> Duration {
        let config = Self::from(config);
        Duration::from_millis(config.graceful_close_timeout_millis as u64)
    }

    pub const fn extract_retrying_strategy(config: u64) -> RetryingStrategies {
        let config = Self::from(config);
        config.retrying_strategy
    }

    pub const fn extract_socket_options(config: u64) -> SocketOptions {
        let config = Self::from(config);
        config.socket_options
    }
}

/// Helper for retrieving data (other than simple flags) from the configuration
/// -- as stored in the specified `bits` by [Self::set_bits()]
const fn get_bits(config: u64, bits: RangeInclusive<usize>) -> u64 {
    let bits_len = *bits.end()-*bits.start()+1;
    (config>>*bits.start()) & ((1<<bits_len)-1)
}

/// Helper for storing data (other than simple flags) in the configuration
/// -- stored in the specified `bits`.\
/// `value` should not be higher than what fits in the bits.\
/// Returns the `configs` with the `value` applied to it in a way it may be retrieved by [Self::get_bits()]
const fn set_bits(mut config: u64, bits: RangeInclusive<usize>, value: u64) -> u64 {
    let bits_len = *bits.end()-*bits.start()+1;
    if value > (1<<bits_len)-1 {
        // "The value specified is above the maximum the reserved bits for it can take"
        unreachable!();
    } else {
        config &= !( ((1<<bits_len)-1) << *bits.start() );   // clear the target bits
        config |= value << *bits.start();                    // set them
        config
    }
}

/// Retrieves 5 `bits` from `configs` that represents a power of 2 over the `u32` space
const fn get_power_of_2_u32_bits(config: u64, bits: RangeInclusive<usize>) -> u32 {
    let value = get_bits(config, bits);
    1 << value
}

/// Packs, optimally, the `power_of_2_u32_value` into 5 `bits`, returning the new value for the given `config`
const fn set_bits_from_power_of_2_u32(config: u64, bits: RangeInclusive<usize>, power_of_2_u32_value: u32) -> u64 {
    if power_of_2_u32_value.is_power_of_two() {
        set_bits(config, bits, power_of_2_u32_value.ilog2() as u64)
    } else {
        // "The value must be a power of 2"
        unreachable!();
    }
}

/// Retrieves 4 `bits` from `configs` that represents a power of 2 over the `u16` space
const fn get_power_of_2_u16_bits(config: u64, bits: RangeInclusive<usize>) -> u16 {
    let value = get_bits(config, bits);
    1 << value
}

/// Packs, optimally, the `power_of_2_u16_value` into 4 `bits`, returning the new value for the given `config`
const fn set_bits_from_power_of_2_u16(config: u64, bits: RangeInclusive<usize>, power_of_2_u16_value: u16) -> u64 {
    if power_of_2_u16_value.is_power_of_two() {
        set_bits(config, bits, power_of_2_u16_value.ilog2() as u64)
    } else {
        // "The value must be a power of 2"
        unreachable!();
    }
}

/// Retrieves 3 `bits` from `configs` that represents a power of 2 over the `u8` space
const fn get_power_of_3_u8_bits(config: u64, bits: RangeInclusive<usize>) -> u8 {
    let value = get_bits(config, bits);
    1 << value
}

/// Packs, optimally, the `power_of_2_u8_value` into 3 `bits`, returning the new value for the given `config`
const fn set_bits_from_power_of_2_u8(config: u64, bits: RangeInclusive<usize>, power_of_2_u8_value: u8) -> u64 {
    if power_of_2_u8_value.is_power_of_two() {
        set_bits(config, bits, power_of_2_u8_value.ilog2() as u64)
    } else {
        // "The value must be a power of 2"
        unreachable!();
    }
}

// const versions of some `Option<>` functions
//////////////////////////////////////////////

/// same as Option::<bool>::unwrap_or(fakse), but const
const fn unwrap_bool_or_default(option: Option<bool>) -> bool {
    match option {
        Some(v) => v,
        None => false,
    }
}
/// same as Option::<u8>::unwrap_or(0), but const
const fn unwrap_u8_or_default(option: Option<u8>) -> u8 {
    match option {
        Some(v) => v,
        None => 0,
    }
}
/// same as Option::<u16>::unwrap_or(0), but const
const fn unwrap_u16_or_default(option: Option<u16>) -> u16 {
    match option {
        Some(v) => v,
        None => 0,
    }
}
/// same as Option::<u32>::unwrap_or(0), but const
const fn unwrap_u32_or_default(option: Option<u32>) -> u32 {
    match option {
        Some(v) => v,
        None => 0,
    }
}

// same as Option::<NonZero*>::map(|v| v.get()).unwrap_or(0), but const
const fn unwrap_non_zero_u8_or_default(option: Option<NonZeroU8>) -> u8 {
    match option {
        Some(v) => v.get(),
        None => 0,
    }
}


/// Unit tests & enforces the requisites of the [stream_executor](self) module.\
/// Tests here mixes manual & automated assertions -- you should manually inspect the output of each one and check if the log outputs make sense
#[cfg(any(test,doc))]
mod tests {
    use super::*;

    #[cfg_attr(not(doc),test)]
    fn retrying_strategies_repr() {
        let subjects = vec![
            vec![
                RetryingStrategies::DoNotRetry,
                RetryingStrategies::EndCommunications,
            ].into_iter(),
            (0..8).map(|n| RetryingStrategies::RetrySleepingArithmetically(1<<n)).collect::<Vec<_>>().into_iter(),
            (0..8).map(|n| RetryingStrategies::RetryYieldingForUpToMillis(1<<n)).collect::<Vec<_>>().into_iter(),
            (0..8).map(|n| RetryingStrategies::RetrySpinningForUpToMillis(1<<n)).collect::<Vec<_>>().into_iter(),
        ].into_iter().flatten();

        for expected in subjects {
            let converted = RetryingStrategies::as_repr(&expected);
            let reconverted = RetryingStrategies::from_repr(converted);
            assert_eq!(reconverted, expected, "FAILED: {:?} (repr: 0x{:x}); reconverted: {:?}", expected, converted, reconverted);
        }
    }

    #[cfg_attr(not(doc),test)]
    fn socket_options_repr() {
        let subjects = vec![
            vec![
                SocketOptions { hops_to_live: None,              linger_millis: None,    no_delay: None},
                SocketOptions { hops_to_live: None,              linger_millis: None,    no_delay: Some(false)},
                SocketOptions { hops_to_live: None,              linger_millis: None,    no_delay: Some(true)},
                SocketOptions { hops_to_live: None,              linger_millis: Some(1), no_delay: None},
                SocketOptions { hops_to_live: NonZeroU8::new(1), linger_millis: None,    no_delay: None},
            ].into_iter(),
            (0..31).map(|n| SocketOptions { hops_to_live: None,                 linger_millis: Some(1<<n), no_delay: None }).collect::<Vec<_>>().into_iter(),
            (0..8) .map(|n| SocketOptions { hops_to_live: NonZeroU8::new(1<<n), linger_millis: None,       no_delay: None }).collect::<Vec<_>>().into_iter(),
        ].into_iter().flatten();

        for expected in subjects {
            let converted = SocketOptions::as_repr(&expected);
            let reconverted = SocketOptions::from_repr(converted);
            assert_eq!(reconverted, expected, "FAILED: {:?} (repr: 0x{:x}); reconverted: {:?}", expected, converted, reconverted);
        }
        // for expected in subjects {
        //     let converted = SocketOptions::into_repr(&expected);
        //     let reconverted = SocketOptions::from_repr(converted);
        //     println!("{:?}: repr: 0x{:x}; worked? {} ---- {:?}", expected, converted, reconverted==expected, reconverted);
        // }

    }

    #[cfg_attr(not(doc),test)]
    fn const_config() {
        let expected = || ConstConfig {
            msg_size_hint:                  1024,
            sender_buffer:                  2048,
            receiver_buffer:                2048,
            graceful_close_timeout_millis:  256,
            retrying_strategy:              RetryingStrategies::RetrySleepingArithmetically(14),
            socket_options:                 SocketOptions { hops_to_live: NonZeroU8::new(255), linger_millis: Some(128), no_delay: Some(true) },
            channel:                        Channels::Atomic,
            executor_instruments:           Instruments::from(Instruments::LogsWithExpensiveMetrics.into()),
        };
        let converted = ConstConfig::into(expected());
        let reconverted = ConstConfig::from(converted);
        assert_eq!(reconverted, expected(), "FAILED: {:?} (repr: 0x{:x}); reconverted: {:?}", expected(), converted, reconverted);
    }

}
