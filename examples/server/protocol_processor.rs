//! Resting place for [ServerProtocolProcessor]

use std::cell::UnsafeCell;
use std::sync::Arc;
use reactive_messaging::{
    prelude::{
        Peer,
        ProcessorRemoteStreamType
    },
};
use crate::common::{
    logic::{ping_pong_logic::Umpire,
    ping_pong_models::{GameStates, Players, TurnFlipEvents, PingPongEvent, GameOverStates},
    protocol_processor::{react_to_hard_fault, react_to_rally_event, react_to_score, react_to_service_soft_fault}},
    protocol_model::{ClientMessages, PROTOCOL_VERSION, ServerMessages}
};
use dashmap::DashMap;
use futures::stream::{self, Stream, StreamExt};
use reactive_messaging::prelude::ConnectionEvent;
use log::{debug, info, warn, error};


/// Session for each connected peer
struct Session {
    umpire: UnsafeCell<Option<Umpire>>,     // Mutex is not needed here, as modifications will be done in a non-parallel / non-concurrent fashion
}
// This tells Rust that it is safe to move `Session` to different threads without the need of putting it into any synchronizations wrappers -- allowing it to be used in `UnsafeCell<Umpire>`
unsafe impl Send for Session {}
unsafe impl Sync for Session {}

pub struct ServerProtocolProcessor {
    sessions: Arc<DashMap<u32, Arc<Session>>>,
}

impl ServerProtocolProcessor {

    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }

    pub fn server_events_callback(&self, connection_event: ConnectionEvent<ServerMessages>) {
        match connection_event {
            ConnectionEvent::PeerConnected { peer } => {
                debug!("Connected: {:?}", peer);
                self.sessions.insert(peer.peer_id, Arc::new(Session { umpire: UnsafeCell::new(None) }));
            },
            ConnectionEvent::PeerDisconnected { peer, stream_stats } => {
                debug!("Disconnected: {:?} -- stats: {:?}", peer, stream_stats);
                //let _ = processor_uni.try_send(|slot| *slot = ClientMessages::Quit);
                self.sessions.remove(&peer.peer_id);
            }
            ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                info!("Ping-Pong server shutdown requested. Notifying all peers within {timeout_ms}ms...");
            }
        }
    }

    pub fn dialog_processor(&self, _client_addr: String, _port: u16, peer: Arc<Peer<ServerMessages>>, client_messages_stream: ProcessorRemoteStreamType<ClientMessages>) -> impl Stream<Item=ServerMessages> {
        let session = self.sessions.get(&peer.peer_id)
                                                 .unwrap_or_else(|| panic!("Server BUG! Peer {:?} showed up, but we don't have a session for it! It should have been created by the `connection_events()` callback", peer))
                                                 .value()
                                                 .clone();     // .clone() the Arc, so we are free to move it to the the next closure (and drop it after the Stream closes)
        client_messages_stream.map(move |client_message| {

            // get the game's umpire instance or expect the first client message to be the one to create the match umpire
            let umpire_option = unsafe { &mut * (session.umpire.get()) };
            let umpire = match umpire_option {
                Some(umpire) => umpire,
                None => return {
                    if let ClientMessages::Config(match_config) = &*client_message {
                        // instantiate the game
                        let umpire = Umpire::new(match_config, Players::Opponent);
                        umpire_option.replace(umpire);
                        vec![ServerMessages::GameStarted]
                    } else {
                        vec![ServerMessages::Error(format!("The first message sent must be `Config(match_config)` -- the received one was `{:?}`", client_message))]
                    }
                }
            };

            // from this point on, we have a configured umpire in the `umpire` variable

            match &*client_message {

                ClientMessages::Config(offending_match_config) => {
                    // protocol offense
                    vec![ServerMessages::Error(format!("Protocol Offense: Was `Config` sent twice? You just sent `Config({:?}) , but we have `{:?}` already associated with you (due to a previous call to `Config()`)", offending_match_config, umpire.config()))]
                },

                ClientMessages::PingPongEvent(reported_ping_pong_event) => {
                    match reported_ping_pong_event {
                        PingPongEvent::TurnFlip { player_action: opponent_action, resulting_event} => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => vec![
                                    ServerMessages::PingPongEvent( react_to_rally_event(umpire,
                                                                                        "WaitingForService",
                                                                                        |rs| matches!(rs, GameStates::WaitingForService { .. }),
                                                                                        opponent_action,
                                                                                        /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulService } ) )
                                ],
                                TurnFlipEvents::SoftFaultService => vec![
                                    ServerMessages::PingPongEvent( react_to_rally_event(umpire,
                                                                                        "WaitingForService` or `Rally",
                                                                                        |rs| matches!(rs, GameStates::WaitingForService { .. } | GameStates::Rally),
                                                                                        opponent_action,
                                                                                        /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SoftFaultService } ) )
                                ],
                                TurnFlipEvents::SuccessfulRebate => vec![
                                    ServerMessages::PingPongEvent( react_to_rally_event(umpire,
                                                                                        "Rally",
                                                                                        |rs| matches!(rs, GameStates::Rally),
                                                                                        opponent_action,
                                                                                        /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulRebate } ) )
                                ],
                            }
                        }
                        PingPongEvent::HardFault { player_action: opponent_action, resulting_fault_event} => {
                            react_to_hard_fault(umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(ServerMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::SoftFault { player_action: opponent_action, resulting_fault_event} => {
                            react_to_service_soft_fault(umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(ServerMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::Score { point_winning_player, last_player_action, last_fault } => {
                            if *point_winning_player != Players::Opponent {
                                error!("TO-BE-REMOVED Unrepresentable state IN THE SERVER: It is not up to any client ({:?}) to tell the server that a score was made", *peer);
                                vec![ServerMessages::GoodBye]
                            } else {
                                // our score: opponent's hard fault
                                react_to_score(umpire, last_player_action, last_fault).into_iter()
                                    .map(ServerMessages::PingPongEvent)
                                    .collect()
                            }
                        },

                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_player_action: _, last_fault: _ } => {
                                    info!("Game ended: {} Server; {} Client #{} @ {}", final_score.opponent, final_score.oneself, peer.peer_id, peer.peer_address);
                                    vec![ServerMessages::GoodBye]
                                },
                                GameOverStates::GameCancelled { partial_score: _, broken_rule_description: _ } => {
                                    vec![ServerMessages::GoodBye]
                                },
                            }
                        },
                    }
                }
                ClientMessages::ContestedScore(client_provided_match_score) => {
                    warn!("Client {:?} contested the match score. Ours: {:?}; Theirs: {:?}", peer, umpire.score(), client_provided_match_score);
                    vec![ServerMessages::GoodBye]
                },

                ClientMessages::EndorsedScore => {
                    vec![ServerMessages::GoodBye]
                },

                ClientMessages::Error(err) => {
                    error!("Client {:?} errored. Closing the connection after receiving: '{}'", *peer, err);
                    vec![ServerMessages::GoodBye]
                },

                ClientMessages::NoAnswer => {
                    panic!("BUG: received a `NoAnswer` message")
                },

                ClientMessages::Quit => {
                    vec![ServerMessages::GoodBye]
                },

                ClientMessages::Version => {
                    vec![ServerMessages::Version(PROTOCOL_VERSION.to_string())]
                },
            }
        })
        .flat_map(stream::iter)
    }

}
