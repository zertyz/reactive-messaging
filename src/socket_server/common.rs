//! Contains some functions and other goodies used across this module


use crate::prelude::ConnectionEvent;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use futures::future::BoxFuture;
use reactive_mutiny::prelude::FullDuplexUniChannel;
use tokio::sync::Mutex;
use log::{warn, error};

/// Upgrades the user provided `connection_events_callback()` into a callback able to keep track of the shutdown event
/// -- so the "shutdown is complete" signal may be sent
pub(crate) fn upgrade_to_shutdown_tracking<SenderChannelType:              FullDuplexUniChannel + Sync + Send + 'static,
                                           ConnectionEventsCallbackFuture: Future<Output=()> + Send>

                                          (shutdown_is_complete_signaler:            tokio::sync::oneshot::Sender<()>,
                                           user_provided_connection_events_callback: impl Fn(ConnectionEvent<SenderChannelType>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)

                                          -> impl Fn(ConnectionEvent<SenderChannelType>) -> BoxFuture<'static, ()> + Send + Sync + 'static {

    let shutdown_is_complete_signaler = Arc::new(Mutex::new(Option::Some(shutdown_is_complete_signaler)));
    let user_provided_connection_events_callback = Arc::new(user_provided_connection_events_callback);
    move |connection_event | {
        let shutdown_is_complete_signaler = Arc::clone(&shutdown_is_complete_signaler);
        let user_provided_connection_events_callback = Arc::clone(&user_provided_connection_events_callback);
        Box::pin(async move {
            if let ConnectionEvent::ApplicationShutdown { timeout_ms } = connection_event {
                let _ = tokio::time::timeout(Duration::from_millis(timeout_ms as u64), user_provided_connection_events_callback(connection_event)).await;
                let Some(shutdown_is_complete_signaler) = shutdown_is_complete_signaler.lock().await.take()
                else {
                    warn!("Socket Server: a shutdown was asked, but a previous shutdown seems to have already taken place. There is a bug in your shutdown logic. Ignoring the current shutdown request...");
                    return
                };
                if let Err(_sent_value) = shutdown_is_complete_signaler.send(()) {
                    error!("Socket Server BUG: couldn't send shutdown signal to the local `one_shot` channel. Program is, likely, hanged. Please, investigate and fix!");
                }
            } else {
                user_provided_connection_events_callback(connection_event).await;
            }
        })
    }
}

