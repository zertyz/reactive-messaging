//! Models used in the ping-pong game [super::ping_pong_logic]

use serde::{Serialize, Deserialize};


/// Events that may happen on a ping-pong match
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum PingPongEvent {
    /// The ball hit the opponent's field without any hard faults, causing the turn to flip
    /// --requiring the opponent to react to it in order not to lose the score opportunity
    TurnFlip { player_action: PlayerAction, resulting_event: TurnFlipEvents },
    /// A hard fault occurred due to the player's reaction when hitting the ball
    /// -- the score opportunity was won by the opponent and, which must service the next ball
    /// (if the game is not over yet due to having reached the score limit)
    HardFault { player_action: PlayerAction, resulting_fault_event: FaultEvents },
    /// A soft fault happened -- which may cause the service to be attempted again or simply ignored during the rally (a "Let" event)
    SoftFault { player_action: PlayerAction, resulting_fault_event: FaultEvents },
    /// Indicates that a score was won by the given player
    Score {
        point_winning_player: Players,
        last_player_action:   PlayerAction,
        last_fault:           FaultEvents,
    },
    /// Indicates that the match is over
    GameOver(GameOverStates),
}

/// Events describing the result of the previous `PlayerAction`, continuing in the "rally" state
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum TurnFlipEvents {
    /// the player shot the ball, causing it to hit the opponent's field (without touching the net)
    SuccessfulService,
    SoftFaultService,
    /// the player rebated the ball, causing it to hit the opponent's field (a net touch is allowed)
    SuccessfulRebate,
}

/// Contains details of the action taken by a player during his turn in the game
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct PlayerAction {
    /// Details of the ball hitting intention
    pub lucky_number: f32,
}

/// Events describing a fault in the player's turn due to his/her (not so skilled) actions.\
/// The player turn starts when the ball touch his field (or when he hits the ball, when servicing) and ends
/// when the ball touches the opponent's field or a hard fault is committed.
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum FaultEvents {
    /// occurs when the player rebates the ball before it performed a kick on his/her field -- the player should either have waited for the kick or let the ball go out
    /// (note that this event also accounts for any "opponent's field invasion" that might happen)
    NoBounce,
    /// occurs when the ball either bounces twice into the player's field or when it touches it only once, then follows into the ground -- where the player was expected to rebate it just after the first kick
    NoRebate,
    /// occurs when the rabate (or service) attempt simply doesn't touch the ball
    Mishit,
    /// occurs when the ball touches the rebating player field before crossing the net
    PreBounce,
    /// occurs when the rebated (or serviced) ball touches the net, but is not contained by it, and hits the opponent's field
    NetTouch,
    /// occurs when the rebated (or serviced) ball is contained by the net and doesn't hit the opponent's field
    NetBlock,
    /// occurs when the rebated (or serviced) ball doesn't touch the opponent's field
    BallOut,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum GameStates {
    /// When servicing the ball, up to 3 attempts are allowed for the following faults: `TouchedTheNet` & `MissedTheBall`.\
    /// From the 3rd attempt on, the mentioned "soft faults" will be considered "hard faults" and the score will be computed to the opponent.
    WaitingForService {
        attempt: u32,
    },
    /// The game is in the "rally" phase, which starts when the serviced ball hits the opponent's field and in which a score point is under dispute.\
    /// When rebating, `ToucedTheNet` is no longer considered a fault (nor light nor hard)
    Rally,
    GameOver(GameOverStates),
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum GameOverStates {
    /// The score limit was reached and the game ended gracefully
    GracefullyEnded {
        final_score:        MatchScore,
        last_player_action: PlayerAction,
        last_fault:         FaultEvents,
    },
    /// A rule was not honored, causing the game to be aborted due to the given (human readable) reason
    GameCancelled {
        partial_score:           MatchScore,
        broken_rule_description: String,
    },
}

/// The configuration used for our Ping-Pong matches
#[derive(Debug, PartialEq, Clone, Copy,  Serialize, Deserialize)]
pub struct MatchConfig {
    /// ping-pong matches end when this score is reached
    pub score_limit: u32,
    /// if greater than 0, specifies that the given timeout must be enforced by the server, when on the rally phase of the ping-pong match
    pub rally_timeout_millis: u32,
    /// the probability that the player will prevent his turn from correctly starting by hitting the ball before it bounces on his field after the opponent had rebated it
    pub no_bounce_probability: f32,
    /// the probability of the player not hitting the ball after it bounces on his field after the opponent had rebated it
    pub no_rebate_probability: f32,
    /// the probability of the player not being able to hit the ball -- either when servicing it or when rebating it
    pub mishit_probability: f32,
    /// the probability of the ball touching the player field before crossing the border
    pub pre_bounce_probability: f32,
    /// the probability of the ball touching the net (but not being prevented from continuing on) -- this is a service fault, but does not affect the rally
    pub net_touch_probability: f32,
    /// the probability of the ball being stopped by the net (or being deflected out of the field, without crossing the border)
    pub net_block_probability: f32,
    /// the probability of the ball not hitting the opponent field, after being rebated or serviced by a player
    pub ball_out_probability: f32,
}

/// The result of a match -- either ongoing or finished
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct MatchScore {
    pub limit:    u32,
    pub oneself:  u32,
    pub opponent: u32,
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum Players {
    OneSelf,
    Opponent,
}