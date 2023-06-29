//! Defines the functions to be used as the dialogue processors of server & clients

use std::cell::UnsafeCell;
use std::sync::Arc;
use reactive_messaging::{
    prelude::{
        Peer,
        ProcessorRemoteStreamType
    },
};
use crate::common::logic::ping_pong_logic::{act, Umpire};
use crate::common::protocol_model::{ClientMessages, PROTOCOL_VERSION, ServerMessages};
use crate::common::logic::ping_pong_models::{GameStates, Players, TurnFlipEvents, PingPongEvent, GameOverStates, PlayerAction, FaultEvents};
use dashmap::DashMap;
use futures::stream::{Stream, StreamExt};
use log::{info, warn, error};
use reactive_messaging::prelude::ConnectionEvent;


/// Feeds the `opponent_action` to our `umpire`, progressing on the match and checking for matching game states and game events.\
/// Then, reacts to the opponent's action by making a move, progressing the game further and returning the `PingPongEvent` that the
/// opponent may use to verify our actions.
pub fn react_to_rally_events(umpire:                  &mut Umpire,
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
pub fn react_to_hard_fault(umpire: &mut Umpire, opponent_action: &PlayerAction, reported_fault_event: &FaultEvents) -> PingPongEvent {
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