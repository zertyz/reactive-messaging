//! Resting place for [SocketConnection] -- our connection wrapper

use std::fmt::Debug;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use tokio::net::TcpStream;


static CONNECTION_COUNTER: AtomicU32 = AtomicU32::new(0);
pub type ConnectionId = u32;


/// A wrapper for a [TcpStream] -- attaching a custom "state" and unique id to it.\
/// This abstraction plays a role in enabling the "Composite Protocol Stacking" design pattern.\
/// IMPLEMENTATION NOTE: The [crate::prelude::Peer] object still holds a copy of the state -- synced elsewhere with us here
///                      -- in the future, this should be reworked.
#[derive(Debug)]
pub struct SocketConnection<StateType: Debug> {
    /// The connection object
    connection: TcpStream,
    /// `true` if a "connection is closed" is reported via [Self::set_closed()]
    closed: bool,
    /// A unique ID for the connection, facilitating protocol processors that need to handle sessions
    id: ConnectionId,
    /// Any state that the protocol processors might attribute to the connection, when using the
    /// "Composite Protocol Stacking" pattern.
    state: StateType,
}

impl<StateType: Debug> SocketConnection<StateType> {

    pub fn new(connection: TcpStream, initial_state: StateType) -> Self {
        Self {
            connection,
            closed: false,
            id:     CONNECTION_COUNTER.fetch_add(1, Relaxed),
            state:  initial_state,
        }
    }

    pub fn connection(&self) -> &TcpStream {
        &self.connection
    }

    pub fn connection_mut(&mut self) -> &mut TcpStream {
        &mut self.connection
    }

    pub fn id(&self) -> ConnectionId {
        self.id
    }

    pub fn state(&self) -> &StateType {
        &self.state
    }

    pub fn set_state(&mut self, new_state: StateType) {
        self.state = new_state;
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    pub fn report_closed(&mut self) {
        self.closed = true;
    }

}