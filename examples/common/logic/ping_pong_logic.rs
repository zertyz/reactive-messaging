//! Implementation of the Ping-Pong game logic as described in [protocol_model]

use super::ping_pong_models::*;
use rand::Rng;


/// Represents a Ping-Pong match "official" or "Umpire", responsible for ensuring the ping-pong rules are followed and for accounting scores & declaring the winner.\
/// It is able to monitor the match to be done turn-by-turn, issuing [PingPongEvent]s of interest and keeping the game state.
pub struct Umpire {
    config:      MatchConfig,
    score:       MatchScore,
    state:       GameStates,
    turn_player: Players,
}

impl Umpire {

    /// The maximum number of [PingPongEvent::SoftFault]s a player may commit while servicing, before the turn is flipped to the opponent
    pub const SERVICING_ATTEMPTS: u32 = 3;

    /// Creates a new Ping-Pong match to be managed according to the given `config`, where the player to service the first ball is designated by `server_player`
    pub fn new(config: &MatchConfig, first_service_player: Players) -> Self {
        let score = MatchScore {
            limit:  config.score_limit,
            oneself: 0,
            opponent: 0,
        };
        Self {
            config: *config,
            score,
            state: GameStates::WaitingForService { attempt: 1 },
            turn_player: first_service_player,
        }
    }

    /// Process the given `game_event`, returning the next game event --
    /// to be passed again to this function while the event is not [PingPongEvent::GameOver]
    pub fn process_turn(&mut self, player: Players, action: &PlayerAction) -> PingPongEvent {
        // check if it is the right player's turn
        if self.turn_player != player {
            let game_over_state = GameOverStates::GameCancelled {
                partial_score: self.score,
                broken_rule_description: format!("The expected player for this turn was {:?}, but {:?} claimed it instead", self.turn_player, player),
            };
            self.state = GameStates::GameOver(game_over_state.clone());
            PingPongEvent::GameOver(game_over_state)
        } else {
            match self.state.clone() {
                GameStates::WaitingForService { attempt } => {
                    self.consider_service_soft_fault(action, attempt)
                        .map(|soft_fault| {
                            self.state = GameStates::WaitingForService { attempt: attempt + 1 };
                            PingPongEvent::SoftFault { player_action: *action, resulting_fault_event: soft_fault }
                        })
                        .unwrap_or_else(|| {
                            self.consider_hard_fault(action)
                                .map(|hard_fault| self.compute_lost_score(hard_fault, *action))
                                .unwrap_or_else(|| {
                                    self.state = GameStates::Rally;
                                    self.progress_on_the_rally(*action, TurnFlipEvents::SuccessfulService)
                                })
                        })
                },
                GameStates::Rally => {
                    self.consider_hard_fault(action)
                        .map(|hard_fault| self.compute_lost_score(hard_fault, *action))
                        .unwrap_or_else(|| self.progress_on_the_rally(*action, TurnFlipEvents::SuccessfulRebate))
                }
                GameStates::GameOver(_game_over_state) => {
                    panic!("common::logic::process_turn(): Attempted to process another event after the game is over")
                }
            }
        }
    }

    /// Returns the [MatchConfig] used for this game
    pub fn config(&self) -> &MatchConfig {
        &self.config
    }

    /// Returns the current [MatchScore]
    pub fn score(&self) -> &MatchScore {
        &self.score
    }

    /// Returns the current [GameState]
    pub fn state(&self) -> &GameStates {
        &self.state
    }

    /// the action took by the player was skilled enough to let the score remain under dispute, with the ball still flying around
    fn progress_on_the_rally(&mut self, player_action: PlayerAction, turn_flip_event: TurnFlipEvents) -> PingPongEvent {
        self.next_turn_player();
        PingPongEvent::TurnFlip { player_action, resulting_event: turn_flip_event }
    }

    /// either end the game (if the score limit was reached) or compute the score to the opponent, setting it to service the next ball
    fn compute_lost_score(&mut self, hard_fault: FaultEvents, action: PlayerAction) -> PingPongEvent {
        match self.turn_player {
            Players::Ourself => self.score.opponent += 1,
            Players::Opponent => self.score.oneself += 1,
        }
        if self.score.oneself >= self.config.score_limit ||
           self.score.opponent >= self.config.score_limit {
            let game_over_state = GameOverStates::GracefullyEnded {
                final_score:        self.score,
                last_player_action: action,
                last_fault:         hard_fault,
            };
            self.state = GameStates::GameOver(game_over_state.clone());
            PingPongEvent::GameOver(game_over_state)
        } else {
            self.next_turn_player();
            self.state = GameStates::WaitingForService { attempt: 1 };
            PingPongEvent::Score {
                point_winning_player: self.turn_player,
                last_player_action:   action,
                last_fault:           hard_fault,
            }
        }
    }

