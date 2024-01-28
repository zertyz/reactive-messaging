//! Defines the protocol & messages clients and servers may exchange,
//! as well as serializers & deserializers (either textual or binary).
//!
//! Here goes an hypothetically useful client/server communications which should show the suggested design patterns as well as some nice features of this library:
//!   - How easy and decoupled we can build a protocol, a client logic and a server logic -- as well how to unit test them;
//!   - How to include some nice & advanced features, such as timeouts and protocol dishonor protections;
//!   - How meaningful the provided error messages are, easing the development process of both client and server, as well as debugging;
//!   - How fast and low-latency this library enables their programs to be -- specially when compiled in "Release" mode.
//! Note, specially, how invalid states are simply not representable on our client/server model.
//!
//! In this example, we are in a Ping-Pong match where the client should start serving a ball after connecting and configuring the game.
//! When playing, messages are issued by each peer, which says what happened with the turn, as in the following example:
//!   1) Client connects, configures the game and sends "Service <number>";
//!   2) Server responds either with a "rally message" -- in a back and forth dialog with the client, which goes on until a
//!      "rally ending message" is sent by either of them;
//!   3) Scores are computed and the game continues according to the following possibilities:
//!     3.1) If one of the players reaches the `score_limit` (set in the "configuration message"), the server starts the "end game process";
//!     3.2) The next service responsability will be assigned to the peer who just had their score incremented.
//!
//! If the client fails to progress in the game for more than 1 second, the server will answer with either "No service" or "No rebate" and will score a point
//! -- unless the client fails to configure the game in that period, which is considered a "protocol dishonor".
//!
//! A "rally message" may be preceded by an "informative message" and is one of "Ping <number>", "Pong <number>", or a "rally ending message"
//!
//! A "rally ending message" is one of "Ball out", "Net", "Pre-bounce", "No rebate"
//!
//! An "informative message", if existing, is one of "Let" -- which happens if the ball touches the net but still lands in the opposite field:
//! in a service, this represents a failure with scoring consequences; during the rally, this is just a warning.
//!
//! An "end game process" starts with the server sending "GAME OVER: Score: Server X; Client: Y" and is issued by the server.
//! The client have the responsability of verifying the results and answering with "Endorsed" or "Contested: Score: Server Y; Client: X".
//! After receiving that, the server should close the connection -- which also gets closed if the client doesn't issue a verification within 1 second.
//!
//! At any time, the client may send "Stop", which should have the server to start the "end game process".
//! Not answering for 1 second has the same effect as sending the "Stop" message.
//! If the client fails to adhere to the protocol at any time, the server will answer with "protocol dishonored <meaningful message>" and close the connection, not computing any game statistics.
//!
//! Along with all of the above, the server keeps track of some global statistics (for all clients):
//!   -
//!   - Number of matches played until the end / Number of matches prematurely ended
//!   - Matches won / matches lost
//!   - Points won / Points lost
//!   - Number of accepted games results / Number of contested game results / Number of no responses
//!
//! A "configure" message -- which should be the first message after the connection, contains some constants
//! that are important for the "rally" algorithm both the client and server will play:
//!   - score_limit
//!   - enable_rally_timeouts
//!   - ball_out_probability
//!   - net_touch_probability
//!   - net_blocked_probability
//!   - pre_bounce_probability
//!   - no_rebate_probability
//! Those probabilities are such that, when one peer sends "Ping <number>", "Pong <number>" or "Service <number>", the other peer will
//! simply match that number with all the probabilities above, in that order: if the received number is lower or equal, the rally will end
//! for the triggered reason.

use std::error::Error;
use super::logic::ping_pong_models::*;
use reactive_messaging::prelude::{ResponsiveMessages, ron_deserializer, ron_serializer, ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use serde::{Serialize, Deserialize};


pub const PROTOCOL_VERSION: &str = "2024-01-08";


/// The states the dialog between client and server may be into
/// (used for the "Composite Protocol Stacking" pattern)
#[derive(Debug,Clone)]
pub enum ProtocolStates {
    /// Both client and server are in the "pre-game", awaiting for a negotiated configuration to actually start the game
    PreGame,
    /// PreGame arrangements were not mutually agreed between client and server and a disconnection is about to happen
    Disconnect,
    /// PreGame went fine and the game is going on -- the ball is either in service or in rally
    Game,
}

/// Client messages to setup the game in the server
#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum PreGameClientMessages {

    /// First message that the client must send, upon establishing a connection
    /// -- tells what the game parameters will be.\
    /// The response should be [PreGameServerMessages::Version] and, if both agree, the state progresses [ProtocolStates::Game]
    Config(MatchConfig),

    // standard messages
    ////////////////////

    /// States that the last command wasn't correctly processed or was not recognized as valid
    Error(/*explanation: */String),
    /// Issued by the client's local processor when no answer should be sent back to the server\
    /// -- we don't use this, as our processor is no longer a 1->1 map, but a 1->* flat_map
    #[default]
    NoAnswer,
}

