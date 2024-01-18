//! Resting place for [ServerProtocolProcessor]

use crate::composite_protocol_stacking_common::{
    logic::{ping_pong_logic::Umpire,
    ping_pong_models::{GameStates, Players, TurnFlipEvents, PingPongEvent, GameOverStates},
    protocol_processor::{react_to_hard_fault, react_to_rally_event, react_to_score, react_to_service_soft_fault}},
    protocol_model::{GameClientMessages, PROTOCOL_VERSION, GameServerMessages}
};
use std::{
    cell::UnsafeCell,
    fmt::Debug,
    sync::Arc,
};
use reactive_messaging::prelude::{ConnectionEvent, Peer};
use reactive_mutiny::prelude::FullDuplexUniChannel;
use dashmap::DashMap;
use futures::stream::{self, Stream, StreamExt};
use log::{debug, info, warn, error};
use crate::composite_protocol_stacking_common::protocol_model::{PreGameClientMessages, PreGameServerMessages, ProtocolStates};


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
impl Default for ServerProtocolProcessor {
    fn default() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }
}

impl ServerProtocolProcessor {

    pub fn new() -> Self {
        Self::default()
    }

    pub fn pre_game_connection_events_handler<const NETWORK_CONFIG: u64,
                                              SenderChannel:        FullDuplexUniChannel<ItemType=GameServerMessages, DerivedItemType=GameServerMessages> + Send + Sync>
                                             (&self,
                                              connection_event: ConnectionEvent<NETWORK_CONFIG, GameServerMessages, SenderChannel>) {
        match connection_event {
            ConnectionEvent::PeerConnected { peer } => {
                debug!("Connected: {:?}", peer);
                self.sessions.insert(peer.peer_id, Arc::new(Session { umpire: UnsafeCell::new(None) }));
            },
            ConnectionEvent::PeerDisconnected { peer, stream_stats } => {
                debug!("Disconnected: {:?} -- stats: {:?}", peer, stream_stats);
                self.sessions.remove(&peer.peer_id);
            },
            ConnectionEvent::LocalServiceTermination => {},
        }
    }

    pub fn pre_game_dialog_processor<const NETWORK_CONFIG: u64,
                                     SenderChannel:        FullDuplexUniChannel<ItemType=PreGameServerMessages, DerivedItemType=PreGameServerMessages> + Send + Sync,
                                     StreamItemType:       AsRef<PreGameClientMessages> + Debug>
                            
                                    (&self,
                                     _client_addr:           String,
                                     _port:                  u16,
                                     peer:                   Arc<Peer<NETWORK_CONFIG, PreGameServerMessages, SenderChannel, ProtocolStates>>,
                                     client_messages_stream: impl Stream<Item=StreamItemType>)

                                    -> impl Stream<Item=PreGameServerMessages> {
                            
        let session = self.sessions.get(&peer.peer_id)
                                                 .unwrap_or_else(|| panic!("Server BUG! Peer {:?} showed up, but we don't have a session for it! It should have been created by the `connection_events()` callback", peer))
                                                 .value()
                                                 .clone();     // .clone() the Arc, so we are free to move it to the the next closure (and drop it after the Stream closes)
        client_messages_stream.map(move |client_message| {
            // crate a umpire for the new game
            match client_message.as_ref() {
                PreGameClientMessages::Config(match_config) => {
                    // instantiate the game
                    let umpire_option = unsafe { &mut * (session.umpire.get()) };
                    let umpire = Umpire::new(&match_config, Players::Opponent);
                    umpire_option.replace(umpire);
                    peer.set_state_sync(ProtocolStates::Game);
                    PreGameServerMessages::Version(String::from(PROTOCOL_VERSION))
                },
                PreGameClientMessages::Error(err) => {
                    error!("Pre-game Client {:?} errored. Closing the connection after receiving: '{}'", *peer, err);
                    peer.set_state_sync(ProtocolStates::Disconnect);
                    peer.cancel_and_close();
                    PreGameServerMessages::NoAnswer
                },
                PreGameClientMessages::NoAnswer => PreGameServerMessages::NoAnswer,
            }
        })
    }

    pub fn game_connection_events_handler<const NETWORK_CONFIG: u64,
                                          SenderChannel:        FullDuplexUniChannel<ItemType=GameServerMessages, DerivedItemType=GameServerMessages> + Send + Sync>
                                         (&self,
                                          connection_event: ConnectionEvent<NETWORK_CONFIG, GameServerMessages, SenderChannel>) {
        match connection_event {
            ConnectionEvent::PeerConnected { peer } => {
                debug!("Game Started: {:?}", peer);
            },
            ConnectionEvent::PeerDisconnected { peer, stream_stats } => {
                debug!("Disconnected: {:?} -- stats: {:?}", peer, stream_stats);
                self.sessions.remove(&peer.peer_id);
            }
            ConnectionEvent::LocalServiceTermination => {
                info!("Ping-Pong server shutdown requested. Notifying all peers...");
            }
        }
    }