    /// switches the player expected to play the next turn
    fn next_turn_player(&mut self) {
        self.turn_player = match self.turn_player {
            Players::Ourself => Players::Opponent,
            Players::Opponent => Players::Ourself,
        }
    }

    /// soft [FaultEvents] are faults that cause the servicing to repeat up to the 3rd attempt -- returns `None` if there was no such fault
    fn consider_service_soft_fault(&self, action: &PlayerAction, attempt: u32) -> Option<FaultEvents> {
        if attempt < Self::SERVICING_ATTEMPTS {
            if action.lucky_number <= self.config.mishit_probability {
                return Some(FaultEvents::Mishit)
            } else if action.lucky_number <= self.config.net_touch_probability {
                return Some(FaultEvents::NetTouch)
            }
        }
        None
    }

    /// returns one of the [FaultEvents] events triggered by `lucky_number` when considering the probabilities from [MatchConfig]
    fn consider_hard_fault(&self, action: &PlayerAction) -> Option<FaultEvents> {
        if action.lucky_number <= self.config.no_bounce_probability {
            Some(FaultEvents::NoBounce)
        } else if action.lucky_number <= self.config.no_rebate_probability {
            Some(FaultEvents::NoRebate)
        } else if action.lucky_number <= self.config.mishit_probability {
            Some(FaultEvents::Mishit)
        } else if action.lucky_number <= self.config.pre_bounce_probability {
            Some(FaultEvents::PreBounce)
        } else if action.lucky_number <= self.config.net_block_probability {
            Some(FaultEvents::NetBlock)
        } else if action.lucky_number <= self.config.ball_out_probability {
            Some(FaultEvents::BallOut)
        } else {
            None
        }
    }
}


/// This is how to play a Ping-Pong: by picking a number in 0.00..1.00
pub fn act() -> PlayerAction {
    PlayerAction { lucky_number: rand::thread_rng().gen() }
}


