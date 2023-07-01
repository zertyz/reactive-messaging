//! Resting place of [ClientProtocolProcessor]

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;
use futures::{Stream, stream, StreamExt};
use reactive_messaging::prelude::{ConnectionEvent, Peer, ProcessorRemoteStreamType, SocketProcessorDerivedType};
use crate::common::logic::ping_pong_logic::{act, Umpire};
use crate::common::logic::ping_pong_models::{GameOverStates, GameStates, MatchConfig, PingPongEvent, PlayerAction, Players, TurnFlipEvents};
use crate::common::protocol_model::{ClientMessages, PROTOCOL_VERSION, ServerMessages};
use reactive_mutiny::prelude::ChannelProducer;
use crate::common::logic::protocol_processor::{react_to_hard_fault, react_to_rally_event, react_to_score, react_to_service_soft_fault};
use log::{debug,info,error};


const MATCH_CONFIG: MatchConfig = MatchConfig {
    score_limit:            5000,
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

    pub fn client_events_callback(self: &Arc<Self>, connection_event: ConnectionEvent<ClientMessages>) {
        match connection_event {
            ConnectionEvent::PeerConnected { peer } => {
                debug!("Connected: {:?}", peer);
                peer.sender.try_send(|slot| *slot = ClientMessages::Config(MATCH_CONFIG));
            },
            ConnectionEvent::PeerDisconnected { peer } => {
                let in_messages_count = self.in_messages_count.load(Relaxed);
                info!("Disconnected: {:?} -- with {} messages IN & OUT: {:.2}/s",
                      peer, in_messages_count, in_messages_count as f64 / self.start_instant.elapsed().as_secs_f64());
            }
            ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                info!("Ping-Pong client shutdown requested. Notifying the server within {timeout_ms}ms...");
            }
        }
    }

    pub fn dialog_processor<RemoteStreamType: Stream<Item=SocketProcessorDerivedType<ServerMessages>>>
                           (self: &Arc<Self>, server_addr: String, port: u16, peer: Arc<Peer<ClientMessages>>, server_messages_stream: RemoteStreamType) -> impl Stream<Item=ClientMessages> {
        let cloned_self = Arc::clone(&self);
        let mut umpire = Umpire::new(&MATCH_CONFIG, Players::Ourself);
        server_messages_stream.map(move |server_message| {
            cloned_self.in_messages_count.fetch_add(1, Relaxed);
            match &*server_message {

                ServerMessages::GameStarted => {
                    vec![ClientMessages::Version]
                },

                ServerMessages::PingPongEvent(reported_ping_pong_event) => {
                    // ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.11025572 }, resulting_event: TurnFlipEvents::SuccessfulRebate })
                    match reported_ping_pong_event {
                        PingPongEvent::TurnFlip { player_action: opponent_action, resulting_event } => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => vec![
                                    ClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                       "WaitingForService",
                                                                                       |rs| if let GameStates::WaitingForService { attempt } = rs { true } else { false },
                                                                                       opponent_action,
                                                                                       /*reported_ping_pong_event*/PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulService }))
                                ],
                                TurnFlipEvents::SoftFaultService => vec![
                                    ClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                       "WaitingForService` or `Rally",
                                                                                       |rs| if let GameStates::WaitingForService { attempt: _ } | GameStates::Rally = rs { true } else { false },
                                                                                       opponent_action,
                                                                                       /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SoftFaultService }))
                                ],
                                TurnFlipEvents::SuccessfulRebate => vec![
                                    ClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                       "Rally",
                                                                                       |rs| if let GameStates::Rally = rs { true } else { false },
                                                                                       opponent_action,
                                                                                       /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulRebate }))
                                ],
                            }
                        },
                        PingPongEvent::HardFault { player_action: opponent_action, resulting_fault_event } => {
                            react_to_hard_fault(&mut umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(|ping_pong_event| ClientMessages::PingPongEvent(ping_pong_event))
                                .collect()
                        },
                        PingPongEvent::SoftFault { player_action: opponent_action, resulting_fault_event } => {
                            react_to_service_soft_fault(&mut umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(|ping_pong_event| ClientMessages::PingPongEvent(ping_pong_event))
                                .collect()
                        },
                        PingPongEvent::Score { point_winning_player, last_player_action, last_fault } => {
                            if *point_winning_player != Players::Opponent {
                                error!("TO-BE-REMOVED Unrepresentable state IN THE CLIENT: It is not up to any client ({:?}) to tell the server that a score was made", *peer);
                                vec![ClientMessages::Quit]
                            } else {
                                // our score: opponent's hard fault
                                react_to_score(&mut umpire, last_player_action, last_fault).into_iter()
                                    .map(|ping_pong_event| ClientMessages::PingPongEvent(ping_pong_event))
                                    .collect()
                            }
                        },
                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_player_action, last_fault } => {
                                    info!("Game ended: {} Client; {} Server @ {}:{}", final_score.opponent, final_score.oneself, server_addr, port);
                                    vec![ClientMessages::EndorsedScore]
                                }
                                GameOverStates::GameCancelled { partial_score, broken_rule_description } => {
                                    vec![/*ClientMessages::NoAnswer*/]
                                }
                            }
                        },
                    }
                },

                ServerMessages::Version(server_protocol_version) => {
                    if server_protocol_version == PROTOCOL_VERSION {
                        // Start the game: service the ball
                        let our_action = act();
                        let our_event = umpire.process_turn(Players::Ourself, &our_action);
                        vec![ClientMessages::PingPongEvent(our_event)]
                    } else {
                        let msg = format!("Client protocol version is '{PROTOCOL_VERSION}' while server is '{server_protocol_version}'");
                        error!("{}", msg);
                        vec![ClientMessages::Error(msg)]
                    }
                }

                ServerMessages::Error(err) => {
                    error!("Server answered with error '{err}'");
                    vec![ClientMessages::Quit]
                },

                ServerMessages::NoAnswer => {
                    panic!("NoAnswer message was received");
                },

                ServerMessages::GoodBye | ServerMessages::ServerShutdown => {
                    vec![/*ClientMessages::NoAnswer*/]
                },
            }
        })
        .flat_map(|responses| stream::iter(responses))
    }

}

