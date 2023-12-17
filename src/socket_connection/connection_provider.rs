//! Abstractions for providing connections to Servers and Clients, with the following purposes:
//!   * Allows both incoming connections as well as upgraded connections (previously originated
//!     in other protocols, such as WebSockets) to be provided -- enabling the "Protocol Stack Composition"
//!     pattern.\
//!     See [ServerConnectionHandler] and the lower level [ConnectionChannel].
//!   * Enables the pattern also for clients -- active connections may either be made or an existing one
//!     may be reused.\
//!     See [ClientConnectionManager]
//!   * Abstracts out the TCP/IP intricacies for establishing (and retrying) connections.
//! IMPLEMENTATION NOTE: this code may be improved when Rust allows "async fn in traits": a common trait
//!                      may be implemented.

use std::future;
use std::future::Future;
use std::iter::Peekable;
use std::net::{SocketAddr, ToSocketAddrs};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::vec::IntoIter;
use keen_retry::{ExponentialJitter, ResolvedResult, RetryProducerResult, RetryResult};
use tokio::net::{ TcpStream, TcpListener };
use log::{trace, error, warn};
use tokio::sync::Mutex;
use crate::config::{ConstConfig, RetryingStrategies};


type ConnectionFuture = Pin < Box < dyn Future < Output=RetryProducerResult<TcpStream, Box<dyn std::error::Error + Sync + Send>> > > >;

/// Abstracts out the TCP/IP intricacies for establishing (and retrying) connections,
/// while still enabling the "Protocol Stack Composition" pattern by accepting existing
/// connections to be provided (instead of opening new ones).
pub struct ClientConnectionManager<const CONFIG_U64: u64> {
    host:                         String,
    port:                         u16,
    connect_continuation_closure: Arc < Mutex < Box<dyn FnMut() -> ConnectionFuture> > >,
}

impl<const CONFIG_U64: u64> ClientConnectionManager<CONFIG_U64> {

    pub fn new<IntoString: Into<String>>(host: IntoString, port: u16) -> Self {
        let host = host.into();
        let connect_closure = Box::new(Self::build_connect_continuation_closure(&host, port));
        Self { host, port, connect_continuation_closure: Arc::new(Mutex::new(connect_closure)) }
    }

    pub async fn connect_retryable(&mut self) -> Result<TcpStream, Box<dyn std::error::Error + Send + Sync>> {
        let config = ConstConfig::from(CONFIG_U64);
        let retry_result_supplier = |retrying_start_time| {
            let mut connect_continuation_closure = self.connect_continuation_closure.try_lock().expect("BUG!!! ERROR LoCkInG!!!!");
            async move {
                connect_continuation_closure().await
                    .map_ok(|_, connection| (Duration::ZERO, connection) )
                    .map_input(|_| retrying_start_time)
            }
        };
        let resolved_result = match config.retrying_strategy {
            RetryingStrategies::DoNotRetry |
            RetryingStrategies::EndCommunications =>
                ResolvedResult::from_retry_result(retry_result_supplier(SystemTime::now()).await),
            RetryingStrategies::RetryWithBackoffUpTo(attempts) =>
                retry_result_supplier(SystemTime::now()).await
                    .retry_with_async(retry_result_supplier)
                    .with_exponential_jitter(|| ExponentialJitter::FromBackoffRange {
                        backoff_range_millis: 1..=(2.526_f32.powi(attempts as i32) as u32),
                        re_attempts: attempts,
                        jitter_ratio: 0.2,
                    })
                    .await,
            RetryingStrategies::RetryYieldingForUpToMillis(millis) =>
                retry_result_supplier(SystemTime::now()).await
                    .retry_with_async(retry_result_supplier)
                    .yielding_until_timeout(Duration::from_millis(millis as u64), || Box::from(format!("Timed out (>{millis}ms) while attempting to connect to {}:{}", self.host, self.port)))
                    .await,
            RetryingStrategies::RetrySpinningForUpToMillis(millis) =>
                todo!("THIS OPTION SHOULD BE REMOVED, AS IT IS NOT SUPPORTED BY KEEN-RETRY")
        };
        resolved_result
            .inspect_recovered(|retrying_duration, _, errors|
               warn!("`reactive-messaging::SocketClient`: Connection to {}:{} SUCCEEDED Succeeded after retrying {} times in {:?}. Transient errors: {}",
                     self.host, self.port, errors.len(), retrying_duration, keen_retry::loggable_retry_errors(&errors)) )
           .inspect_given_up(|retrying_duration, mut transient_errors, fatal_error|
               error!("`reactive-messaging::SocketClient`: Connection to {}:{} was GIVEN UP after retrying {} times in {:?}, with transient errors {}. The last error was {}",
                      self.host, self.port, transient_errors.len()+1, retrying_duration, keen_retry::loggable_retry_errors(&transient_errors), fatal_error) )
           .inspect_unrecoverable(|retrying_duration, transient_errors, fatal_error|
               error!("`reactive-messaging::SocketClient`: Connection to {}:{} FAILED FATABLY after retrying {} times in {:?}, with transient errors {}. The fatal error was {}",
                      self.host, self.port, transient_errors.len(), retrying_duration, keen_retry::loggable_retry_errors(&transient_errors), fatal_error) )
            .into_result()
    }

