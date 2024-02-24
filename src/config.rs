//! Contains constants and other configuration information affecting default & fixed behaviors of this library

use std::{ops::RangeInclusive, num::NonZeroU8};
use std::time::Duration;
use reactive_mutiny::prelude::Instruments;


/// Specifies the message form to use for communications:
///   - Textual messages end in '\n';
///   - Binary messages are prefixed 4 bytes specifying the payload size
///   - Binary messages may, optionally, have a fixed size -- avoiding the 4 bytes prefix
#[derive(Debug,PartialEq)]
pub enum MessageForms {
    /// Specifies that messages will be sent & received in the textual form -- model types should
    /// implement [ReactiveMessagingSerializer] & [ReactiveMessagingDeserializer] traits appropriately.\
    /// The given `max_size` value is used to pre-allocate the sender/receiver buffers (must be a power of 2).
    /// Set it wisely: if too small, an error will be issued and the connection will be closed; if too big, RAM
    /// will be wasted & processing times may increase slightly.
    Textual { max_size: u32 },
    /// Specifies that messages will be sent in the binary form with a variable size -- possibly serialized by `rkyv`, which enables
    /// zero-copy deserialization while still allowing strings, hashmaps, hashsets, vectors, etc.\
    /// Each payload will be preceded by a `u32` indicating its exact size.\
    /// The given `max_size`, if non-zero, will deny messages above the specified value -- which must be a power of 2
    /// -- sending a feedback and closing the connection.\
    /// Models are required to override the appropriate functions of the [ReactiveMessagingSerializer] &
    /// [ReactiveMessagingDeserializer] traits.
    VariableBinary { max_size: u32 },
    /// Specifies that messages will be sent in the binary form -- and that each message will have the same
    /// `size` -- enabling zero-copy serialization/deserialization.\
    /// Notice that this form avoids having the payload preceded by an `u32` indicating its size, making it optimal for
    /// models containing small raw types & subtypes containing raw types themselves.\
    /// `size` must be a power of 2 and models are required to override the appropriate functions of the
    /// [ReactiveMessagingSerializer] & [ReactiveMessagingDeserializer] traits.
    FixedBinary { size: u32 },
}
impl MessageForms {
    /// requires 2+5=7 bits to represent the data; reverse of [Self::from_repr()]
    const fn as_repr(&self) -> u8 {
        (match self {
            Self::Textual { max_size }        => set_bits_from_power_of_2_u32(0, 2..=6, *max_size),
            Self::VariableBinary { max_size } => set_bits_from_power_of_2_u32(1, 2..=6, *max_size),
            Self::FixedBinary { size }        => set_bits_from_power_of_2_u32(2, 2..=6, *size),
        }) as u8
    }
    /// reverse of [Self::as_repr()]
    const fn from_repr(repr: u8) -> Self {
        let (variant, n) = (repr & 0x03, get_power_of_2_u32_bits(repr as u64, 2..=6));
        match variant {
            0 => Self::Textual        { max_size: n  },
            1 => Self::VariableBinary { max_size: n },
            2 => Self::FixedBinary    { size: n },
            _ => unreachable!(),    // If this errors out, was a new enum member added?
        }
    }
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

    /// Retries, in case of "buffer is full" errors, ending the communications if success still can't be achieved.\
    /// Uses an Exponential Backoff strategy with factor 2.526 and 20% jitter, giving the milliseconds to sleep between,
    /// at most, the given number of attempts.\
    /// The total retrying time would be the sum of the geometric progression: (-1+2.526^n)/(1.526) -- in milliseconds.\
    /// Example: for up to 5 minutes retrying, use 14 attempts.
    RetryWithBackoffUpTo(u8),

    /// Retries, in case of "buffer is full" errors, ending the communications if success still can't be achieved
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
            Self::RetryWithBackoffUpTo(n)        => 2 | (*n as u16) << 3,
            Self::RetryYieldingForUpToMillis(n)  => 3 | (*n as u16) << 3,
            Self::RetrySpinningForUpToMillis(n)  => 4 | (*n as u16) << 3,
        }
    }
    /// reverse of [Self::as_repr()]
    const fn from_repr(repr: u16) -> Self {
        let (variant, n) = (repr & 0x07, repr >> 3);
        match variant {
            0 => Self::DoNotRetry,
            1 => Self::EndCommunications,
            2 => Self::RetryWithBackoffUpTo(n as u8),
            3 => Self::RetryYieldingForUpToMillis(n as u8),
            4 => Self::RetrySpinningForUpToMillis(n as u8),
            _ => unreachable!(),    // If this errors out, was a new enum member added?
        }
    }
}


