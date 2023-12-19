//! Resting place of [ClientProtocolProcessor]

use crate::common::{
    protocol_model::{ClientMessages, PROTOCOL_VERSION, ServerMessages},
    logic::{
        ping_pong_logic::{act, Umpire},
        ping_pong_models::{GameOverStates, GameStates, MatchConfig, PingPongEvent, Players, TurnFlipEvents},
        protocol_processor::{react_to_hard_fault, react_to_rally_event, react_to_score, react_to_service_soft_fault},
    }
};
use reactive_messaging::prelude::{ConnectionEvent, Peer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Instant,
};
use reactive_mutiny::prelude::FullDuplexUniChannel;
use futures::{Stream, stream, StreamExt};
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

    pub fn client_events_callback<const NETWORK_CONFIG: u64,
                                  SenderChannel:        FullDuplexUniChannel<ItemType=ClientMessages, DerivedItemType=ClientMessages> + Send + Sync>
                                 (self: &Arc<Self>,
                                  connection_event: ConnectionEvent<NETWORK_CONFIG, ClientMessages, SenderChannel>) {
        match connection_event {
            ConnectionEvent::PeerConnected { peer } => {
                debug!("Connected: {:?}", peer);
                _ = peer.send(ClientMessages::Config(MATCH_CONFIG));
            },
            ConnectionEvent::PeerDisconnected { peer, stream_stats } => {
                let in_messages_count = self.in_messages_count.load(Relaxed);
                info!("Disconnected: {:?}; stats: {:?} -- with {} messages IN & OUT: {:.2}/s",
                      peer,
                      stream_stats,
                      in_messages_count, in_messages_count as f64 / self.start_instant.elapsed().as_secs_f64());
            }
            ConnectionEvent::LocalServiceTermination => {
                info!("Ping-Pong client shutdown requested. Notifying the server...");
            }
        }
    }

    pub fn dialog_processor<const NETWORK_CONFIG: u64,
                            SenderChannel:        FullDuplexUniChannel<ItemType=ClientMessages, DerivedItemType=ClientMessages> + Send + Sync,
                            StreamItemType:       AsRef<ServerMessages>>

                           (self:                   &Arc<Self>,
                            server_addr:            String,
                            port:                   u16,
                            peer:                   Arc<Peer<NETWORK_CONFIG, ClientMessages, SenderChannel>>,
                            server_messages_stream: impl Stream<Item=StreamItemType>)

                           -> impl Stream<Item=ClientMessages> {
                            
        let cloned_self = Arc::clone(self);
        let mut umpire = Umpire::new(&MATCH_CONFIG, Players::Ourself);
        server_messages_stream.map(move |server_message| {
            cloned_self.in_messages_count.fetch_add(1, Relaxed);
            match server_message.as_ref() {

                ServerMessages::GameStarted => {
                    vec![ClientMessages::Version]
                },

                ServerMessages::MatchConfig(match_config) => {
                    info!("Server told us it is using {:?}", match_config);
                    vec![/*ClientMessages::NoAnswer*/]
                }

                ServerMessages::PingPongEvent(reported_ping_pong_event) => {
                    // ClientMessages::PingPongEvent(PingPongEvent::TurnFlip { player_action: PlayerAction { lucky_number: 0.11025572 }, resulting_event: TurnFlipEvents::SuccessfulRebate })
                    match reported_ping_pong_event {
                        PingPongEvent::TurnFlip { player_action: opponent_action, resulting_event } => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => vec![
                                    ClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                       "WaitingForService",
                                                                                       |rs| matches!(rs, GameStates::WaitingForService { attempt: _ }),
                                                                                       opponent_action,
                                                                                       /*reported_ping_pong_event*/PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulService }))
                                ],
                                TurnFlipEvents::SoftFaultService => vec![
                                    ClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                       "WaitingForService` or `Rally",
                                                                                       |rs| matches!(rs, GameStates::WaitingForService { attempt: _ }),
                                                                                       opponent_action,
                                                                                       /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SoftFaultService }))
                                ],
                                TurnFlipEvents::SuccessfulRebate => vec![
                                    ClientMessages::PingPongEvent(react_to_rally_event(&mut umpire,
                                                                                       "Rally",
                                                                                       |rs| matches!(rs, GameStates::Rally),
                                                                                       opponent_action,
                                                                                       /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulRebate }))
                                ],
                            }
                        },
                        PingPongEvent::HardFault { player_action: opponent_action, resulting_fault_event } => {
                            react_to_hard_fault(&mut umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(ClientMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::SoftFault { player_action: opponent_action, resulting_fault_event } => {
                            react_to_service_soft_fault(&mut umpire, opponent_action, resulting_fault_event).into_iter()
                                .map(ClientMessages::PingPongEvent)
                                .collect()
                        },
                        PingPongEvent::Score { point_winning_player, last_player_action, last_fault } => {
                            if *point_winning_player != Players::Opponent {
                                error!("TO-BE-REMOVED Unrepresentable state IN THE CLIENT: It is not up to any client ({:?}) to tell the server that a score was made", *peer);
                                vec![ClientMessages::Quit]
                            } else {
                                // our score: opponent's hard fault
                                react_to_score(&mut umpire, last_player_action, last_fault).into_iter()
                                    .map(ClientMessages::PingPongEvent)
                                    .collect()
                            }
                        },
                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_player_action: _, last_fault: _ } => {
                                    info!("Game ended: {} Client; {} Server @ {}:{}", final_score.opponent, final_score.oneself, server_addr, port);
                                    vec![ClientMessages::EndorsedScore]
                                }
                                GameOverStates::GameCancelled { partial_score: _, broken_rule_description: _ } => {
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
        .flat_map(stream::iter)
    }

}