    /// Consumes this object and returns the underlying connect closure
    fn into_connect_continuation_closure(self) -> Arc < Mutex < Box<dyn FnMut() -> ConnectionFuture> > > {
        self.connect_continuation_closure
    }

    /// Advanced connection procedure suitable for retrying: returns an async closure that does the connection with advanced and special features:
    ///   * If the `server` is a name and it resolves to several IPs, calling the returned closure again will attempt to connect to the next IP
    ///   * If the IPs list is over, a new host resolution will be done and the process above repeats
    ///   * The continuation closure may be indefinitely stored by the client, so an easy reconnection might be attempted at any time, in case it drops.
    /// IMPLEMENTATION NOTE: this method implements the "Partial Completion with Continuation Closure", as described in the `keen-retry` crate's book.
    fn build_connect_continuation_closure(host: &str, port: u16) -> impl FnMut() -> ConnectionFuture {
        let address = format!("{}:{}", host, port);
        let mut opt_addrs: Option<Peekable<IntoIter<SocketAddr>>> = None;
        move || {
            // common code for resolving a host into its addresses
            macro_rules! resolve {
                () => {
                    match address.to_socket_addrs() {
                        Ok(resolved) => {
                            opt_addrs = Some(resolved.peekable());
                            opt_addrs.as_mut().unwrap()
                        },
                        Err(err) => return Box::pin(future::ready(RetryResult::Fatal { input: (), error: Box::from(format!("Unable to resolve address '{}': {}", address, err)) })),
                    }
                };
            }
            let resolved_addrs = if let Some(addrs) = opt_addrs.as_mut() {
                if addrs.peek().is_none() {
                    resolve!()
                } else {
                    addrs
                }
            } else {
                resolve!()
            };

            let socket_addr = resolved_addrs.next().unwrap();
            let address = address.clone();
            Box::pin(async move {
                match TcpStream::connect(socket_addr).await {
                    Ok(socket) => RetryResult::Ok { reported_input: (), output: socket },
                    Err(err) => RetryResult::Transient { input: (), error: Box::from(format!("Couldn't connect to socket address '{socket_addr}' resolved from '{address}': {err}")) },
                }
            })
        }
    }
}


/// Abstracts out, from servers, the connection handling so to enable the "Protocol Stack Composition" pattern:\
/// Binds to a network listening interface and port and starts a network event loop for accepting connections,
/// supplying them to an internal [ConnectionChannel] (while also allowing manually fed connections).
pub struct ServerConnectionHandler {
    connection_channel:          ConnectionChannel,
    listening_interface:         String,
    listening_port:              u16,
    network_event_loop_signaler: tokio::sync::oneshot::Sender<()>,
}

impl ServerConnectionHandler {

