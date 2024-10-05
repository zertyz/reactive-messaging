//! Resting place of [ClientProtocolProcessor]

use crate::composite_protocol_stacking_common::{
    protocol_model::{GameClientMessages, PROTOCOL_VERSION, GameServerMessages},
    logic::{
        ping_pong_logic::{act, Umpire},
        ping_pong_models::{GameOverStates, GameStates, MatchConfig, PingPongEvent, Players, TurnFlipEvents},
        protocol_processor::{react_to_hard_fault, react_to_rally_event, react_to_score, react_to_service_soft_fault},
    }
};
use reactive_messaging::prelude::{ProtocolEvent, Peer, ResponsiveStream};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Instant,
};
use std::fmt::Debug;
use reactive_mutiny::prelude::FullDuplexUniChannel;
use futures::{future, Stream, stream, StreamExt};
use log::{debug, info, error, warn};
use crate::composite_protocol_stacking_common::protocol_model::{PreGameClientError, PreGameClientMessages, PreGameServerMessages, ProtocolStates, PROTOCOL_VERSIONS};


const MATCH_CONFIG: MatchConfig = MatchConfig {
    score_limit:            15000,
    rally_timeout_millis:   1000,
    no_bounce_probability:  0.001,
    no_rebate_probability:  0.002,
    mishit_probability:     0.003,
    pre_bounce_probability: 0.004,
    net_touch_probability:  0.005,
    net_block_probability:  0.006,
    ball_out_probability:   0.007,
};

pub struct ClientProtocolProcessor {
    start_instant:     Instant,
    in_messages_count: AtomicU64,
}

