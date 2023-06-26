//! Defines the functions to be used as the dialogue processors of server & clients

use std::cell::UnsafeCell;
use std::sync::Arc;
use reactive_messaging::{
    prelude::{
        Peer,
        ProcessorRemoteStreamType
    },
    ConnectionEvent,
};
use crate::common::logic::ping_pong_logic::{act, Umpire};
use crate::common::protocol_model::{ClientMessages, PROTOCOL_VERSION, ServerMessages};
use crate::common::logic::ping_pong_models::{GameStates, Players, TurnFlipEvents, PingPongEvent, GameOverStates, PlayerAction, FaultEvents};
use dashmap::DashMap;
use futures::stream::{Stream, StreamExt};
use log::{info, warn, error};

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
                info!("Connected: {:?}", peer);
                self.sessions.insert(peer.peer_id, Arc::new(Session { umpire: UnsafeCell::new(None) }));
            },
            ConnectionEvent::PeerDisconnected { peer } => {
                info!("Disconnected: {:?}", peer);
                //let _ = processor_uni.try_send(|slot| *slot = ClientMessages::Quit);
                self.sessions.remove(&peer.peer_id);
            }
            ConnectionEvent::ApplicationShutdown { timeout_ms } => {
                info!("Ping-Pong server shutdown requested. Notifying all peers within {timeout_ms}ms...");
            }
        }
    }

    pub fn dialog_processor(&self, client_addr: String, port: u16, peer: Arc<Peer<ServerMessages>>, client_messages_stream: ProcessorRemoteStreamType<ClientMessages>) -> impl Stream<Item=ServerMessages> {
        let mut session = self.sessions.get(&peer.peer_id)
                                                   .unwrap_or_else(|| panic!("Server BUG! Peer {:?} showed up, but we don't have a session for it! It should have been created by the `connection_events()` callback", peer))
                                                   .value()
                                                   .clone();     // .clone() the Arc, so we are free to move it to the the next closure (and drop it after the Stream closes)
        client_messages_stream.map(move |client_message| {

            // get the game's umpire instance or expect the first client message to be the one to create the match umpire
            let umpire_option = unsafe { &mut * (session.umpire.get()) };
            let mut umpire = match umpire_option {
                Some(umpire) => umpire,
                None => return {
                    if let ClientMessages::Config(match_config) = &*client_message {
                        // instantiate the game
                        let umpire = Umpire::new(&match_config, Players::Opponent);
                        umpire_option.replace(umpire);
                        ServerMessages::GameStarted
                    } else {
                        ServerMessages::Error(format!("The first message sent must be `Config(match_config)` -- the received one was `{:?}`", client_message))
                    }
                }
            };

            // from this point on, we have a configured umpire in the `umpire` variable

            match &*client_message {

                ClientMessages::Config(offending_match_config) => {
                    // protocol offense
                    ServerMessages::Error(format!("Protocol Offense: Was `Config` sent twice? You just sent `Config({:?}) , but we have `{:?}` already associated with you (due to a previous call to `Config()`)", offending_match_config, umpire.config()))
                },

                ClientMessages::PingPongEvent(reported_ping_pong_event) => {
                    match reported_ping_pong_event {
                        PingPongEvent::TurnFlip { player_action: opponent_action, resulting_event} => {
                            match resulting_event {
                                TurnFlipEvents::SuccessfulService => {
                                    ServerMessages::PingPongEvent( react_to_rally_events(&mut umpire,
                                                                                         "WaitingForService",
                                                                                         |rs| if let GameStates::WaitingForService { attempt } = rs {true} else {false},
                                                                                         opponent_action,
                                                                                         /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulService } ) )
                                },
                                TurnFlipEvents::SoftFaultService => {
                                    ServerMessages::PingPongEvent( react_to_rally_events(&mut umpire,
                                                                                         "WaitingForService` or `Rally",
                                                                                         |rs| if let GameStates::WaitingForService { attempt: _ } | GameStates::Rally = rs {true} else {false},
                                                                                         opponent_action,
                                                                                         /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SoftFaultService } ) )
                                },
                                TurnFlipEvents::SuccessfulRebate => {
                                    ServerMessages::PingPongEvent( react_to_rally_events(&mut umpire,
                                                                                         "Rally",
                                                                                         |rs| if let GameStates::Rally = rs {true} else {false},
                                                                                         opponent_action,
                                                                                         /*reported_ping_pong_event*/ PingPongEvent::TurnFlip { player_action: *opponent_action, resulting_event: TurnFlipEvents::SuccessfulRebate } ) )
                                },
                            }
                        }
                        PingPongEvent::HardFault { player_action: opponent_action, resulting_fault_event} => {
                            ServerMessages::PingPongEvent( react_to_hard_fault(&mut umpire, opponent_action, resulting_fault_event) )
                        },
                        PingPongEvent::SoftFault { player_action: opponent_action, resulting_fault_event} => {
                            // can't thing of that scenario now... fix it, future me!
                            error!("WTF?? A SoftFault??? WHHHYYYYYYYYYYYYYYYY????? client: {:?}; event: {:?}", *peer, resulting_fault_event);
                            ServerMessages::GoodBye
                        },
                        PingPongEvent::Score { point_winning_player, opponent_fault } => {
                            error!("TO-BE-REMOVED Unrepresentable state: It is not up to any client ({:?}) to tell the server that a score was made", *peer);
                            ServerMessages::GoodBye
                        },

                        PingPongEvent::GameOver(game_over_state) => {
                            match game_over_state {
                                GameOverStates::GracefullyEnded { final_score, last_fault } => {
                                    error!("TO-BE-REMOVED Unrepresentable state: It is not up to any client ({:?}) to tell the server that the game is over (gracefully, in this case)", *peer);
                                    ServerMessages::GoodBye
                                },
                                GameOverStates::GameCancelled { partial_score, broken_rule_description } => {
                                    error!("TO-BE-REMOVED Unrepresentable state: It is not up to any client ({:?}) to tell the server that the game is over -- cancelled @ {:?} due to a broken rule: '{}'", *peer, partial_score, broken_rule_description);
                                    // in this case, the client must report it to the server through an error message
                                    ServerMessages::GoodBye
                                },
                            }
                        },
                    }
                }
                ClientMessages::ContestedScore(client_provided_match_score) => {
                    warn!("Client {:?} contested the match score. Ours: {:?}; Theirs: {:?}", peer, umpire.score(), client_provided_match_score);
                    ServerMessages::GoodBye
                },

                ClientMessages::EndorsedScore => {
                    ServerMessages::GoodBye
                },

                ClientMessages::Error(err) => {
                    error!("Client {:?} errored. Closing the connection after receiving: '{}'", *peer, err);
                    ServerMessages::GoodBye
                },

                ClientMessages::NoAnswer => {
                    panic!("BUG: received a `NoAnswer` message")
                },

                ClientMessages::Quit => {
                    ServerMessages::GoodBye
                },

                ClientMessages::Version => {
                    ServerMessages::Version(PROTOCOL_VERSION.to_string())
                },
            }
        })
    }

}

