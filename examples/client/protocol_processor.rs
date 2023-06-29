//! Resting place of [ClientProtocolProcessor]

use std::cell::UnsafeCell;
use std::sync::Arc;
use futures::{Stream, StreamExt};
use reactive_messaging::prelude::{ConnectionEvent, Peer, ProcessorRemoteStreamType, SocketProcessorDerivedType};
use crate::common::logic::ping_pong_logic::Umpire;
use crate::common::logic::ping_pong_models::{GameOverStates, MatchConfig, PingPongEvent, PlayerAction, TurnFlipEvents};
use crate::common::protocol_model::{ClientMessages, PROTOCOL_VERSION, ServerMessages};
use log::{info,error};
use reactive_mutiny::prelude::ChannelProducer;


const MATCH_CONFIG: MatchConfig = MatchConfig {
    score_limit:            3000,
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
    umpire: Option<Umpire>,
}

impl ClientProtocolProcessor {

    pub fn new() -> Self {
        Self {
            umpire: None,
        }
    }

    pub fn client_events_callback(&self, connection_event: ConnectionEvent<ClientMessages>) {
        match connection_event {
            ConnectionEvent::PeerConnected { peer } => {
                info!("Connected: {:?}", peer);
                peer.sender.try_send(|slot| *slot = ClientMessages::Config(MATCH_CONFIG));
            },
            ConnectionEvent::PeerDisconnected { peer } => {
                info!("Disconnected: {:?}", peer);
            }
            ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                info!("Ping-Pong client shutdown requested. Notifying the server within {timeout_ms}ms...");
            }
        }
    }

    pub fn dialog_processor<RemoteStreamType: Stream<Item=SocketProcessorDerivedType<ServerMessages>>>
                           (&self, server_addr: String, port: u16, peer: Arc<Peer<ClientMessages>>, server_messages_stream: RemoteStreamType) -> impl Stream<Item=ClientMessages> {
//        let session = Arc::clone(&
        server_messages_stream.map(|server_message| {
            match &*server_message {

                ServerMessages::GameStarted => {
                    ClientMessages::Version
                },

                ServerMessages::PingPongEvent(ping_pong_event) => {
                    // ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.11025572 }, resulting_event: TurnFlipEvents::SuccessfulRebate })
                    match ping_pong_event {
                        PingPongEvent::TurnFlip { player_action, resulting_event } => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => {
                                    ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.11025572 }, resulting_event: TurnFlipEvents::SuccessfulRebate })
                                },
                                TurnFlipEvents::SoftFaultService => {
                                    ClientMessages::NoAnswer
                                },
                                TurnFlipEvents::SuccessfulRebate => {
                                    ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.11025572 }, resulting_event: TurnFlipEvents::SuccessfulRebate })
                                },
                            }
                        },
                        PingPongEvent::HardFault { player_action, resulting_fault_event } => {
                            ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.15 }, resulting_event: TurnFlipEvents::SuccessfulService })
                        },
                        PingPongEvent::SoftFault { player_action, resulting_fault_event } => {
                            ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.15 }, resulting_event: TurnFlipEvents::SuccessfulService })
                        },
                        PingPongEvent::Score { point_winning_player, last_player_action, last_fault } => {
                            ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.15 }, resulting_event: TurnFlipEvents::SuccessfulService })
                        },
                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_player_action, last_fault } => {
                                    info!("Game ended with score of {:?}", final_score);
                                    ClientMessages::EndorsedScore
                                }
                                GameOverStates::GameCancelled { partial_score, broken_rule_description } => {
                                    ClientMessages::NoAnswer
                                }
                            }
                        },
                    }
                },

                ServerMessages::Version(server_protocol_version) => {
                    if server_protocol_version == PROTOCOL_VERSION {
                        ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.15 }, resulting_event: TurnFlipEvents::SuccessfulService })
                    } else {
                        let msg = format!("Client protocol version is '{PROTOCOL_VERSION}' while server is '{server_protocol_version}'");
                        error!("{}", msg);
                        ClientMessages::Error(msg)
                    }
                }

                ServerMessages::Error(err) => {
                    error!("Server answered with error '{err}'");
                    ClientMessages::Quit
                },

                ServerMessages::NoAnswer => {
                    panic!("NoAnswer message was received");
                },

                ServerMessages::GoodBye | ServerMessages::ServerShutdown => {
                    ClientMessages::NoAnswer
                },
            }
        })
    }

}

