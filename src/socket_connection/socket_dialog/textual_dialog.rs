use std::error::Error;
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::RangeInclusive;
use std::sync::Arc;
use futures::StreamExt;
use reactive_mutiny::prelude::{FullDuplexUniChannel, GenericUni};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::prelude::{Peer, SocketConnection};
use crate::serde::{ReactiveMessagingDeserializer, ReactiveMessagingSerializer};
use crate::socket_connection::common::ReactiveMessagingUniSender;
use crate::socket_connection::socket_dialog::dialog_types::SocketDialog;
use log::{debug, error, trace, warn};
use tokio::io;

pub struct TextualDialog<const CONFIG:       u64,
                         RemoteMessagesType: ReactiveMessagingDeserializer<RemoteMessagesType> + Send + Sync + PartialEq + Debug + 'static,
                         LocalMessagesType:  ReactiveMessagingSerializer<LocalMessagesType>    + Send + Sync + PartialEq + Debug + 'static,
                        > {
    _phantom_data: PhantomData<(RemoteMessagesType, LocalMessagesType)>,
}

impl<const CONFIG: u64,
     RemoteMessagesType: ReactiveMessagingDeserializer<RemoteMessagesType> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingSerializer<LocalMessagesType>    + Send + Sync + PartialEq + Debug + 'static,
    >
Default
for TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType> {
    fn default() -> Self {
        Self {
            _phantom_data: PhantomData,
        }
    }
}

impl<const CONFIG: u64,
     RemoteMessagesType: ReactiveMessagingDeserializer<RemoteMessagesType> + Send + Sync + PartialEq + Debug + 'static,
     LocalMessagesType:  ReactiveMessagingSerializer<LocalMessagesType>    + Send + Sync + PartialEq + Debug + 'static,
    >
