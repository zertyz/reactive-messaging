//! Contains some functions and other goodies used across this module


use std::fmt::Debug;
use crate::types::ProtocolEvent;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use futures::future::BoxFuture;
use reactive_mutiny::types::FullDuplexUniChannel;
use log::{error, warn};
use tokio::sync::Mutex;
use crate::serde::ReactiveMessagingConfig;


/// Upgrades the user provided `connection_events_callback` into a callback able to keep track of connect/disconnection events
/// as well as local service termination events, so any waiters on the client processor may be notified
pub fn upgrade_to_connection_event_tracking<const CONFIG:                   u64,
                                            LocalMessages:                  ReactiveMessagingConfig<LocalMessages>                                      + Send + Sync + PartialEq + Debug + 'static,
                                            SenderChannel:                  FullDuplexUniChannel<ItemType=LocalMessages, DerivedItemType=LocalMessages> + Send + Sync                     + 'static,
                                            ConnectionEventsCallbackFuture: Future<Output=()>                                                           + Send,
                                            StateType:                                                                                                    Send + Sync + Clone     + Debug + 'static>

                                           (connected_state:                          &Arc<AtomicBool>,
                                            termination_is_complete_signaler:         tokio::sync::mpsc::Sender<()>,
                                            user_provided_connection_events_callback: impl Fn(ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) -> ConnectionEventsCallbackFuture + Send + Sync + 'static)

                                           -> impl Fn(ProtocolEvent<CONFIG, LocalMessages, SenderChannel, StateType>) -> BoxFuture<'static, ()> + Send + Sync + 'static {

    let connected_state = Arc::clone(connected_state);
    let termination_is_complete_signaler = Arc::new(Mutex::new(Some(termination_is_complete_signaler)));
    let user_provided_connection_events_callback = Arc::new(user_provided_connection_events_callback);
    move |connection_event | {
        let connected_state = Arc::clone(&connected_state);
        let termination_is_complete_signaler = Arc::clone(&termination_is_complete_signaler);
        let user_provided_connection_events_callback = Arc::clone(&user_provided_connection_events_callback);
        let report_service_termination = |cant_take_msg| async move {
            let Some(termination_is_complete_signaler) = termination_is_complete_signaler.lock().await.take()
                else {
                    warn!("{}", cant_take_msg);
                    return
                };
            if let Err(_sent_value) = termination_is_complete_signaler.send(()).await {
                error!("Socket Client BUG: couldn't send the 'Termination is Complete' signal to the local `one_shot` channel. A deadlock (waiting on a client that will never terminate) might occur. Please, investigate and fix!");
            }
        };
        Box::pin(async move {
            match connection_event {
                ProtocolEvent::PeerArrived { .. } => {
                    connected_state.store(true, Relaxed);
                    user_provided_connection_events_callback(connection_event).await;
                },
                ProtocolEvent::PeerLeft { .. } => {
                    connected_state.store(false, Relaxed);
                    user_provided_connection_events_callback(connection_event).await;
                    report_service_termination("Socket Client: The remote party ended the connection, but a previous termination process seems to have already taken place. May this message be considered for elimination? Ignoring the current termination request...").await;
                },
                ProtocolEvent::LocalServiceTermination => {
                    user_provided_connection_events_callback(connection_event).await;
                    report_service_termination("Socket Client: a local service termination was asked, but a previous termination process seems to have already taken place. This is suggestive of a bug in your shutdown logic. Ignoring the current termination request...").await;
                }
            }
        })
    }
}