    /// Creates a new instance of a server, binding to the specified `listening_interface` and `listening_port`.\
    /// Incoming connections are [feed()] as they arrive -- but you can also do so manually, by calling the mentioned method.
    pub async fn new<IntoString: Into<String>>(listening_interface: IntoString, listening_port: u16) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let listening_interface = listening_interface.into();
        let connection_channel = ConnectionChannel::new();
        let connection_sender = connection_channel.sender.clone();
        let (network_event_loop_sender, network_event_loop_receiver) = tokio::sync::oneshot::channel::<()>();
        Self::spawn_connection_listener(&listening_interface, listening_port, connection_sender, network_event_loop_receiver).await?;
        Ok(Self {
            connection_channel,
            listening_interface,
            listening_port,
            network_event_loop_signaler: network_event_loop_sender,
        })
    }

    /// spawns the server network loop in a new task, possibly returning an error if binding to the specified `listening_interface` and `listening_port` was not allowed.
    async fn spawn_connection_listener(listening_interface:             &str,
                                       listening_port:                  u16,
                                       sender:                          tokio::sync::mpsc::Sender<TcpStream>,
                                       mut network_event_loop_signaler: tokio::sync::oneshot::Receiver<()>)
                                      -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let listening_interface_and_port = format!("{}:{}", listening_interface, listening_port);
        let listener = TcpListener::bind(&listening_interface_and_port).await?;
        tokio::spawn( async move {
            loop {
                // wait for a connection -- or for a shutdown signal
                let (connection, addr) = if let Some(accepted_connection_and_addr) = tokio::select! {
                    // incoming connection
                    acceptance_result = listener.accept() => {
                        if let Err(err) = acceptance_result {
                            error!("`reactive-messaging::IncomingConnectionHandler`: ERROR while accepting a connection for the server @ {listening_interface_and_port}: {:?}", err);
                            None
                        } else {
                            Some(acceptance_result.unwrap())
                        }
                    }
                    // shutdown signal
                    result = &mut network_event_loop_signaler => {
                        match result {
                            Ok(())             => trace!("`reactive-messaging::IncomingConnectionHandler`: SHUTDOWN requested for the server @ {listening_interface_and_port} -- releasing the interface bind and bailing out of the network event loop"),
                            Err(err) => error!("`reactive-messaging::IncomingConnectionHandler`: ERROR in the `shutdown signaler` for the server @ {listening_interface_and_port} (a server shutdown will be commanded now due to this occurrence): {:?}", err),
                        };
                        break
                    }
                } {
                    accepted_connection_and_addr
                } else {
                    // error accepting for a particular client -- not fatal: server is still going
                    continue
                };

                if let Err(unconsumed_connection) = sender.send(connection).await {
                    let client_address = unconsumed_connection.0.peer_addr().map(|peer_addr| peer_addr.to_string()).unwrap_or(String::from("<<couldn't determine the client's address>>"));
                    error!("`reactive-messaging::IncomingConnectionHandler` BUG! -- The server @ {listening_interface_and_port} faced an ERROR when feeding an incoming connection (from '{client_address}') to the 'connections consumer': it had dropped the consumption receiver prematurely. The server's network event loop will be ABORTED and you should expect undefined behavior, as the application thinks the server is still running.");
                    break;
                }
            }
        });
        Ok(())
    }

    /// Consumes and returns the `tokio::sync::mpsc::Receiver` which will able to
    /// provide connections previously sent through [Self::feed_connection()].\
    /// The receiver blocks while there are no connections available and
    /// yields `None` if `self` is dropped -- meaning no more connections
    /// will be feed through the channel.
    pub fn connection_receiver(&mut self) -> Option<tokio::sync::mpsc::Receiver<TcpStream>> {
        self.connection_channel.receiver()
    }

    /// Delivers `connection` to the receiver obtained via a call to [Self::connection_receiver()],
    /// blocking if there are previous connections awaiting delivery
    pub async fn feed_connection(&self, connection: TcpStream) -> Result<(), ReceiverDroppedErr<TcpStream>> {
        self.connection_channel.feed(connection).await
    }

    /// "Shutdown" the connection listener for this server, releasing the bind to the listening interface and port
    /// and bailing out from the network event loop.\
    /// Any consumers using [Self::connection_receiver()] will be notified with a `None` last element.
    pub async fn shutdown(self) {
        _ = self.network_event_loop_signaler.send(());
        self.connection_channel.close().await;
    }

}


