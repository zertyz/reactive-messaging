//! Doc it.


use std::sync::Arc;
use crate::prelude::Peer;

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