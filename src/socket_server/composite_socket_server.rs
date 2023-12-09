//! Doc it.


use std::sync::Arc;
use crate::prelude::Peer;

// TODO: from code taken after the latest refactorings -- THE NEXT STEPS ARE FOR THE COMPOSITE SOCKET SERVER ONLY:
//       allow the current server to add connections to the handler
//       (other dialog processor may require a reference to this server so to add connections to them)

/*
pub type A<const CONFIG: u64,
           _Self,
          > =
    Box<dyn Fn(/*client_addr: */String, /*connected_port: */u16, /*handle: */Arc<_Self>, /*client_messages_stream: */MessagingMutinyStream<ProcessorUniType>) -> ServerStreamType               + Send + Sync               + 'static>>;

pub struct GenericCompositeSocketServer<const CONFIG: u64> {
    interface_ip: String,
    port: u16,
    server_shutdown_signaler: Option<tokio::sync::oneshot::Sender<u32>>,
    local_shutdown_receiver: Option<tokio::sync::oneshot::Receiver<()>>
}

impl<const CONFIG: u64> GenericCompositeSocketServer<CONFIG> {

}
*/