impl ClientProtocolProcessor {

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            start_instant:     Instant::now(),
            in_messages_count: AtomicU64::new(0),
        })
    }

    pub fn pre_game_connection_events_handler<const NETWORK_CONFIG: u64,
                                              SenderChannel:        FullDuplexUniChannel<ItemType=PreGameClientMessages, DerivedItemType=PreGameClientMessages> + Send + Sync>
                                             (self: &Arc<Self>,
                                              connection_event: ProtocolEvent<NETWORK_CONFIG, PreGameClientMessages, SenderChannel, ProtocolStates>) {
        if let ProtocolEvent::PeerArrived { peer } = connection_event {
            debug!("Connected: {:?}", peer);
        }
    }

    pub fn pre_game_dialog_processor<const NETWORK_CONFIG: u64,
                                     SenderChannel:        FullDuplexUniChannel<ItemType=PreGameClientMessages, DerivedItemType=PreGameClientMessages> + Send + Sync,
                                     StreamItemType:       AsRef<PreGameServerMessages> + Debug>

                                    (self:                   &Arc<Self>,
                                     _server_addr:           String,
                                     _port:                  u16,
                                     peer:                   Arc<Peer<NETWORK_CONFIG, PreGameClientMessages, SenderChannel, ProtocolStates>>,
                                     server_messages_stream: impl Stream<Item=StreamItemType>)

                                    -> impl Stream<Item=bool> {
                            
        let cloned_self = Arc::clone(self);
        let peer_ref = Arc::clone(&peer);
        _ = peer.send(PreGameClientMessages::Config(MATCH_CONFIG));
        server_messages_stream
            .map(move |server_message| {
                cloned_self.in_messages_count.fetch_add(1, Relaxed);
                match server_message.as_ref() {
                    PreGameServerMessages::Version(server_protocol_version) => {
                        if server_protocol_version == &PROTOCOL_VERSION {
                            // Upgrade to the next protocol
                            _ = peer.try_set_state(ProtocolStates::Game);
                            None
                        } else {
                            warn!("Aborting Connection: Client protocol version is {:?} while server is {server_protocol_version:?}", PROTOCOL_VERSIONS.get_key_value(&PROTOCOL_VERSION));
                            _ = peer.try_set_state(ProtocolStates::Disconnect);
                            Some(PreGameClientMessages::Error(PreGameClientError::IncompatibleProtocols))
                        }
                    },
                    PreGameServerMessages::Error(err) => {
                        warn!("Server (pre game) answered with error {err:?} -- closing the connection");
                        _ = peer.try_set_state(ProtocolStates::Disconnect);
                        Some(PreGameClientMessages::Error(PreGameClientError::TextualProtocolProcessorParsingError))
                    },
                }
            })
            .take_while(|server_message| future::ready(server_message.is_some()))    // stop if the protocol was upgraded
            .map(|server_message| server_message.unwrap())
            .to_responsive_stream(peer_ref, |server_message, _peer| matches!(server_message, PreGameClientMessages::Error(..)) )
            .take_while(|stop| future::ready(!stop))    // stop if an error happened (after sending the error message)
    }

    pub fn game_connection_events_handler<const NETWORK_CONFIG: u64,
                                          SenderChannel:        FullDuplexUniChannel<ItemType=GameClientMessages, DerivedItemType=GameClientMessages> + Send + Sync>
                                         (self: &Arc<Self>,
                                          connection_event: ProtocolEvent<NETWORK_CONFIG, GameClientMessages, SenderChannel, ProtocolStates>) {
        match connection_event {
            ProtocolEvent::PeerArrived { peer: _ } => {},
            ProtocolEvent::PeerLeft { peer, stream_stats } => {
                let in_messages_count = self.in_messages_count.load(Relaxed);
                info!("CLIENT Disconnected: {:?}; stats: {:?} -- with {} messages IN & OUT: {:.2}/s",
                      peer,
                      stream_stats,
                      in_messages_count, in_messages_count as f64 / self.start_instant.elapsed().as_secs_f64());
            }
            ProtocolEvent::LocalServiceTermination => {
                info!("Ping-Pong client shutdown requested. Notifying the server...");
            }
        }
    }

    pub fn game_dialog_processor<const NETWORK_CONFIG: u64,
                                 SenderChannel:        FullDuplexUniChannel<ItemType=GameClientMessages, DerivedItemType=GameClientMessages> + Send + Sync,
                                 StreamItemType:       AsRef<GameServerMessages>>

                                (self:                   &Arc<Self>,
                                 server_addr:            String,
                                 port:                   u16,
                                 peer:                   Arc<Peer<NETWORK_CONFIG, GameClientMessages, SenderChannel, ProtocolStates>>,
                                 server_messages_stream: impl Stream<Item=StreamItemType>)

                                -> impl Stream<Item=bool> {

        _ = peer.try_set_state(ProtocolStates::Disconnect);     // the next state -- after this stream ends -- is "disconnect".
                                                                // TODO 2024-01-27: this may be moved to the connection event handler after the new state is added
        let cloned_self = Arc::clone(self);
        let peer_ref = Arc::clone(&peer);
        let mut umpire = Umpire::new(&MATCH_CONFIG, Players::Ourself);
        server_messages_stream.map(move |server_message| {
            cloned_self.in_messages_count.fetch_add(1, Relaxed);
            match server_message.as_ref() {

                GameServerMessages::GameStarted => {
                    // Start the game: service the ball
                    let our_action = act();
                    let our_event = umpire.process_turn(Players::Ourself, &our_action);
                    vec![GameClientMessages::PingPongEvent(our_event)]
                },

                GameServerMessages::MatchConfig(match_config) => {
                    info!("Server told us it is using {:?}", match_config);
                    vec![/* no answer */]
                }

                GameServerMessages::PingPongEvent(reported_ping_pong_event) => {
                    match reported_ping_pong_event {
                        PingPongEvent::TurnFlip { player_action: opponent_action, resulting_event } => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => vec![
                                    GameClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                           "WaitingForService",
                                                                                           |rs| matches!(rs, GameStates::WaitingForService { attempt: _ }),
                                                                                           opponent_action,
                                                                                           /*reported_ping_pong_event*/PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulService }))
                                ],
                                TurnFlipEvents::SoftFaultService => vec![
                                    GameClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                           "WaitingForService` or `Rally",
                                                                                           |rs| matches!(rs, GameStates::WaitingForService { attempt: _ }),
                                                                                           opponent_action,
                                                                                           /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SoftFaultService }))
                                ],
                                TurnFlipEvents::SuccessfulRebate => vec![
                                    GameClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                           "Rally",
                                                                                           |rs| matches!(rs, GameStates::Rally),
                                                                                           opponent_action,
                                                                                           /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulRebate }))
                                ],
                            }
                        },
                        PingPongEvent::HardFault { player_action: opponent_action, resulting_fault_event } => {
                            react_to_hard_fault(&mut umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(GameClientMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::SoftFault { player_action: opponent_action, resulting_fault_event } => {
                            react_to_service_soft_fault(&mut umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(GameClientMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::Score { point_winning_player, last_player_action, last_fault } => {
                            if *point_winning_player != Players::Opponent {
                                error!("TO-BE-REMOVED Unrepresentable state IN THE CLIENT: It is not up to any client ({:?}) to tell the server that a score was made", *peer);
                                vec![GameClientMessages::Quit]
                            } else {
                                // our score: opponent's hard fault
                                react_to_score(&mut umpire, last_player_action, last_fault).into_iter()
                                    .map(GameClientMessages::PingPongEvent)
                                    .collect()
                            }
                        },
                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_player_action: _, last_fault: _ } => {
                                    info!("Game ended: {} Client; {} Server @ {}:{}", final_score.opponent, final_score.oneself, server_addr, port);
                                    vec![GameClientMessages::EndorsedScore]
                                }
                                GameOverStates::GameCancelled { partial_score: _, broken_rule: _ } => {
                                    vec![/* no answer */]
                                }
                            }
                        },
                    }
                },

                GameServerMessages::Error(err) => {
                    error!("Server answered with error {err:?}");
                    vec![GameClientMessages::Quit]
                },

                GameServerMessages::GoodBye | GameServerMessages::ServerShutdown => {
                    peer.cancel_and_close();
                    vec![/* no answer */]
                },
            }
        })
        .flat_map(stream::iter)
        .to_responsive_stream(peer_ref, |client_message, _peer| matches!(client_message, GameClientMessages::Quit | GameClientMessages::Error(..)))
        .take_while(|stop| future::ready(!stop))
    }
}