/// Feeds the `opponent_action` to our `umpire`, progressing on the match and checking for matching game states and game events.\
/// Then, reacts to the opponent's action by making a move, progressing the game further and returning the `PingPongEvent` that the
/// opponent may use to verify our actions.
fn react_to_rally_events(umpire:                  &mut Umpire,
                         reported_state_name:     &str,
                         reported_state_matcher:  impl FnOnce(&GameStates) -> bool,
                         opponent_action:         &PlayerAction,
                         reported_event:          PingPongEvent)
                         -> PingPongEvent {

    let expected_state = umpire.state();
    if reported_state_matcher(expected_state) {
        let expected_event = umpire.process_turn(Players::Opponent, opponent_action);
        if reported_event == expected_event {
            // the opponent successfully sent us the ball. Lets take an action to rebate it!
            let our_action = act();
            let our_event = umpire.process_turn(Players::OneSelf, &our_action);
            our_event
        } else {
            // game rule offense: unexpected event
            let broken_rule_description = format!("You said your action `{:?}` lead to a `{:?}` event, but our umpire said the resulting event would be `{:?}`",
                                                  opponent_action, reported_event, expected_event);
            PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })
        }
    } else {
        // game rule offense: unexpected game state
        let broken_rule_description = format!("Your said your action `{:?}` was taken due to the game being in the `{:?}` state, but our umpire said the game state was `{:?}`",
                                              opponent_action, reported_state_name, expected_state);
        PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })
    }

}

/// Feeds the `opponent_action` (in the opponent's disfavour) to our `umpire`, returning our reaction `PingPongEvent`
/// -- checking for the end of the game (in our favour) or servicing a new ball
fn react_to_hard_fault(umpire: &mut Umpire, opponent_action: &PlayerAction, reported_fault_event: &FaultEvents) -> PingPongEvent {
    let expected_event = umpire.process_turn(Players::Opponent, opponent_action);
    if let PingPongEvent::HardFault { player_action: _opponent_action, resulting_fault_event: expected_fault_event } = expected_event {
        if &expected_fault_event == reported_fault_event {
            // service a new ball
            let our_action = act();
            let our_event = umpire.process_turn(Players::OneSelf, &our_action);
            our_event
        } else {
            // game rule offense: unexpected event
            let broken_rule_description = format!("You said your action `{:?}` lead to a `{:?}` event, but our umpire said the resulting event would be `{:?}`",
                                                         opponent_action, reported_fault_event, expected_event);
            PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })
        }
    } else {
        // game rule offense: unexpected game state
        let broken_rule_description = format!("You said your action `{:?}` lead to the HardFault `{:?}`, but our umpire said the resulting state was `{:?}`",
                                                     opponent_action, reported_fault_event, expected_event);
        PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })
    }
}