/// Messages coming from the clients after [PreGameClientMessages] protocol was performed, suitable to be deserialized by the server
#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum GameClientMessages {

    /// The rally -- to be started after receiving [GameServerMessages::GameStarted]
    PingPongEvent(PingPongEvent),

    /// Tells the server we agree with the score given by [GameServerMessages::PingPongEvent] -> [PingPongEvent::GameOver] -> [GameOverStates::GracefullyEnded].\
    /// Upon receiving this message, the server must close the connection.
    EndorsedScore,
    /// Tells the server we computed a different score than the one given by [GameServerMessages::PingPongEvent] -> [PingPongEvent::GameOver] -> [GameOverStates::GracefullyEnded].\
    /// Upon receiving this message, the server must close the connection.
    ContestedScore(MatchScore),

    /// The client may inquire the server about the math's config in place -- once the game has started.
    /// The server returns with [GameServerMessages::MatchConfig]
    DumpConfig,

    // standard messages
    ////////////////////

    /// States that the last command wasn't correctly processed or was not recognized as valid
    Error(/*explanation: */String),
    /// Sent at any time, when the client request an immediate termination of the communications.\
    /// It is elegant to send it before abruptly closing the connection, even if we are in the middle of a transaction
    /// -- anyway, if the connection is dropped or faces an error, the "connection events callback" may be instructed to
    /// send this message, so the server processor knows it is time to release the resources associated with this client.
    #[default]
    Quit,
}

/// Messages coming from the server, suitable to be deserialized by the clients
#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum PreGameServerMessages {

    /// After taking notice of a new client -- after it sent [PreGameClientMessages::Config] --
    /// answers to the client which version the server is running.\
    /// After this, the server progresses to the [GameServerMessages] protocol and the
    /// client should do it as well (progressing to [GameClientMessages]) or disconnect if
    /// it doesn't agree with the server version.
    Version(String),

    // standard messages
    ////////////////////

    /// States that the last command wasn't correctly processed or was not recognized as valid
    Error(/*explanation: */String),
    /// Issued by the server's local processor when no answer should be sent back to the client
    #[default]
    NoAnswer,
}

/// Messages coming from the server, suitable to be deserialized by the clients
#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum GameServerMessages {

    /// Issued after a [PreGameClientMessages::Config] has been received, indicating both the client and server should start the game
    GameStarted,

    /// When asked, at any time, through [GameClientMessages::DumpConfig], informs the match config in place
    MatchConfig(MatchConfig),

    /// The rally
    PingPongEvent(PingPongEvent),

    // standard messages
    ////////////////////

    /// States that the last command wasn't correctly processed or was not recognized as valid
    Error(/*explanation: */String),
    /// Issued by the server's local processor when no answer should be sent back to the client
    #[default]
    NoAnswer,
    /// Issued when the server is ending the connection due to gracefully completing the communications
    GoodBye,
    /// Issued when the server was asked to gracefully shutdown: all connected clients will be informed, data will be persisted and pretty much nothing else will be done.
    ServerShutdown,

}

// implementation of the RON SerDe & Responsive traits
//////////////////////////////////////////////////////

impl AsRef<GameClientMessages> for GameClientMessages {
    #[inline(always)]
    fn as_ref(&self) -> &GameClientMessages {
        self
    }
}
impl AsRef<PreGameClientMessages> for PreGameClientMessages {
    #[inline(always)]
    fn as_ref(&self) -> &PreGameClientMessages {
        self
    }
}

impl ReactiveMessagingSerializer<PreGameClientMessages> for PreGameClientMessages {
    #[inline(always)]
    fn serialize(remote_message: &PreGameClientMessages, buffer: &mut Vec<u8>) {
        ron_serializer(remote_message, buffer)
            .expect("BUG in `ReactiveMessagingSerializer<PreGameClientMessages>`. Is the buffer too small?");
    }
    #[inline(always)]
    fn processor_error_message(err: String) -> PreGameClientMessages {
        PreGameClientMessages::Error(err)
    }
}
impl ReactiveMessagingSerializer<GameClientMessages> for GameClientMessages {
    #[inline(always)]
    fn serialize(remote_message: &GameClientMessages, buffer: &mut Vec<u8>) {
        ron_serializer(remote_message, buffer)
            .expect("BUG in `ReactiveMessagingSerializer<GameClientMessages>`. Is the buffer too small?");
    }
    #[inline(always)]
    fn processor_error_message(err: String) -> GameClientMessages {
        GameClientMessages::Error(err)
    }
}