/// The abstraction for handling server connections -- here, the connections are
/// provided through a `Stream` instead of through the TCP/IP API directly. This enables
/// the "Protocol Stack Composition" pattern, as already existing connections may be also
/// added to the `Stream` (in addition to fresh incoming ones).\
/// When the end-of-stream is reached (possibly due to a "server shutdown" request),
/// the `Stream` will return `None`.
pub struct ConnectionChannel {
    pub(crate) sender:   tokio::sync::mpsc::Sender<TcpStream>,
               receiver: Option<tokio::sync::mpsc::Receiver<TcpStream>>,
    // throttling may be implemented by using a moving average for the number of opened connections
    // and the statistics struct from `reactive-mutiny` may help here
}

impl ConnectionChannel {

    /// Creates a new instance
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel::<TcpStream>(2);
        Self {
            sender,
            receiver: Some(receiver),
        }
    }

    /// Consumes and returns the `tokio::sync::mpsc::Receiver` which will able to
    /// provide connections previously sent through [Self::feed()].\
    /// The receiver blocks while there are no connections available and
    /// yields `None` if `self` is dropped -- meaning no more connections
    /// will be feed through the channel.
    pub fn receiver(&mut self) -> Option<tokio::sync::mpsc::Receiver<TcpStream>> {
        self.receiver.take()
    }

    /// Delivers `connection` to the receiver obtained via a call to [Self::receiver()],
    /// blocking if there are previous connections awaiting delivery
    pub async fn feed(&self, connection: TcpStream) -> Result<(), ReceiverDroppedErr<TcpStream>> {
        self.sender.send(connection).await
            .map_err(|unconsumed_connection| ReceiverDroppedErr(unconsumed_connection.0))
    }

    /// Returns a cloned version of the sender.\
    /// Notice this method should be considered "advanced", as keeping
    /// a cloned sender will prevent the channel from shutting down,
    /// rendering [Self::close()] useless -- currently there is no way
    /// for `close()` to detect this situation.
    pub fn clone_sender(&self) -> tokio::sync::mpsc::Sender<TcpStream> {
        self.sender.clone()
    }

    /// Closes the channel (by dropping the sender), causing the receiver
    /// produced by [receiver()] to return `None`, indicating the
    /// end-of-stream to the consumer.
    pub async fn close(self) {
        drop(self);
        // give a little time for the receiver to be notified
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Indicates the receiver end of a channel was dropped, therefore the
/// element of type `T` couldn't be sent and is being returned back
/// along with the error indication.\
/// Important: This is an unrecoverable situation, so trying again is futile.
pub struct ReceiverDroppedErr<T>(T);


#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::atomic::Ordering::Relaxed;


    /// Checks that the low level [ConnectionChannel] works according to the specification
    #[tokio::test]
    async fn connection_channel() -> Result<(), Box<dyn std::error::Error>> {
        let expected_count = 10;
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_ref = received_count.clone();
        let stream_ended = Arc::new(AtomicBool::new(false));
        let stream_ended_ref = stream_ended.clone();
        let mut connection_channel = ConnectionChannel::new();
        let mut receiver = connection_channel.receiver().expect("The `receiver` should be available at this point");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;   // proves that the sender will block if the consumer is slower
            while let Some(connection) = receiver.recv().await {
                received_count_ref.fetch_add(1, Relaxed);
            }
            stream_ended_ref.store(true, Relaxed);
        });
        for i in 0..10 {
            let value = TcpStream::connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::from_str("66.45.249.218")?), 80)).await?;
            connection_channel.feed(value).await.unwrap_or_else(|_| panic!("Failed to send value"));
        }
        assert_eq!(stream_ended.load(Relaxed), false, "The connections stream was prematurely closed");
        connection_channel.close().await;
        assert!(stream_ended.load(Relaxed), "The connections stream (on the receiver end) wasn't notified that closing had happened");
        assert_eq!(received_count.load(Relaxed), expected_count, "The wrong number of connections were received");
        Ok(())
    }

    /// Checks that [ServerConnectionHandler] works according to the specification
    #[tokio::test]
    async fn server_connection_handler() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let expected_count = 10 + 1;    // 10 received + 1 feed manually
        let interface = "127.0.0.1";
        let port = 8356;
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_ref = received_count.clone();
        let stream_ended = Arc::new(AtomicBool::new(false));
        let stream_ended_ref = stream_ended.clone();
        let mut server_connection_handler = ServerConnectionHandler::new(interface.to_string(), port).await?;
        let mut connection_receiver = server_connection_handler.connection_receiver().expect("The `receiver` should be available at this point");
        tokio::spawn(async move {
            while let Some(connection) = connection_receiver.recv().await {
                received_count_ref.fetch_add(1, Relaxed);
            }
            stream_ended_ref.store(true, Relaxed);
        });
        for i in 0..10 {
            let value = TcpStream::connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::from_str(interface)?), port)).await?;
            if i == 0 {
                // feed a single extra connection manually, to check that we can do so
                server_connection_handler.feed_connection(value).await.unwrap_or_else(|_| panic!("Failed to send value"));
            }
        }
        assert_eq!(stream_ended.load(Relaxed), false, "The connections stream was prematurely closed");
        server_connection_handler.shutdown().await;
        assert!(stream_ended.load(Relaxed), "The connections stream (on the receiver end) wasn't notified that closing had happened");
        assert_eq!(received_count.load(Relaxed), expected_count, "The wrong number of connections were received");
        Ok(())
    }

    /// Checks that [ClientConnectionManager] works according to the specification
    #[tokio::test]
    async fn client_connection() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let expected_count = 10;
        let interface = "127.0.0.1";
        let port = 8357;
        let received_count = Arc::new(AtomicU32::new(0));
        let received_count_ref = received_count.clone();
        let stream_ended = Arc::new(AtomicBool::new(false));
        let stream_ended_ref = stream_ended.clone();

        // attempt to connect to a non-existing host
        let mut connect_shareable = ClientConnectionManager::<{ConstConfig::default().into()}>::new("non-existing-host.com.br", port)
            .into_connect_continuation_closure();
        let mut connect = connect_shareable
            .lock().await;
        let error_message = connect().await
            .expect_fatal(&format!("Tried to connect to a non-existing host, but the result of a connection attempt was not a `Fatal` error"))
            .into_result()
            .expect_err("A `Result::Err` should have been issued")
            .to_string();
        assert_eq!(error_message, "Unable to resolve address 'non-existing-host.com.br:8357': failed to lookup address information: Name or service not known", "Wrong error message");

        let connect_shareable = ClientConnectionManager::<{ConstConfig::default().into()}>::new(interface, port)
            .into_connect_continuation_closure();
        let mut connect = connect_shareable
            .lock().await;

        // attempt to connect to an existing host, but to a server that is not there
        let error_message = connect().await
            .expect_transient(&format!("There is no server currently listening at {interface}:{port}, but the result of a connection attempt was not a `Transient` error"))
            .into_result()
            .expect_err("A `Result::Err` should have been issued")
            .to_string();
        assert_eq!(error_message, "Couldn't connect to socket address '127.0.0.1:8357' resolved from '127.0.0.1:8357': Connection refused (os error 111)", "Wrong error message");

        // now with a server listening
        let mut server_connection_handler = ServerConnectionHandler::new(interface.to_string(), port).await?;
        let mut connection_receiver = server_connection_handler.connection_receiver().expect("The `receiver` should be available at this point");
        tokio::spawn(async move {
            while let Some(connection) = connection_receiver.recv().await {
                received_count_ref.fetch_add(1, Relaxed);
            }
            stream_ended_ref.store(true, Relaxed);
        });
        for i in 0..10 {
            connect().await
                .expect_ok(&format!("There is a server listening at {interface}:{port}, so the `connect()` closure should have worked"));
        }
        server_connection_handler.shutdown().await;
        assert_eq!(received_count.load(Relaxed), expected_count, "The wrong number of connections were received");
        Ok(())
    }

}