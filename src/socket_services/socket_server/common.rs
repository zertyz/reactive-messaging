//! Contains some functions and other goodies used across this module


use crate::prelude::{
    ReactiveMessagingSerializer,
    ProtocolEvent,
};
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
};
use reactive_mutiny::prelude::FullDuplexUniChannel;
use tokio::sync::Mutex;
use log::{warn, error};
use crate::serde::ReactiveMessagingConfig;

/// Upgrades the user provided `connection_events_callback()` into a callback able to keep track of the shutdown event
/// -- so the "shutdown is complete" signal may be sent
#[inline(always)]
pub(crate) fn upgrade_to_termination_tracking<const CONFIG:                   u64,
                                           LocalMessages:                  ReactiveMessagingConfig<LocalMessages>                                      + Send + Sync + PartialEq + Debug + 'static,
                                           SenderChannel:                  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
                                           ConnectionEventsCallbackFuture: Future<Output=()>                                                           + Send,
                                           StateType:                                                                                                    Send + Sync + Clone     + Debug + 'static>

                                          (shutdown_is_complete_signaler:            tokio::sync::oneshot::Sender<()>,
                                           user_provided_connection_events_callback: impl Fn(ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)

                                          -> impl Fn(ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) -> Pin<Box<dyn Future<Output=()> + Send>> {

    let shutdown_is_complete_signaler = Arc::new(Mutex::new(Some(shutdown_is_complete_signaler)));
    let user_provided_connection_events_callback = Arc::new(user_provided_connection_events_callback);
    move |connection_event | {
        let shutdown_is_complete_signaler = Arc::clone(&shutdown_is_complete_signaler);
        let user_provided_connection_events_callback = Arc::clone(&user_provided_connection_events_callback);
        Box::pin(async move {
            if let ProtocolEvent::LocalServiceTermination { } = connection_event {
                let Some(shutdown_is_complete_signaler) = shutdown_is_complete_signaler.lock().await.take()
                else {
                    warn!("Socket Server: a shutdown was asked, but a previous shutdown seems to have already taken place. There is a bug in your shutdown logic. Ignoring the current shutdown request...");
                    return
                };
                user_provided_connection_events_callback(connection_event).await;
                if let Err(_sent_value) = shutdown_is_complete_signaler.send(()) {
                    error!("Socket Server BUG: couldn't send shutdown-is-complete signal to the local `one_shot` channel. Program is, likely, hanged. Please, investigate and fix!");
                }
            } else {
                user_provided_connection_events_callback(connection_event).await;
            }
        })
    }
}

