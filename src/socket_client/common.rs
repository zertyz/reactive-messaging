//! Contains some functions and other goodies used across this module


use std::fmt::Debug;
use crate::types::ConnectionEvent;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use futures::future::BoxFuture;
use reactive_mutiny::types::FullDuplexUniChannel;
use log::{error, warn};
use tokio::sync::Mutex;
use crate::ReactiveMessagingSerializer;
use crate::socket_connection::common::{ReactiveMessagingSender, RetryableSender};

/// Upgrades the user provided `connection_events_callback` into a callback able to keep track of disconnection events
/// as well as shutdown events, so any waiters on the client shutdown event may be notified
pub fn upgrade_to_shutdown_and_connected_state_tracking<const CONFIG:                   u64,
                                                        LocalMessages:                  ReactiveMessagingSerializer<LocalMessages>                                  + Send + Sync + PartialEq + Debug + 'static,
                                                        SenderChannel:                  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
                                                        ConnectionEventsCallbackFuture: Future<Output=()> + Send>

                                                       (connected_state:                          &Arc<AtomicBool>,
                                                        shutdown_is_complete_signaler:            tokio::sync::oneshot::Sender<()>,
                                                        user_provided_connection_events_callback: impl Fn(ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)

                                                       -> impl Fn(ConnectionEvent<CONFIG, LocalMessages, SenderChannel>) -> BoxFuture<'static, ()> + Send + Sync + 'static {

    let connected_state = Arc::clone(connected_state);
    let shutdown_is_complete_signaler = Arc::new(Mutex::new(Option::Some(shutdown_is_complete_signaler)));
    let user_provided_connection_events_callback = Arc::new(user_provided_connection_events_callback);
    move |connection_event | {
        let connected_state = Arc::clone(&connected_state);
        let shutdown_is_complete_signaler = Arc::clone(&shutdown_is_complete_signaler);
        let user_provided_connection_events_callback = Arc::clone(&user_provided_connection_events_callback);
        Box::pin(async move {
            match connection_event {
                ConnectionEvent::PeerConnected { .. } => {
                    connected_state.store(true, Relaxed);
                    user_provided_connection_events_callback(connection_event).await;
                },
                ConnectionEvent::PeerDisconnected { .. } => {
                    connected_state.store(false, Relaxed);
                    user_provided_connection_events_callback(connection_event).await;
                },
                ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                    let _ = tokio::time::timeout(Duration::from_millis(timeout_ms as u64), user_provided_connection_events_callback(connection_event)).await;
                    let Some(shutdown_is_complete_signaler) = shutdown_is_complete_signaler.lock().await.take()
                    else {
                        warn!("Socket Client: a shutdown was asked, but a previous shutdown seems to have already taken place. There is a bug in your shutdown logic. Ignoring the current shutdown request...");
                        return
                    };
                    if let Err(_sent_value) = shutdown_is_complete_signaler.send(()) {
                        error!("Socket Client BUG: couldn't send shutdown signal to the local `one_shot` channel. Program is, likely, hanged. Please, investigate and fix!");
                    }
                },
            }
        })
    }
}