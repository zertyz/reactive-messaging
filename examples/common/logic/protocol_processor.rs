//! Defines the functions to be used as the dialogue processors of server & clients

use crate::common::logic::ping_pong_logic::{act, Umpire};
use crate::common::logic::ping_pong_models::{GameStates, Players, PingPongEvent, GameOverStates, PlayerAction, FaultEvents};



/// Feeds the `opponent_action` to our `umpire`, progressing on the match and checking for matching game states and game events.\
/// Then, reacts to the opponent's action by making a move, progressing the game further and returning the `PingPongEvent` that the
/// opponent may use to verify our actions.
pub fn react_to_rally_event(umpire:                  &mut Umpire,
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
            let our_event = umpire.process_turn(Players::Ourself, &our_action);
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
pub fn react_to_hard_fault(umpire: &mut Umpire, opponent_action: &PlayerAction, reported_fault_event: &FaultEvents) -> Vec<PingPongEvent> {
    let expected_event = umpire.process_turn(Players::Opponent, opponent_action);
    if let PingPongEvent::HardFault { player_action: _opponent_action, resulting_fault_event: expected_fault_event } = expected_event {
        if &expected_fault_event == reported_fault_event {
            service_ball(umpire)
        } else {
            // game rule offense: unexpected event
            let broken_rule_description = format!("You said your action `{:?}` lead to a `{:?}` event, but our umpire said the resulting event would be `{:?}`",
                                                         opponent_action, reported_fault_event, expected_event);
            vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
        }
    } else {
        // game rule offense: unexpected game state
        let broken_rule_description = format!("You said your action `{:?}` lead to the HardFault `{:?}`, but our umpire said the resulting event was `{:?}`",
                                                     opponent_action, reported_fault_event, expected_event);
        vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
    }
}

/// Feeds the service `opponent_action` (in the opponent's disfavour) to our `umpire`, returning our reaction `PingPongEvent`
/// -- or, possibly, not resulting in any response, as soft faults are tolerated while servicing
pub fn react_to_service_soft_fault(umpire: &mut Umpire, opponent_action: &PlayerAction, reported_fault_event: &FaultEvents) -> Vec<PingPongEvent> {
    let expected_event = umpire.process_turn(Players::Opponent, opponent_action);
    if let PingPongEvent::SoftFault { player_action: _opponent_action, resulting_fault_event: expected_fault_event } = expected_event {
        if &expected_fault_event == reported_fault_event {
            if let GameStates::WaitingForService { attempt } = umpire.state() {
                if *attempt <= Umpire::SERVICING_ATTEMPTS {
                    // another service is to be attempted by the opposite player, so we take no action
                    vec![]
                } else {
                    // game rule offense: exceeded servicing attempts
                    let broken_rule_description = format!("You said your action `{:?}` lead to a `{:?}` event, but our umpire said the resulting event would be `{:?}`",
                                                                 opponent_action, reported_fault_event, expected_event);
                    vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
                }
            } else {
                // game rule offense: exceeded servicing attempts
                let broken_rule_description = format!("You said your SoftFault after the action `{:?}` lead to a `WaitForService` state, but `{:?}` was found",
                                                             opponent_action, umpire.state());
                vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
            }
        } else {
            // game rule offense: unexpected event
            let broken_rule_description = format!("You said your action `{:?}` lead to a `{:?}` event, but our umpire said the resulting event would be `{:?}`",
                                                         opponent_action, reported_fault_event, expected_event);
            vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
        }
    } else {
        // game rule offense: unexpected game state
        let broken_rule_description = format!("You said your action `{:?}` lead to the SoftFault `{:?}`, but our umpire said the resulting event was `{:?}`",
                                                     opponent_action, reported_fault_event, expected_event);
        vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
    }
}

/// Feeds the `opponent_action` (that led to a score in the opponent's disfavour) to our `umpire`, returning our reaction `PingPongEvent`
/// -- checking for the end of the game (in our favour) or servicing a new ball
pub fn react_to_score(umpire: &mut Umpire, opponent_action: &PlayerAction, reported_fault_event: &FaultEvents) -> Vec<PingPongEvent> {
    let expected_event = umpire.process_turn(Players::Opponent, opponent_action);
    if let PingPongEvent::Score { point_winning_player: _, last_player_action: _opponent_action, last_fault: expected_fault_event } = expected_event {
        if &expected_fault_event == reported_fault_event {
            service_ball(umpire)
        } else {
            // game rule offense: unexpected event
            let broken_rule_description = format!("You said your action `{:?}` lead to a `{:?}` event, but our umpire said the resulting event would be `{:?}`",
                                                  opponent_action, reported_fault_event, expected_event);
            vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
        }
    } else {
        // game rule offense: unexpected game state
        let broken_rule_description = format!("You said your action `{:?}` lead to the Score `{:?}`, but our umpire said the resulting event was `{:?}` (notice the player names, as they should be flipped)",
                                              opponent_action, reported_fault_event, expected_event);
        vec![PingPongEvent::GameOver(GameOverStates::GameCancelled { partial_score: *umpire.score(), broken_rule_description })]
    }
}

/// Returns the oppositor to `player`
pub fn opposite_player(player: Players) -> Players {
    match player {
        Players::Ourself => Players::Opponent,
        Players::Opponent => Players::Ourself,
    }
}

/// Service the ball, accounting for any soft faults that might happen
/// -- soft fault events during service are tolerated, causing a new service to be attempted.\
/// Notice it is up to the umpire to enforce a limit on the number of attempts, in which case a soft fault
/// would become a hard fault.
pub fn service_ball(umpire: &mut Umpire) -> Vec<PingPongEvent> {
    let mut events = Vec::with_capacity(Umpire::SERVICING_ATTEMPTS as usize);
    loop {
        let our_action = act();
        let our_event = umpire.process_turn(Players::Ourself, &our_action);
        match our_event {
            PingPongEvent::SoftFault { player_action: _, resulting_fault_event: _ } => {
                events.push(our_event);
            },
            _ => {
                events.push(our_event);
                break
            },
        }
    }
    events
}