SocketDialog<CONFIG>
for TextualDialog<CONFIG, RemoteMessagesType, LocalMessagesType> {
    type RemoteMessagesConstrainedType = RemoteMessagesType;
    type LocalMessagesConstrainedType = LocalMessagesType;

    /// Dialog loop specialist for text-based message forms, where each in & out event/command/sentence ends in '\n'.\
    /// `max_line_size` is the limit length of the lines that can be parsed (including the '\n' delimiter): if bigger
    /// lines come in, the dialog will end in error.
    #[inline(always)]
    async fn dialog_loop<ProcessorUniType:   GenericUni<ItemType=Self::RemoteMessagesConstrainedType>                                                              + Send + Sync                     + 'static,
                         SenderChannel:      FullDuplexUniChannel<ItemType=Self::LocalMessagesConstrainedType, DerivedItemType=Self::LocalMessagesConstrainedType> + Send + Sync                     + 'static,
                         StateType:                                                                                                                                  Send + Sync + Clone + Debug     + 'static,
                        >
                        (self,
                         socket_connection:     &mut SocketConnection<StateType>,
                         peer:                  &Arc<Peer<CONFIG, Self::LocalMessagesConstrainedType, SenderChannel, StateType>>,
                         processor_sender:      &ReactiveMessagingUniSender<CONFIG, Self::RemoteMessagesConstrainedType, ProcessorUniType::DerivedItemType, ProcessorUniType>,
                         payload_size_range:    RangeInclusive<u32>)

                        -> Result<(), Box<dyn std::error::Error + Sync + Send>> {

        // sanity checks on payload sizes
        let max_line_size = *payload_size_range.end();
        if max_line_size < 4 {
            return Err(Box::from(format!("Textual DIalog Loop: the given `max_line_size` for the payload is too small (only {max_line_size} bytes) and this is probably a BUG in your program")))
        }

        let mut read_buffer = Vec::with_capacity(max_line_size as usize);
        let mut serialization_buffer = Vec::with_capacity(max_line_size as usize);

        let (mut sender_stream, _) = peer.create_stream();

        'connection: loop {
            // wait for the socket to be readable or until we have something to write
            tokio::select!(

                biased;     // sending has priority over receiving

                // send?
                to_send = sender_stream.next() => {
                    match to_send {
                        Some(to_send) => {
                            // serialize
                            LocalMessagesType::serialize_textual(&to_send, &mut serialization_buffer);
                            serialization_buffer.push(b'\n');
                            // send
                            if let Err(err) = socket_connection.connection_mut().write_all(&serialization_buffer).await {
                                warn!("`dialog_loop_for_textual_form()`: PROBLEM in the connection with {peer:#?} while WRITING '{to_send:?}': {err:?}");
                                socket_connection.report_closed();
                                break 'connection
                            }
                        },
                        None => {
                            debug!("`dialog_loop_for_textual_form()`: Sender for {peer:#?} ended (most likely, either `peer.flush_and_close()` or `peer.cancel_and_close()` was called on the `peer`)");
                            break 'connection
                        }
                    }
                },

                // receive?
                read = socket_connection.connection_mut().read_buf(&mut read_buffer) => {
                    match read {
                        Ok(n) if n > 0 => {
                            // deserialize
                            let mut next_line_index = 0;
                            let mut this_line_search_start = read_buffer.len() - n;
                            loop {
                                if let Some(mut eol_pos) = read_buffer[next_line_index+this_line_search_start..].iter().position(|&b| b == b'\n') {
                                    eol_pos += next_line_index+this_line_search_start;
                                    let line_bytes = &read_buffer[next_line_index..eol_pos];
                                    match RemoteMessagesType::deserialize_textual(line_bytes) {
                                        Ok(remote_message) => {
                                            if let Err((abort_processor, error_msg_processor)) = processor_sender.send(remote_message).await {
                                                // log & send the error message to the remote peer
                                                error!("`dialog_loop_for_textual_form()`: {} -- `dialog_processor` is full of unprocessed messages ({}/{})", error_msg_processor, processor_sender.pending_items_count(), processor_sender.buffer_size());
                                                if let Err((abort_sender, error_msg_sender)) = peer.send_async(LocalMessagesType::processor_error_message(error_msg_processor)).await {
                                                        warn!("dialog_loop_for_textual_form(): {error_msg_sender} -- Slow reader {:?}", peer);
                                                    if abort_sender {
                                                        socket_connection.report_closed();
                                                        break 'connection
                                                    }
                                                }
                                                if abort_processor {
                                                    socket_connection.report_closed();
                                                    break 'connection
                                                }
                                            }
                                        },
                                        Err(err) => {
                                            let stripped_line = String::from_utf8_lossy(line_bytes);
                                            let error_message = format!("Unknown command received from {:?} (peer id {}): '{}': {}",
                                                                            peer.peer_address, peer.peer_id, stripped_line, err);
                                            // log & send the error message to the remote peer
                                            warn!("`dialog_loop_for_textual_form()`:  {error_message}");
                                            let outgoing_error = LocalMessagesType::processor_error_message(error_message);
                                            if let Err((abort, error_msg)) = peer.send_async(outgoing_error).await {
                                                if abort {
                                                    warn!("`dialog_loop_for_textual_form()`:  {error_msg} -- Slow reader {:?}", peer);
                                                    socket_connection.report_closed();
                                                    break 'connection
                                                }
                                            }
                                        }
                                    }
                                    next_line_index = eol_pos + 1;
                                    this_line_search_start = 0;
                                    if next_line_index >= read_buffer.len() {
                                        read_buffer.clear();
                                        break
                                    }
                                } else {
                                    if next_line_index > 0 {
                                        read_buffer.drain(0..next_line_index);
                                    }
                                    // TODO: can we break the server (or client) if the message is too big / don't have a '\n'? TEST IT!
                                    break
                                }
                            }
                        },
                        Ok(_) /* zero bytes received -- the other end probably closed the connection */ => {
                            trace!("`dialog_loop_for_textual_form()`: EOF while reading from {:?} (peer id {}) -- it is out of bytes! Dropping the connection", peer.peer_address, peer.peer_id);
                            socket_connection.report_closed();
                            break 'connection
                        },
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {},
                        Err(err) => {
                            error!("`dialog_loop_for_textual_form()`: ERROR in the connection with {:?} (peer id {}) while READING: '{:?}' -- dropping it", peer.peer_address, peer.peer_id, err);
                            socket_connection.report_closed();
                            break 'connection
                        },
                    }
                },
            );
        }
        Ok(())
    }
}