/// Unit tests the [tokio_message_io](self) module
#[cfg(any(test,doc))]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    const DEBUG: bool = true;
    #[cfg(not(debug_assertions))]
    const DEBUG: bool = false;


    #[ctor::ctor]
    fn suite_setup() {
        simple_logger::SimpleLogger::new().with_utc_timestamps().init().unwrap_or_else(|_| eprintln!("--> LOGGER WAS ALREADY STARTED"));
    }

    /// assures the game states progresses as expected, issuing the expected events along the way and that the right scores are computed
    #[cfg_attr(not(doc),test)]
    fn happy_path_game() {
        let config = MatchConfig {
            score_limit:            2,
            rally_timeout_millis:   1000,
            no_bounce_probability:  0.01,
            no_rebate_probability:  0.02,
            mishit_probability:     0.03,
            pre_bounce_probability: 0.04,
            net_touch_probability:  0.05,
            net_block_probability:  0.06,
            ball_out_probability:   0.07,
        };
        let mut umpire = Umpire::new(&config, Players::Ourself);
        let registered_config = umpire.config();
        assert_eq!(registered_config, config, "Initial config not registered correctly");

        // ready to start
        assert_eq!(umpire.state(), GameStates::WaitingForService { attempt: 1 }, "Wrong state when starting the game");
        assert_eq!(umpire.turn_player, Players::Ourself, "next turn's Player is wrong");

        // serviced on the net
        assert_eq!(umpire.process_turn(Players::Ourself, &PlayerAction { lucky_number: config.net_touch_probability }),
                   PingPongEvent::SoftFault(FaultEvents::NetTouch),
                   "Touching the net is a soft fault when attempting to servicing for the first time");
        assert_eq!(umpire.state(), GameStates::WaitingForService { attempt: 2 }, "There should be allowed 3 service attempts when only soft faults happen -- in this case, due to a net touch");
        assert_eq!(umpire.turn_player, Players::Ourself, "next turn's Player is wrong");

        // missed the ball when servicing on attempt #2
        assert_eq!(umpire.process_turn(Players::Ourself, &PlayerAction { lucky_number: config.mishit_probability }),
                   PingPongEvent::SoftFault(FaultEvents::Mishit),
                   "Not hitting the ball is a soft fault when attempting to servicing for the second time");
        assert_eq!(umpire.state(), GameStates::WaitingForService { attempt: 3 }, "There should be allowed 3 service attempts when only soft faults happen -- in this case, due to missing the ball");
        assert_eq!(umpire.turn_player, Players::Ourself, "next turn's Player is wrong");

        // serviced on the net for the 3rd attempt
        assert_eq!(umpire.process_turn(Players::Ourself, &PlayerAction { lucky_number: config.net_touch_probability }),
                   PingPongEvent::Score { point_winning_player: Players::Opponent, last_player_action, last_fault: FaultEvents::NetBlock },
                   "Touching the net on the 3rd service attempt is a hard fault (like a net block during the rally phase)");
        assert_eq!(umpire.state(), GameStates::WaitingForService { attempt: 1 }, "After the 3rd failed attempt, the opponent wins a point and should service");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 0, opponent: 1 }, "wrong scores were computed after a hard fault when servicing");
        assert_eq!(umpire.turn_player, Players::Opponent, "next turn's Player is wrong");

        // serviced straight into a hard fault
        assert_eq!(umpire.process_turn(Players::Opponent, &PlayerAction { lucky_number: config.ball_out_probability }),
                   PingPongEvent::Score { point_winning_player: Players::Ourself, last_player_action, last_fault: FaultEvents::BallOut },
                   "Servicing the ball out wasn't identified");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 1, opponent: 1 }, "Servicing the ball out should give a score to the opponent");
        assert_eq!(umpire.turn_player, Players::Ourself, "next turn's Player is wrong");
        assert_eq!(umpire.state, GameStates::WaitingForService { attempt: 1 }, "A new rally wasn't correctly initiated");

        // a successful service
        assert_eq!(umpire.process_turn(Players::Ourself, &PlayerAction { lucky_number: 1.0 }),
                   PingPongEvent::TurnFlip(TurnFlipEvents::SuccessfulService(PlayerAction { lucky_number: 1.0 })),
                   "A successful Service wasn't identified");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 1, opponent: 1 }, "The score should have remained unchanged");
        assert_eq!(umpire.turn_player, Players::Opponent, "next turn's Player is wrong");
        assert_eq!(umpire.state(), GameStates::Rally, "Wrong game state");

        // a ping
        assert_eq!(umpire.process_turn(Players::Opponent, &PlayerAction { lucky_number: 1.0 }),
                   PingPongEvent::TurnFlip(TurnFlipEvents::SuccessfulRebate(PlayerAction { lucky_number: 1.0 })),
                   "A successful Ping wasn't identified");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 1, opponent: 1 }, "The score should have remained unchanged");
        assert_eq!(umpire.turn_player, Players::Ourself, "next turn's Player is wrong");
        assert_eq!(umpire.state(), GameStates::Rally, "Wrong game state");

        // a pong
        assert_eq!(umpire.process_turn(Players::Ourself, &PlayerAction { lucky_number: 1.0 }),
                   PingPongEvent::TurnFlip(TurnFlipEvents::SuccessfulRebate(PlayerAction { lucky_number: 1.0 })),
                   "A successful Pong wasn't identified");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 1, opponent: 1 }, "The score should have remained unchanged");
        assert_eq!(umpire.turn_player, Players::Opponent, "next turn's Player is wrong");
        assert_eq!(umpire.state(), GameStates::Rally, "Wrong game state");

        // a fault during the rally -- ending the game with the `OneSelf` player winning
        let player_action = PlayerAction { lucky_number: config.no_rebate_probability };
        let expected_game_over_state = GameOverStates::GracefullyEnded { final_score:        MatchScore { limit: 2, oneself: 2, opponent: 1 },
                                                                                         last_player_action: player_action,
                                                                                         last_fault:         FaultEvents::NoRebate };
        assert_eq!(umpire.process_turn(Players::Opponent, &player_action),
                   PingPongEvent::GameOver(expected_game_over_state.clone()),
                   "A NoRebate fault wasn't identified");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 2, opponent: 1 }, "The score should have been updated");
        assert_eq!(umpire.state(), GameStates::GameOver(expected_game_over_state), "Wrong game state");

    }

    /// assures the umpire correctly points out rule violations
    #[cfg_attr(not(doc),test)]
    fn rule_violations() {
        let config = MatchConfig {
            score_limit:            2,
            rally_timeout_millis:   1000,
            no_bounce_probability:  0.01,
            no_rebate_probability:  0.02,
            mishit_probability:     0.03,
            pre_bounce_probability: 0.04,
            net_touch_probability:  0.05,
            net_block_probability:  0.06,
            ball_out_probability:   0.07,
        };

        // wrong player action
        let mut umpire = Umpire::new(&config, Players::Ourself);
        let expected_game_over_state = GameOverStates::GameCancelled { partial_score: MatchScore { limit: 2, oneself: 0, opponent: 0 }, broken_rule_description: format!("The expected player for this turn was OneSelf, but Opponent claimed it instead") };
        assert_eq!(umpire.process_turn(Players::Opponent, &PlayerAction { lucky_number: 1.0 }),
                   PingPongEvent::GameOver(expected_game_over_state.clone()),
                   "The game should be cancelled when a player strikes the ball out of his/her turn");
        assert_eq!(umpire.score, MatchScore { limit: 2, oneself: 0, opponent: 0 }, "The score should have not been touched");
        assert_eq!(umpire.state, GameStates::GameOver(expected_game_over_state), "Wrong game state");

        // playing after the game ended currently panics
        //umpire.process_turn(Players::OneSelf, PlayerAction { lucky_number: 1.0 });

    }


    /// Assures computer vs computer matches works out without discrepancies.\
    /// The test is done by assigning each player with its own umpire, and both must agree at all times, as the match progresses.
    #[cfg_attr(not(doc),test)]
    fn computer_vs_computer() {
        // insane score limit with insane low fault probabilities for really complex matches
        let config = MatchConfig {
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
        let mut player_1_umpire = Umpire::new(&config, Players::Ourself);
        let mut player_2_umpire = Umpire::new(&config, Players::Opponent);

//println!("\n");
        let score = loop {
            let turn_player = player_1_umpire.turn_player;
            let action = act();
            let umpire_1_event = player_1_umpire.process_turn(turn_player, &action);
            let umpire_2_event = player_2_umpire.process_turn(next_turn_player(turn_player), &action);
            assert_game_events(umpire_1_event.clone(), umpire_2_event.clone());
            assert_scores(player_1_umpire.score, player_2_umpire.score);
//println!("Player {:?}: action: {:?}; event: {:?}", turn_player, action, umpire_1_event);
            if let PingPongEvent::GameOver(game_over_state) = umpire_1_event {
                break game_over_state
            }
            assert_eq!(player_2_umpire.state, player_1_umpire.state, "Umpires disagree on the game state");
        };
//println!("Game Ended: {:?}", score);
//println!("Game Manager 1 state: {:?}", player_1_umpire.state);
//println!("Game Manager 2 state: {:?}", player_2_umpire.state);
    }

    /// considering the same match & same turn, asserts that the two events given out by different umpires
    /// (each one recruited for each player) are not in contradiction to each other
    fn assert_game_events(umpire_1_event: PingPongEvent, umpire_2_event: PingPongEvent) {
        match umpire_1_event {
            PingPongEvent::Score { point_winning_player: point_winning_player_1, last_player_action: _, last_fault: opponent_fault_1 } => {
                if let PingPongEvent::Score { point_winning_player: point_winning_player_2, last_player_action: _, last_fault: opponent_fault_2 } = umpire_2_event {
                    assert_eq!(next_turn_player(point_winning_player_2), point_winning_player_1, "Umpires disagree on who scored last");
                    assert_eq!(opponent_fault_2, opponent_fault_1, "Umpires disagree on what was the fault for the last score");
                } else {
                    assert_eq!(umpire_2_event, umpire_1_event, "Umpires disagree on the last game event");
                }
            },
            PingPongEvent::GameOver(ref game_over_event_1) => {
                match game_over_event_1 {
                    GameOverStates::GracefullyEnded { final_score: final_score_1, last_player_action: _, last_fault: last_fault_1 } => {
                        if let PingPongEvent::GameOver(ref game_over_event_2) = umpire_2_event {
                            if let GameOverStates::GracefullyEnded { final_score: final_score_2, last_player_action: _, last_fault: last_fault_2 } = game_over_event_2 {
                                assert_eq!(last_fault_2, last_fault_1, "Umpires disagree on the final match fault");
                                assert_scores(*final_score_1, *final_score_2);
                                return
                            }
                        }
                        assert_eq!(umpire_2_event, umpire_1_event, "Umpires disagree on the last game event");
                    },
                    GameOverStates::GameCancelled { partial_score: partial_score_1, broken_rule_description: broken_rule_description_1 } => {
                        if let PingPongEvent::GameOver(ref game_over_event_2) = umpire_2_event {
                            if let GameOverStates::GameCancelled { partial_score: partial_score_2, broken_rule_description: broken_rule_description_2 } = game_over_event_2 {
                                assert_eq!(broken_rule_description_2, broken_rule_description_1, "Umpires disagree on the broken rule that caused the game to be cancelled");
                                assert_scores(*partial_score_1, *partial_score_2);
                            }
                        }
                    },
                }
            }
            _ => {
                assert_eq!(umpire_2_event, umpire_1_event, "Umpires disagree on the last game event");
            },
        }
    }

    /// considering the same match & same turn, asserts that the two scores computed by different umpires
    /// (each one recruited for each player) are not in contradiction to each other
    fn assert_scores(player_1_umpire_score: MatchScore, player_2_umpire_score: MatchScore) {
        assert_eq!(player_2_umpire_score.opponent, player_1_umpire_score.oneself,  "Player's score mismatch between the two Umpires");
        assert_eq!(player_2_umpire_score.oneself,  player_1_umpire_score.opponent, "Opponent's score mismatch between the two Umpires");
    }

    fn next_turn_player(player: Players) -> Players {
        match player {
            Players::Ourself => Players::Opponent,
            Players::Opponent => Players::Ourself,
        }
    }

}