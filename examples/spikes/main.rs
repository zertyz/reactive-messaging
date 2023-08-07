//! Here you'll only find the Spikes used to help in the top-down design of the server APIs during early development phases.
//! Search elsewhere if you want real examples of how to use the resulting lib.
//! All the structs & traits conteined here are just representative simplifications of the real counter-parts, to help focusing
//! on the design.

mod uni;
mod config;
mod socket_server;

use std::{marker::PhantomData, borrow::Borrow};

use uni::Uni;
use uni::channel::ChannelZeroCopy;
use config::ConstConfig;
use socket_server::SocketServer;

use crate::{socket_server::{GenericSocketServer, AtomicSocketServer, FullSyncSocketServer}, uni::channel::{GenericChannel, ChannelMove}};

type LocalMessages  = f64;
type RemoteMessages = u32;

fn main() {
    println!("Are you ready for the reactive-messaging client/server API Spikes?");
    modality_1();
    modality_2_and_3();
    modality_4();
 }

/// specifying the types to their minimum in the closures
fn modality_1() {
    const CONFIG:                    usize = ConstConfig::into(ConstConfig::Atomic(100));
    const PROCESSOR_UNI_INSTRUMENTS: usize = 0;
    const PROCESSOR_BUFFER_SIZE:     usize = 1024;
    const SENDER_BUFFER_SIZE:        usize = 1024;
    type ProcessorChannelType = ChannelZeroCopy<PROCESSOR_BUFFER_SIZE, RemoteMessages>;
    type ProcessorUniType     = Uni::<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE, RemoteMessages, ProcessorChannelType>;
    type SenderChannelType    = ChannelMove<SENDER_BUFFER_SIZE, LocalMessages>;

    let _server = SocketServer::<CONFIG,
                                                                              PROCESSOR_UNI_INSTRUMENTS,
                                                                              PROCESSOR_BUFFER_SIZE,
                                                                              RemoteMessages,
                                                                              LocalMessages,
                                                                              ProcessorUniType,
                                                                              SenderChannelType> { _phantom: PhantomData }
        .start(
            |_event| {},
            |client_messages_stream| client_messages_stream.map(|_payload| 0.0)
        );
}

/// With fully qualifyed types for external functions.\
/// Modality 2: using impl Iterator\
/// Modality 3: omitted, would be using the concrete alternative '_StreamType', avoiding the 'impl'
fn modality_2_and_3() {
    const CONFIG:                    usize = ConstConfig::into(ConstConfig::Atomic(100));
    const PROCESSOR_UNI_INSTRUMENTS: usize = 0;
    const PROCESSOR_BUFFER_SIZE:     usize = 1024;
    const SENDER_BUFFER_SIZE:        usize = 1024;
    type ProcessorChannelType = ChannelZeroCopy<PROCESSOR_BUFFER_SIZE, RemoteMessages>;
    type ProcessorUniType     = Uni::<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE, RemoteMessages, ProcessorChannelType>;
    type SenderChannelType    = ChannelMove<SENDER_BUFFER_SIZE, LocalMessages>;
    type ServerType           = SocketServer<CONFIG, PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE, RemoteMessages, LocalMessages, ProcessorUniType, SenderChannelType>;
    type ConnectionEventType  = <ServerType as GenericSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::ConnectionEventType;
    type _StreamType          = <ServerType as GenericSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::StreamType;
    type StreamItemType       = <ServerType as GenericSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::StreamItemType;

    let _server = ServerType { _phantom: PhantomData }
        .start(connection_events_handler, processor);

    fn connection_events_handler(_event: ConnectionEventType) {}
    fn processor(client_messages_stream: /*_StreamType*/impl Iterator<Item=StreamItemType/*derived?*/>) -> impl Iterator<Item=LocalMessages> {
        client_messages_stream.map(|payload| *payload as f64)
    }
}

/// Selectively starting a server for each channel/uni, while preserving the same processor functions
fn modality_4 () {
    const CONFIG:                    usize = ConstConfig::into(ConstConfig::Atomic(100));
    const PROCESSOR_UNI_INSTRUMENTS: usize = 0;    
    const PROCESSOR_BUFFER_SIZE:     usize = 1024;
    const SENDER_BUFFER_SIZE:        usize = 1024;
    type AtomicServer           = AtomicSocketServer<CONFIG, PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE, SENDER_BUFFER_SIZE, RemoteMessages, LocalMessages>;
    type AtomicStreamItemType   = <AtomicServer as GenericSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::StreamItemType;
    type FullSyncServer         = FullSyncSocketServer<CONFIG, PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE, SENDER_BUFFER_SIZE, RemoteMessages, LocalMessages>;
    type FullSyncStreamItemType = <FullSyncServer as GenericSocketServer<PROCESSOR_UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::StreamItemType;

    // if atomic
    let _atomic_server = AtomicServer {_phantom: PhantomData}
        .start(connection_events_handler, processor::<AtomicStreamItemType> /* Arc<RemoteMessages> */);

    // if full sync
    let _full_sync_server = FullSyncServer {_phantom: PhantomData}
        .start(connection_events_handler, processor::<FullSyncStreamItemType> /* RemoteMessages */);

    fn connection_events_handler<ConnectionEventType: GenericChannel<PROCESSOR_BUFFER_SIZE>>(_event: ConnectionEventType) {}
    fn processor<StreamItemType: Borrow<RemoteMessages>>(client_messages_stream: impl Iterator<Item=StreamItemType>) -> impl Iterator<Item=LocalMessages> {
        client_messages_stream.map(|payload| *payload.borrow() as f64)
    }
}