impl ReactiveMessagingDeserializer<PreGameClientMessages> for PreGameClientMessages {
    #[inline(always)]
    fn deserialize(local_message: &[u8]) -> Result<PreGameClientMessages, Box<dyn Error + Sync + Send + 'static>> {
        ron_deserializer(local_message)
    }
}
impl ReactiveMessagingDeserializer<GameClientMessages> for GameClientMessages {
    #[inline(always)]
    fn deserialize(local_message: &[u8]) -> Result<GameClientMessages, Box<dyn Error + Sync + Send + 'static>> {
        ron_deserializer(local_message)
    }
}

impl ResponsiveMessages<PreGameClientMessages> for PreGameClientMessages {
    #[inline(always)]
    fn is_disconnect_message(_processor_answer: &PreGameClientMessages) -> bool {
        false
    }
    #[inline(always)]
    fn is_no_answer_message(processor_answer: &PreGameClientMessages) -> bool {
        matches!(processor_answer, PreGameClientMessages::NoAnswer)
    }
}
impl ResponsiveMessages<GameClientMessages> for GameClientMessages {
    #[inline(always)]
    fn is_disconnect_message(processor_answer: &GameClientMessages) -> bool {
        matches!(processor_answer, GameClientMessages::Quit)
    }
    #[inline(always)]
    fn is_no_answer_message(_processor_answer: &GameClientMessages) -> bool {
        false
    }
}

impl AsRef<PreGameServerMessages> for PreGameServerMessages {
    #[inline(always)]
    fn as_ref(&self) -> &PreGameServerMessages {
        self
    }
}
impl AsRef<GameServerMessages> for GameServerMessages {
    #[inline(always)]
    fn as_ref(&self) -> &GameServerMessages {
        self
    }
}

impl ReactiveMessagingSerializer<PreGameServerMessages> for PreGameServerMessages {
    #[inline(always)]
    fn serialize(local_message: &PreGameServerMessages, buffer: &mut Vec<u8>) {
        ron_serializer(local_message, buffer)
            .expect("BUG in `ReactiveMessagingSerializer<PreServerMessages>`. Is the buffer too small?");
    }
    #[inline(always)]
    fn processor_error_message(err: String) -> PreGameServerMessages {
        PreGameServerMessages::Error(err)
    }
}
impl ReactiveMessagingSerializer<GameServerMessages> for GameServerMessages {
    #[inline(always)]
    fn serialize(local_message: &GameServerMessages, buffer: &mut Vec<u8>) {
        ron_serializer(local_message, buffer)
            .expect("BUG in `ReactiveMessagingSerializer<ServerMessages>`. Is the buffer too small?");
    }
    #[inline(always)]
    fn processor_error_message(err: String) -> GameServerMessages {
        GameServerMessages::Error(err)
    }
}

impl ReactiveMessagingDeserializer<PreGameServerMessages> for PreGameServerMessages {
    fn deserialize(local_message: &[u8]) -> Result<PreGameServerMessages, Box<dyn Error + Sync + Send>> {
        ron_deserializer(local_message)
    }
}impl ReactiveMessagingDeserializer<GameServerMessages> for GameServerMessages {
    fn deserialize(local_message: &[u8]) -> Result<GameServerMessages, Box<dyn Error + Sync + Send>> {
        ron_deserializer(local_message)
    }
}

impl ResponsiveMessages<PreGameServerMessages> for PreGameServerMessages {
    /// Disconnects when our processor issues either of "GoodBye" or "GameCancelled"
    #[inline(always)]
    fn is_disconnect_message(processor_answer: &PreGameServerMessages) -> bool {
        matches!(processor_answer, PreGameServerMessages::Error(..))
    }
    #[inline(always)]
    fn is_no_answer_message(processor_answer: &PreGameServerMessages) -> bool {
        matches!(processor_answer, PreGameServerMessages::NoAnswer)
    }
}
impl ResponsiveMessages<GameServerMessages> for GameServerMessages {
    /// Disconnects when our processor issues either of "GoodBye" or "GameCancelled"
    #[inline(always)]
    fn is_disconnect_message(processor_answer: &GameServerMessages) -> bool {
        matches!(processor_answer, GameServerMessages::GoodBye | GameServerMessages::PingPongEvent(PingPongEvent::GameOver(GameOverStates::GameCancelled { .. })))
    }
    #[inline(always)]
    fn is_no_answer_message(processor_answer: &GameServerMessages) -> bool {
        matches!(processor_answer, GameServerMessages::NoAnswer)
    }
}