    pub fn game_dialog_processor<const NETWORK_CONFIG: u64,
                                 SenderChannel:        FullDuplexUniChannel<ItemType=GameServerMessages, DerivedItemType=GameServerMessages> + Send + Sync,
                                 StreamItemType:       AsRef<GameClientMessages> + Debug>

                                (&self,
                                 _client_addr:           String,
                                 _port:                  u16,
                                 peer:                   Arc<Peer<NETWORK_CONFIG, GameServerMessages, SenderChannel, ProtocolStates>>,
                                 client_messages_stream: impl Stream<Item=StreamItemType>)

                                 -> impl Stream<Item=GameServerMessages> {

        let session = self.sessions.get(&peer.peer_id)
                                                 .unwrap_or_else(|| panic!("Server BUG! Peer {:?} showed up, but we don't have a session for it! It should have been created by the `connection_events()` callback", peer))
                                                 .value()
                                                 .clone();     // .clone() the Arc, so we are free to move it to the the next closure (and drop it after the Stream closes)
        let umpire_option = unsafe { &mut * (session.umpire.get()) };
        let Some(umpire) = umpire_option else {
            panic!("BUG! There is no umpire in the session entry for peer {peer:?}");
        };
        client_messages_stream.map(move |client_message| {

            match client_message.as_ref() {

                GameClientMessages::PingPongEvent(reported_ping_pong_event) => {
                    match reported_ping_pong_event {
                        PingPongEvent::TurnFlip { player_action: opponent_action, resulting_event} => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => vec![
                                    GameServerMessages::PingPongEvent( react_to_rally_event(umpire,
                                                                                            "WaitingForService",
                                                                                            |rs| matches!(rs, GameStates::WaitingForService { .. }),
                                                                                            opponent_action,
                                                                                            /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulService } ) )
                                ],
                                TurnFlipEvents::SoftFaultService => vec![
                                    GameServerMessages::PingPongEvent( react_to_rally_event(umpire,
                                                                                            "WaitingForService` or `Rally",
                                                                                            |rs| matches!(rs, GameStates::WaitingForService { .. } | GameStates::Rally),
                                                                                            opponent_action,
                                                                                            /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SoftFaultService } ) )
                                ],
                                TurnFlipEvents::SuccessfulRebate => vec![
                                    GameServerMessages::PingPongEvent( react_to_rally_event(umpire,
                                                                                            "Rally",
                                                                                            |rs| matches!(rs, GameStates::Rally),
                                                                                            opponent_action,
                                                                                            /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulRebate } ) )
                                ],
                            }
                        }
                        PingPongEvent::HardFault { player_action: opponent_action, resulting_fault_event} => {
                            react_to_hard_fault(umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(GameServerMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::SoftFault { player_action: opponent_action, resulting_fault_event} => {
                            react_to_service_soft_fault(umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(GameServerMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::Score { point_winning_player, last_player_action, last_fault } => {
                            if *point_winning_player != Players::Opponent {
                                error!("TO-BE-REMOVED Unrepresentable state IN THE SERVER: It is not up to any client ({:?}) to tell the server that a score was made", *peer);
                                vec![GameServerMessages::GoodBye]
                            } else {
                                // our score: opponent's hard fault
                                react_to_score(umpire, last_player_action, last_fault).into_iter()
                                    .map(GameServerMessages::PingPongEvent)
                                    .collect()
                            }
                        },

                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_player_action: _, last_fault: _ } => {
                                    info!("Game ended: {} Server; {} Client #{} @ {}", final_score.opponent, final_score.oneself, peer.peer_id, peer.peer_address);
                                    vec![GameServerMessages::GoodBye]
                                },
                                GameOverStates::GameCancelled { partial_score: _, broken_rule_description: _ } => {
                                    vec![GameServerMessages::GoodBye]
                                },
                            }
                        },
                    }
                }
                GameClientMessages::ContestedScore(client_provided_match_score) => {
                    warn!("Client {:?} contested the match score. Ours: {:?}; Theirs: {:?}", peer, umpire.score(), client_provided_match_score);
                    vec![GameServerMessages::GoodBye]
                },

                GameClientMessages::EndorsedScore => {
                    vec![GameServerMessages::GoodBye]
                },

                GameClientMessages::DumpConfig => {
                    vec![GameServerMessages::MatchConfig(*umpire.config())]
                }

                GameClientMessages::Error(err) => {
                    error!("Client {:?} errored. Closing the connection after receiving: '{}'", *peer, err);
                    peer.set_state_sync(ProtocolStates::Disconnect);
                    vec![GameServerMessages::GoodBye]
                },

                GameClientMessages::Quit => {
                    peer.set_state_sync(ProtocolStates::Disconnect);
                    vec![GameServerMessages::GoodBye]
                },
            }
        })
        .flat_map(stream::iter)
    }

}
