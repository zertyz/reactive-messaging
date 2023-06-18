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

use super::logic::models::*;
use serde::{Serialize, Deserialize};

/// Messages coming from the clients, suitable to be deserialized by the server
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessages {

    /// First message that the client must send, upon connecting
    Config(MatchConfig),

    PingPongEvent,

    /// Sent when the client request an immediate termination of the game
    Stop,
    EndorsedScore,
    ContestedScore(MatchScore),
}

/// Messages coming from the server, suitable to be deserialized by the clients
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessages {

    /// Issued after a [ClientMessages::Config] has been received, indicating the client should service the first ball of the game
    GameStarted,

    PingPongEvent,
}