/// Socket options for the local peer to be set when the connection is established
#[derive(Debug,PartialEq)]
pub struct SocketOptions {
    /// Also known as time-to-live (TTL), specifies how many hops may relay an outgoing packet before it being dropped and an error being returned.\
    /// If NonZero, will cause the socket configuration function to be called with that value.
    pub hops_to_live: Option<NonZeroU8>,
    /// If specified, must be a power of 2 with the number of milliseconds to wait for any unsent messages when closing the connection.\
    /// In Linux, defaults to 0.
    pub linger_millis: Option<u32>,
    /// Set this to `true` if lower latency is preferred over throughput; `false` (default on Linux) to use all the available bandwidth
    /// (sending full packets, waiting up to 200ms for fulfillment).\
    /// `None` will leave it as the system's default -- in Linux, false.
    /// Some hints:
    ///   - The peer reporting events may prefer to set it to `true`;
    ///   - The other peer, receiving events from many, many peers and sending messages that won't be used for decision-making, may set it to `false`
    pub no_delay: Option<bool>,
}
impl SocketOptions {
    /// requires 8+(1+5)+(1+1)=16 bits to represent the data; reverse of [Self::from_repr()]
    const fn as_repr(&self) -> u32 {
        (self.no_delay.is_some() as u32)                                                        // << 0       // no delay flag
        | (unwrap_bool_or_default(self.no_delay) as u32)                                           << 1       // no delay data
        | (self.linger_millis.is_some() as u32)                                                    << 2       // linger flag
        | if let Some(linger_millis) = self.linger_millis {
              if linger_millis > 0 {
                set_bits_from_power_of_2_u32(0, 3..=7, linger_millis) as u32        // 3..=7       // linger data
              } else {
                0
              }
          } else {
            0
          }
        | (unwrap_non_zero_u8_or_default(self.hops_to_live) as u32)                                << 8      // hops
    }
    /// reverse of [Self::as_repr()]
    const fn from_repr(repr: u32) -> Self {
        let (no_delay_flag, no_delay_data, linger_flag, linger_data, hops) = (
            (repr & (1 << 0) ) > 0,                             // no delay flag
            (repr & (1 << 1) ) > 0,                             // no delay data
            (repr & (1 << 2) ) > 0,                             // linger flag
            get_power_of_2_u32_bits(repr as u64, 3..=7),   // power of 32 bits linger data
            (repr & (( (!0u8)  as u32) << 8) ) >> 8,            // raw 8 bits hops data
        );
        Self {
            hops_to_live:  if hops > 0      { NonZeroU8::new(hops as u8) } else { None },
            linger_millis: if linger_flag   { Some(linger_data) }          else { None },
            no_delay:      if no_delay_flag { Some(no_delay_data) }        else { None },
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
    /// How many local messages (per peer) may be enqueued while awaiting delivery to the remote party (power of 2)
    pub sender_channel_size: u32,
    /// How many remote messages (per peer) may be enqueued while awaiting processing by the local processor (power of 2)
    pub receiver_channel_size: u32,
    /// How many milliseconds to wait when flushing messages out to a to-be-closed connection.
    pub flush_timeout_millis: u16,
    /// Specifies what to do when operations fail (full buffers / connection droppings)
    pub retrying_strategy: RetryingStrategies,
    /// Messes with the low level (system) socket options
    pub socket_options: SocketOptions,
    /// Tells if messages should be sent in TEXTUAL or BINARY forms
    pub message_form: MessageForms,
    /// Allows changing the Stream executor options in regard to logging & collected/reported metrics
    pub executor_instruments: /*reactive_mutiny::*/Instruments,
}

#[warn(non_snake_case)]
impl ConstConfig {

    #![allow(non_snake_case)]   // some consts accepts parameters... _/o\_

    // the consts here determine what bits they use
    // and may also specify ranges for store data (rather than just flags)

    /// u32_value = 2^n
    const SENDER_CHANNEL_SIZE: RangeInclusive<usize> = 0..=4;
    /// u32_value = 2^n
    const RECEIVER_CHANNEL_SIZE: RangeInclusive<usize> = 5..=9;
    /// u16_value = 2^n
    const FLUSH_TIMEOUT_MILLIS: RangeInclusive<usize> = 10..=13;
    /// One of [RetryingStrategies], converted by [RetryingStrategies::as_repr()]
    const RETRYING_STRATEGY: RangeInclusive<usize> = 14..=24;
    /// One of [SocketOptions], converted by [SocketOptions::as_repr()]
    const SOCKET_OPTIONS: RangeInclusive<usize> = 25..=40;
    /// One of [MessageForms], converted by [MessageForms::as_repr()]
    const MESSAGE_FORM: RangeInclusive<usize> = 41..=47;
    /// The 8 bits from `reactive-mutiny`
    const EXECUTOR_INSTRUMENTS: RangeInclusive<usize> = 48..=56;


    /// Contains sane & performant defaults.\
    /// Usage example:
    /// ```nocompile
    ///  const CONFIG: ConstConfig = ConstConfig {
    ///     receiver_buffer: 1024,
    ///     ..ConstConfig::default()
    /// };
    pub const fn default() -> ConstConfig {
        ConstConfig {
            sender_channel_size:    1024,
            receiver_channel_size:  1024,
            flush_timeout_millis:   256,
            retrying_strategy:      RetryingStrategies::RetryWithBackoffUpTo(20),
            socket_options:         SocketOptions { hops_to_live: NonZeroU8::new(255), linger_millis: Some(128), no_delay: Some(true) },
            message_form:           MessageForms::Textual { max_size: 1024 },
            executor_instruments:   Instruments::from(Instruments::NoInstruments.into()),
        }
    }

    /// For use when instantiating a generic struct that uses the "Const Config Pattern"
    /// -- when choosing a pre-defined configuration.\
    /// See also [Self::custom()].\
    /// Example:
    /// ```nocompile
    ///     see bellow
    pub const fn into(self) -> u64 {
        let mut config = 0u64;
        config = set_bits_from_power_of_2_u32(config, Self::SENDER_CHANNEL_SIZE,   self.sender_channel_size);
        config = set_bits_from_power_of_2_u32(config, Self::RECEIVER_CHANNEL_SIZE, self.receiver_channel_size);
        config = set_bits_from_power_of_2_u16(config, Self::FLUSH_TIMEOUT_MILLIS,  self.flush_timeout_millis);
        let retrying_strategy_repr = self.retrying_strategy.as_repr();
        config = set_bits(config, Self::RETRYING_STRATEGY, retrying_strategy_repr as u64);
        let socket_options_repr = self.socket_options.as_repr();
        config = set_bits(config, Self::SOCKET_OPTIONS, socket_options_repr as u64);
        let message_form_repr = self.message_form.as_repr();
        config = set_bits(config, Self::MESSAGE_FORM, message_form_repr as u64);
        let executor_instruments_repr = self.executor_instruments.into();
        config = set_bits(config, Self::EXECUTOR_INSTRUMENTS, executor_instruments_repr as u64);
        config
    }

    /// Builds [Self] from the generic `const CONFIGS: usize` parameter used in structs
    /// by the "Const Config Pattern"
    pub const fn from(config: u64) -> Self {
        let sender_buffer              = get_power_of_2_u32_bits(config, Self::SENDER_CHANNEL_SIZE);
        let receiver_buffer            = get_power_of_2_u32_bits(config, Self::RECEIVER_CHANNEL_SIZE);
        let flush_timeout_millis       = get_power_of_2_u16_bits(config, Self::FLUSH_TIMEOUT_MILLIS);
        let retrying_strategy_repr     = get_bits(config, Self::RETRYING_STRATEGY);
        let socket_options_repr        = get_bits(config, Self::SOCKET_OPTIONS);
        let message_form_repr          = get_bits(config, Self::MESSAGE_FORM);
        let executor_instruments_repr  = get_bits(config, Self::EXECUTOR_INSTRUMENTS);
        Self {
            flush_timeout_millis,
            sender_channel_size:   sender_buffer,
            receiver_channel_size: receiver_buffer,
            retrying_strategy:     RetryingStrategies::from_repr(retrying_strategy_repr as u16),
            socket_options:        SocketOptions::from_repr(socket_options_repr as u32),
            message_form:          MessageForms::from_repr(message_form_repr as u8),
            executor_instruments:  Instruments::from(executor_instruments_repr as usize),
        }
    }

    // query functions for business logic configuration attributes
    //////////////////////////////////////////////////////////////
    // to be used by the struct in which the generic `const CONFIGS: usize` resides

    pub const fn extract_sender_channel_size(config: u64) -> u32 {
        let config = Self::from(config);
        config.sender_channel_size
    }

    pub const fn extract_receiver_channel_size(config: u64) -> u32 {
        let config = Self::from(config);
        config.receiver_channel_size
    }

    pub const fn extract_flush_timeout(config: u64) -> Duration {
        let config = Self::from(config);
        Duration::from_millis(config.flush_timeout_millis as u64)
    }

    pub const fn extract_retrying_strategy(config: u64) -> RetryingStrategies {
        let config = Self::from(config);
        config.retrying_strategy
    }

    pub const fn extract_socket_options(config: u64) -> SocketOptions {
        let config = Self::from(config);
        config.socket_options
    }

    pub const fn extract_message_form(config: u64) -> MessageForms {
        let config = Self::from(config);
        config.message_form
    }

    pub const fn extract_executor_instruments(config: u64) -> usize {
        let config = Self::from(config);
        config.executor_instruments.into()
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
const fn _get_power_of_3_u8_bits(config: u64, bits: RangeInclusive<usize>) -> u8 {
    let value = get_bits(config, bits);
    1 << value
}

/// Packs, optimally, the `power_of_2_u8_value` into 3 `bits`, returning the new value for the given `config`
const fn _set_bits_from_power_of_2_u8(config: u64, bits: RangeInclusive<usize>, power_of_2_u8_value: u8) -> u64 {
    if power_of_2_u8_value.is_power_of_two() {
        set_bits(config, bits, power_of_2_u8_value.ilog2() as u64)
    } else {
        // "The value must be a power of 2"
        unreachable!();
    }
}

// const versions of some `Option<>` functions
//////////////////////////////////////////////

/// same as Option::<bool>::unwrap_or(false), but const
const fn unwrap_bool_or_default(option: Option<bool>) -> bool {
    match option {
        Some(v) => v,
        None => false,
    }
}
/// same as Option::<u8>::unwrap_or(0), but const
const fn _unwrap_u8_or_default(option: Option<u8>) -> u8 {
    match option {
        Some(v) => v,
        None => 0,
    }
}
/// same as Option::<u16>::unwrap_or(0), but const
const fn _unwrap_u16_or_default(option: Option<u16>) -> u16 {
    match option {
        Some(v) => v,
        None => 0,
    }
}
/// same as Option::<u32>::unwrap_or(0), but const
const fn _unwrap_u32_or_default(option: Option<u32>) -> u32 {
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
            (0..8).map(|n| RetryingStrategies::RetryWithBackoffUpTo(1<<n)).collect::<Vec<_>>().into_iter(),
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
                SocketOptions { hops_to_live: None,                 linger_millis: None,    no_delay: None},
                SocketOptions { hops_to_live: None,                 linger_millis: None,    no_delay: Some(false)},
                SocketOptions { hops_to_live: None,                 linger_millis: None,    no_delay: Some(true)},
                SocketOptions { hops_to_live: None,                 linger_millis: Some(1), no_delay: None},
                SocketOptions { hops_to_live: NonZeroU8::new(1), linger_millis: None,    no_delay: None},
            ].into_iter(),
            (0..31).map(|n| SocketOptions { hops_to_live: None,                    linger_millis: Some(1<<n), no_delay: None }).collect::<Vec<_>>().into_iter(),
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
            sender_channel_size:   2048,
            receiver_channel_size: 2048,
            flush_timeout_millis:  256,
            retrying_strategy:     RetryingStrategies::RetryWithBackoffUpTo(14),
            socket_options:        SocketOptions { hops_to_live: NonZeroU8::new(255), linger_millis: Some(128), no_delay: Some(true) },
            message_form:          MessageForms::Textual { max_size: 65536 },
            executor_instruments:  Instruments::from(Instruments::LogsWithExpensiveMetrics.into()),
        };
        let converted = ConstConfig::into(expected());
        let reconverted = ConstConfig::from(converted);
        assert_eq!(reconverted, expected(), "FAILED: {:?} (repr: 0x{:x}); reconverted: {:?}", expected(), converted, reconverted);
    }

}
