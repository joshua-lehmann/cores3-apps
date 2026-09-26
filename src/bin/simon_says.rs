//! A colour-and-sound memory game for the CoreS3.
//!
//! Watch the pads light up and listen to their tones, then repeat the pattern.
//! Each completed round adds one more colour.

#![no_std]
#![no_main]

use core::fmt::Write as _;

use arrayvec::{ArrayString, ArrayVec};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::{Point, Size},
    primitives::Rectangle,
};
use serde::{Deserialize, Serialize};
use simon_sound::{
    Board,
    capabilities::{
        audio::{self, Speaker},
        display::{Display, SCREEN, SIZE},
        network::{DeviceId, MAX_PEERS, Message, Network},
        touch::{Touch, TouchEvent},
    },
    synth::{SineWave, midi_note_hz},
    ui::{Canvas, common, theme},
};

esp_bootloader_esp_idf::esp_app_desc!();

/// The game stores a fixed sequence so it needs no heap allocation.
const MAX_SEQUENCE: usize = 32;
/// Maximum number of boards in one multiplayer game, including the host.
const MAX_PLAYERS: usize = 4;
/// Remote players use seats two through four; seat one belongs to the host.
const MAX_GUESTS: usize = MAX_PLAYERS - 1;
/// Long enough for a clear note without making the demonstration drag.
const TONE_DURATION_MS: usize = 300;
/// Frames of one tone at the speaker's sample rate.
const TONE_FRAMES: usize = audio::SAMPLE_RATE_HZ as usize * TONE_DURATION_MS / 1_000;
/// Keep generated audio chunks small enough for the task stack.
const AUDIO_CHUNK_FRAMES: usize = 128;
/// Bound synthesis work per loop while keeping audio ahead of the display.
const AUDIO_FEED_FRAMES_PER_TICK: usize = 2_048;
/// Tone volume, from silence (0.0) to maximum (1.0).
const TONE_VOLUME: f32 = 0.10;
/// Fifteen-millisecond fade at each end avoids a clipped sounding note.
const TONE_RAMP_FRAMES: usize = audio::SAMPLE_RATE_HZ as usize * 15 / 1_000;
/// How long a pad stays bright while its tone plays.
const FLASH_MS: u64 = 340;
/// The quiet space between colours in the sequence.
const GAP_MS: u64 = 170;
/// Room for the player to get ready before the first colour.
const FIRST_FLASH_DELAY_MS: u64 = 450;
/// Pause before handing over a turn or showing the next, longer round.
const NEXT_ROUND_DELAY_MS: u64 = 700;
/// A little feedback flash after a player taps a pad.
const INPUT_FLASH_MS: u64 = TONE_DURATION_MS as u64;
/// How long to wait before retrying a network message that needs a reply.
const NETWORK_RETRY_MS: u64 = 300;
/// Redraw cadence for the victory confetti.
const CONFETTI_FRAME_MS: u64 = 60;
/// Number of falling confetti pieces on the victory screen.
const CONFETTI_COUNT: usize = 32;
/// Available lobby entries shown on one page.
const LOBBY_ROWS_PER_PAGE: usize = 5;
/// A lobby advertisement expires if no fresh packet arrives within this time.
const LOBBY_OFFER_TTL_MS: u64 = 1_200;

/// The strip above the four colour pads.
const HEADER: Rectangle = Rectangle::new(Point::zero(), Size::new(SIZE.width, 76));
/// The button used to start or restart a single-player game.
const SOLO_BUTTON: Rectangle = Rectangle::new(Point::new(222, 10), Size::new(40, 56));
/// The button used to search for or join a match of up to four players.
const DUEL_BUTTON: Rectangle = Rectangle::new(Point::new(270, 10), Size::new(40, 56));
/// The lobby's exit/cancel button.
const LOBBY_EXIT_BUTTON: Rectangle = Rectangle::new(Point::new(258, 8), Size::new(54, 34));
/// Start an open lobby after at least one guest has joined.
const LOBBY_START_BUTTON: Rectangle = Rectangle::new(Point::new(190, 8), Size::new(60, 34));
/// Page navigation at the bottom of the lobby list.
const LOBBY_PREVIOUS_BUTTON: Rectangle = Rectangle::new(Point::new(8, 211), Size::new(72, 23));
const LOBBY_NEXT_BUTTON: Rectangle = Rectangle::new(Point::new(240, 211), Size::new(72, 23));

/// One of the four colours in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Color {
    /// Red pad and lowest tone.
    Red,
    /// Green pad and second-lowest tone.
    Green,
    /// Blue pad and second-highest tone.
    Blue,
    /// Yellow pad and highest tone.
    Yellow,
}

/// A player's seat in a multiplayer match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Player {
    /// The board that started the duel.
    One,
    /// The board that joined the invitation.
    Two,
    /// The third board in the match.
    Three,
    /// The fourth board in the match.
    Four,
}

impl Player {
    /// Every possible seat in a four-player match.
    const ALL: [Self; MAX_PLAYERS] = [Self::One, Self::Two, Self::Three, Self::Four];

    /// Index in scores, guest-device storage, and seat masks.
    const fn index(self) -> usize {
        match self {
            Self::One => 0,
            Self::Two => 1,
            Self::Three => 2,
            Self::Four => 3,
        }
    }

    /// The index of this player's score in the score array.
    const fn score_index(self) -> usize {
        self.index()
    }

    /// This seat's bit in an active-player mask.
    const fn bit(self) -> u8 {
        1 << self.index()
    }

    /// Convert a zero-based seat index to a player.
    const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::One),
            1 => Some(Self::Two),
            2 => Some(Self::Three),
            3 => Some(Self::Four),
            _ => None,
        }
    }

    /// The visible seat number.
    const fn number(self) -> u8 {
        self.index() as u8 + 1
    }
}

/// The match stage shared over ESP-NOW.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum WirePhase {
    /// The host is still accepting players.
    Lobby,
    /// Both boards show the current pattern.
    Showing,
    /// One player is repeating the pattern.
    Listening,
    /// A correct round ended and the sequence is about to grow.
    NextRound,
    /// One player's mistake ended the match.
    GameOver,
}

/// A complete, repeatable snapshot of the current match state.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct WireState {
    /// Increases whenever the host changes the match state.
    revision: u32,
    /// The complete sequence, so a missed radio packet can be recovered.
    sequence: [Color; MAX_SEQUENCE],
    /// Number of valid colours in `sequence`.
    sequence_len: u8,
    /// Position the current player is repeating.
    input_index: u8,
    /// Player whose turn it is.
    turn: Player,
    /// Shared game stage.
    phase: WirePhase,
    /// Rounds completed by each of the four seats.
    scores: [u8; MAX_PLAYERS],
    /// Number of boards that joined this match.
    player_count: u8,
    /// Players still in the match.
    alive_mask: u8,
    /// Players who have not yet repeated the current round.
    turns_remaining: u8,
    /// First player for the current round.
    round_start: Player,
    /// The player who made the losing mistake, if the duel has ended.
    loser: Option<Player>,
    /// The winner, once the match is over.
    winner: Option<Player>,
}

/// One packet used by the Simon Says multiplayer protocol.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct GamePacket {
    /// Identifies one match and rejects late messages from earlier games.
    session: u32,
    /// What this packet asks the other board to do.
    body: PacketBody,
}

/// A message exchanged by the two boards.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum PacketBody {
    /// Advertise an open lobby and its current occupancy.
    Lobby { player_count: u8 },
    /// Accept the host's invitation.
    Join,
    /// Send the complete current state from the host to the guest.
    Sync(WireState),
    /// Send one guest pad press to the authoritative host.
    Move {
        /// Host state revision the move was based on.
        revision: u32,
        /// Sequence length for the current round.
        round: u8,
        /// Position of this move in the sequence.
        index: u8,
        /// Pad the guest pressed.
        color: Color,
    },
    /// Confirm that a state snapshot arrived.
    Ack { revision: u32 },
    /// Tell the host that this player left the match.
    Forfeit,
    /// Assign a seat and send the current state to one joining device.
    Joined { player: Player, state: WireState },
    /// Tell a guest that this lobby already has an opponent.
    Busy,
    /// Cancel a join while the guest is still connecting.
    CancelJoin,
    /// The host is leaving this match.
    Abort,
}

impl Message for GamePacket {
    /// Separates Simon Says packets from other applications on the same radio.
    const NAME: &'static str = "simon-sound.simon-says";
}

/// Which board controls the current networked game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkRole {
    /// No multiplayer game is active on this board.
    None,
    /// This board started the game and owns the authoritative state.
    Host,
    /// This board joined a game and sends its moves to the host.
    Guest,
}

impl Color {
    /// The colours in display order, with a distinct tone for each.
    const ALL: [Self; 4] = [Self::Red, Self::Green, Self::Blue, Self::Yellow];

    /// The touch target and drawing area for this colour.
    const fn area(self) -> Rectangle {
        match self {
            Self::Red => Rectangle::new(Point::new(8, 84), Size::new(148, 70)),
            Self::Green => Rectangle::new(Point::new(164, 84), Size::new(148, 70)),
            Self::Blue => Rectangle::new(Point::new(8, 162), Size::new(148, 70)),
            Self::Yellow => Rectangle::new(Point::new(164, 162), Size::new(148, 70)),
        }
    }

    /// The pad under `point`, if it lies inside a pad.
    fn at(point: Point) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|color| color.area().contains(point))
    }

    /// The printed label on the pad.
    const fn name(self) -> &'static str {
        match self {
            Self::Red => "RED",
            Self::Green => "GREEN",
            Self::Blue => "BLUE",
            Self::Yellow => "YELLOW",
        }
    }

    /// Distinct lower notes that stay above the speaker's distortion range.
    const fn midi_note(self) -> u8 {
        match self {
            Self::Red => 75,
            Self::Green => 77,
            Self::Blue => 79,
            Self::Yellow => 81,
        }
    }

    /// The matching tone in hertz.
    fn frequency_hz(self) -> f32 {
        midi_note_hz(f32::from(self.midi_note()))
    }

    /// The pad colour, dim normally and bright during a flash.
    const fn fill(self, lit: bool) -> Rgb565 {
        match (self, lit) {
            (Self::Red, false) => Rgb565::new(15, 0, 0),
            (Self::Red, true) => Rgb565::new(31, 0, 0),
            (Self::Green, false) => Rgb565::new(0, 30, 0),
            (Self::Green, true) => Rgb565::new(0, 63, 0),
            (Self::Blue, false) => Rgb565::new(0, 0, 15),
            (Self::Blue, true) => Rgb565::new(0, 0, 31),
            (Self::Yellow, false) => Rgb565::new(15, 30, 0),
            (Self::Yellow, true) => Rgb565::new(31, 63, 0),
        }
    }

    /// A label colour that stays readable on both dim and bright pads.
    const fn label(self, lit: bool) -> Rgb565 {
        match self {
            Self::Green if lit => theme::CHARCOAL,
            Self::Yellow => theme::CHARCOAL,
            _ => theme::WHITE,
        }
    }
}

/// The visible phase of a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Waiting for the player to begin.
    Ready,
    /// This board is advertising an open multiplayer lobby.
    Lobby,
    /// This board accepted an invitation and is waiting for the host's state.
    Joining,
    /// This board joined, but the host has not started the game yet.
    WaitingStart,
    /// Showing the current pattern.
    Showing,
    /// Waiting for the player to repeat the pattern.
    Listening,
    /// Waiting while the other player repeats the pattern.
    WatchingOpponent,
    /// The guest sent a move and is waiting for the host to confirm it.
    AwaitingAck,
    /// Brief pause before the next player's turn or the next colour.
    NextRound,
    /// The player selected a colour out of order.
    GameOver,
    /// The player completed the full sequence.
    Won,
}

/// Whether the current sequence step is in its quiet gap or bright flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShowPhase {
    /// The next colour has not lit yet.
    Gap,
    /// A colour is currently lit.
    Flash,
    /// The local display finished playback and awaits the host's next state.
    Finished,
}

/// The part of the screen that can change between frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Picture {
    /// Current game phase.
    mode: Mode,
    /// Number of colours in the sequence being played.
    sequence_len: usize,
    /// Best fully completed round in this session.
    best: usize,
    /// Next colour the player needs to repeat.
    input_index: usize,
    /// Pad currently lit, if any.
    lit: Option<Color>,
    /// Whether the screen belongs to a multiplayer game.
    network_role: NetworkRole,
    /// This board's seat in a match.
    player: Option<Player>,
    /// The active player's seat.
    turn: Player,
    /// Completed rounds for all four seats.
    scores: [u8; MAX_PLAYERS],
    /// The most recent player eliminated from the match.
    loser: Option<Player>,
    /// The player who won the match.
    winner: Option<Player>,
    /// The selected or connected remote device.
    opponent: Option<DeviceId>,
    /// Number of players who joined this match.
    player_count: u8,
    /// Players who still need a turn before the next colour is added.
    turns_remaining: u8,
    /// Other boards currently advertising open lobbies.
    lobbies: [Option<LobbyListing>; MAX_PEERS],
    /// Page of lobby entries currently shown.
    lobby_page: usize,
    /// Frame of the victory confetti animation.
    confetti_frame: u16,
}

/// One board advertising a game that can be joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LobbyListing {
    /// Device to send the join request to.
    device: DeviceId,
    /// Session advertised by that board.
    session: u32,
    /// Current number of seats filled, including the host.
    player_count: u8,
}

/// An advertised lobby and the last time it was heard.
#[derive(Clone, Copy)]
struct LobbyOffer {
    /// Lobby identity shown in the list and used to join it.
    listing: LobbyListing,
    /// Milliseconds since boot when the latest advertisement arrived.
    last_seen_ms: u64,
}

/// The application owns its hardware handles and game state.
struct SimonSaysApp {
    /// The LCD that shows the game.
    display: Display,
    /// Queued presses and releases from the touch panel.
    touch: Touch,
    /// Nearby boards and typed ESP-NOW messages.
    network: Network,
    /// The board's speaker.
    speaker: Speaker,
    /// Generates the current colour tone a little at a time.
    tone: TonePlayer,
    /// Screen-sized drawing buffer, allocated once in PSRAM.
    canvas: Canvas,
    /// Sequence storage, fixed to avoid heap allocation.
    sequence: [Color; MAX_SEQUENCE],
    /// Number of valid colours in `sequence`.
    sequence_len: usize,
    /// Next colour to show during playback.
    playback_index: usize,
    /// Position in the sequence the player is currently repeating.
    input_index: usize,
    /// Highest round completed during this run.
    best: usize,
    /// Pseudo-random sequence generator.
    random: SequenceRandom,
    /// Current network role for a multiplayer game.
    network_role: NetworkRole,
    /// This board's seat in a match.
    player: Option<Player>,
    /// The selected opponent, or the sender of an invitation awaiting a reply.
    opponent: Option<DeviceId>,
    /// Remote devices assigned to seats two through four while hosting.
    guest_devices: [Option<DeviceId>; MAX_GUESTS],
    /// This board's lobby identity, retained while joining another lobby.
    lobby_session: u32,
    /// Recently heard open lobbies, keyed by device id.
    lobby_offers: [Option<LobbyOffer>; MAX_PEERS],
    /// Page currently shown in the lobby list.
    lobby_page: usize,
    /// Session id used to ignore stale match messages.
    session: u32,
    /// Authoritative host revision for the active match.
    revision: u32,
    /// The state revision the host is waiting for the guest to acknowledge.
    pending_state_ack: Option<u32>,
    /// Guests still expected to acknowledge the current host snapshot.
    pending_state_ack_mask: u8,
    /// The state revision the guest still needs to acknowledge.
    pending_ack: Option<u32>,
    /// Whether the guest is still trying to join.
    pending_join: bool,
    /// A guest move waiting for the host's updated state.
    pending_move: Option<(u32, u8, u8, Color)>,
    /// Next time a queued network action should be retried.
    next_network_send: Instant,
    /// The player whose turn it is.
    turn: Player,
    /// First player for the current round.
    round_start: Player,
    /// Players who have not yet repeated the current round.
    turns_remaining: u8,
    /// Number of boards that joined this match.
    player_count: u8,
    /// Players still in the match.
    alive_mask: u8,
    /// Rounds completed by each seat.
    scores: [u8; MAX_PLAYERS],
    /// The most recent player eliminated from the match.
    loser: Option<Player>,
    /// The player who won the match.
    winner: Option<Player>,
    /// Frame of the victory confetti animation.
    confetti_frame: u16,
    /// Time when the next victory frame should be drawn.
    next_confetti_frame: Instant,
    /// Current phase of the game.
    mode: Mode,
    /// Current gap/flash part of playback.
    show_phase: ShowPhase,
    /// Guest phase to enter after its local copy of the pattern finishes.
    after_playback: Option<Mode>,
    /// When the current timed phase ends.
    transition_at: Instant,
    /// Pad that is currently bright.
    lit: Option<Color>,
    /// When a player-input flash should end.
    input_flash_until: Instant,
    /// Last picture sent to the panel, for skipping redundant redraws.
    drawn: Option<Picture>,
}

/// The entry point: read touch, advance the game, feed audio and draw changes.
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let Board {
        display,
        touch,
        speaker,
        network,
        ..
    } = Board::init();

    let mut app = SimonSaysApp {
        display,
        touch,
        network,
        speaker,
        tone: TonePlayer::default(),
        canvas: Canvas::new(SIZE),
        sequence: [Color::Red; MAX_SEQUENCE],
        sequence_len: 0,
        playback_index: 0,
        input_index: 0,
        best: 0,
        random: SequenceRandom::new(1),
        network_role: NetworkRole::None,
        player: None,
        opponent: None,
        lobby_session: 0,
        lobby_offers: [None; MAX_PEERS],
        lobby_page: 0,
        session: 0,
        revision: 0,
        pending_state_ack: None,
        pending_state_ack_mask: 0,
        pending_ack: None,
        pending_join: false,
        pending_move: None,
        next_network_send: Instant::now(),
        turn: Player::One,
        round_start: Player::One,
        turns_remaining: Player::One.bit(),
        player_count: 1,
        alive_mask: Player::One.bit(),
        scores: [0; MAX_PLAYERS],
        loser: None,
        winner: None,
        guest_devices: [None; MAX_GUESTS],
        confetti_frame: 0,
        next_confetti_frame: Instant::now(),
        mode: Mode::Ready,
        show_phase: ShowPhase::Gap,
        after_playback: None,
        transition_at: Instant::now(),
        lit: None,
        input_flash_until: Instant::now(),
        drawn: None,
    };

    loop {
        let now = Instant::now();
        app.handle_network(now);
        app.handle_touch(now);
        app.update(now);
        app.send_network(now);
        app.feed_speaker();
        app.draw_if_changed();
        Timer::after(Duration::from_millis(5)).await;
    }
}

impl SimonSaysApp {
    /// Read every waiting event so the touch queue stays clear.
    fn handle_touch(&mut self, now: Instant) {
        while let Some(event) = self.touch.next_event() {
            if let TouchEvent::Pressed(point) = event {
                if self.mode == Mode::Lobby {
                    self.handle_lobby_touch(point, now);
                } else if matches!(self.mode, Mode::Joining | Mode::WaitingStart) {
                    if LOBBY_EXIT_BUTTON.contains(point) {
                        self.cancel_join(now);
                    }
                } else if SOLO_BUTTON.contains(point) {
                    self.start_solo(now, point);
                } else if DUEL_BUTTON.contains(point) {
                    self.start_duel(now, point);
                } else if let Some(color) = Color::at(point) {
                    match self.mode {
                        Mode::Ready | Mode::GameOver | Mode::Won => self.start_solo(now, point),
                        Mode::Listening => self.accept_input(color, now),
                        Mode::Showing
                        | Mode::Lobby
                        | Mode::Joining
                        | Mode::WaitingStart
                        | Mode::WatchingOpponent
                        | Mode::AwaitingAck
                        | Mode::NextRound => {}
                    }
                }
            }
        }
    }

    /// Start a new solo game and clear any pending duel state.
    fn start_solo(&mut self, now: Instant, point: Point) {
        self.leave_duel();
        self.reset_duel_state();
        self.lobby_session = 0;
        self.random = SequenceRandom::new(seed_from(now, point));
        self.sequence_len = 0;
        self.append_color();
        self.loser = None;
        self.winner = None;
        self.begin_playback(now, FIRST_FLASH_DELAY_MS);
    }

    /// Open a lobby and advertise it to nearby Simon Says boards.
    fn start_duel(&mut self, now: Instant, point: Point) {
        self.leave_duel();
        self.reset_duel_state();
        let seed = seed_from(now, point);
        self.random = SequenceRandom::new(seed);
        self.session = seed;
        self.lobby_session = seed;
        self.network_role = NetworkRole::Host;
        self.player = Some(Player::One);
        self.player_count = 1;
        self.alive_mask = Player::One.bit();
        self.lobby_offers = [None; MAX_PEERS];
        self.lobby_page = 0;
        self.mode = Mode::Lobby;
        self.next_network_send = now;
    }

    /// Join the selected lobby and wait for its host to send the game state.
    fn join_lobby(&mut self, listing: LobbyListing, now: Instant) {
        if self.network_role == NetworkRole::Host && self.mode == Mode::Lobby {
            for device in self.guest_devices.iter().flatten().copied() {
                let _ = self.network.send_to(
                    device,
                    &GamePacket {
                        session: self.session,
                        body: PacketBody::CancelJoin,
                    },
                );
            }
        }
        self.reset_duel_state();
        self.opponent = Some(listing.device);
        self.session = listing.session;
        self.network_role = NetworkRole::Guest;
        self.pending_join = true;
        self.mode = Mode::Joining;
        self.next_network_send = now;
    }

    /// Stop advertising this lobby and return to the game screen.
    fn close_lobby(&mut self) {
        for device in self.guest_devices.iter().flatten().copied() {
            let _ = self.network.send_to(
                device,
                &GamePacket {
                    session: self.session,
                    body: PacketBody::CancelJoin,
                },
            );
        }
        self.reset_duel_state();
        self.lobby_session = 0;
        self.lobby_offers = [None; MAX_PEERS];
        self.lobby_page = 0;
        self.mode = Mode::Ready;
    }

    /// Cancel a pending join and restore this board's own open lobby.
    fn cancel_join(&mut self, now: Instant) {
        if let Some(opponent) = self.opponent {
            let _ = self.network.send_to(
                opponent,
                &GamePacket {
                    session: self.session,
                    body: PacketBody::CancelJoin,
                },
            );
        }
        self.return_to_lobby(now);
    }

    /// Resume this board's lobby after a declined or cancelled join.
    fn return_to_lobby(&mut self, now: Instant) {
        let lobby_session = self.lobby_session;
        self.reset_duel_state();
        self.session = lobby_session;
        self.network_role = NetworkRole::Host;
        self.player = Some(Player::One);
        self.player_count = 1;
        self.alive_mask = Player::One.bit();
        self.turns_remaining = Player::One.bit();
        self.mode = Mode::Lobby;
        self.next_network_send = now;
    }

    /// Handle taps on the dedicated lobby screen.
    fn handle_lobby_touch(&mut self, point: Point, now: Instant) {
        if LOBBY_EXIT_BUTTON.contains(point) {
            self.close_lobby();
            return;
        }
        if LOBBY_START_BUTTON.contains(point) && self.player_count >= 2 {
            self.begin_match(now);
            return;
        }

        let page_count = self.lobby_page_count();
        if LOBBY_PREVIOUS_BUTTON.contains(point) && self.lobby_page > 0 {
            self.lobby_page -= 1;
            return;
        }
        if LOBBY_NEXT_BUTTON.contains(point) && self.lobby_page + 1 < page_count {
            self.lobby_page += 1;
            return;
        }

        for row in 0..LOBBY_ROWS_PER_PAGE {
            let area = Rectangle::new(
                Point::new(8, 75 + (row as i32 * 27)),
                Size::new(SIZE.width - 16, 26),
            );
            if !area.contains(point) {
                continue;
            }
            let index = self.lobby_page * LOBBY_ROWS_PER_PAGE + row;
            if let Some(offer) = self.lobby_offers.get(index).copied().flatten() {
                self.join_lobby(offer.listing, now);
            }
            return;
        }
    }

    /// Count active lobby offers and keep at least one empty list page.
    fn lobby_page_count(&self) -> usize {
        self.lobby_offers
            .iter()
            .filter(|offer| offer.is_some())
            .count()
            .div_ceil(LOBBY_ROWS_PER_PAGE)
            .max(1)
    }

    /// Remember an open lobby, refreshing its age if this device is already listed.
    fn remember_lobby(&mut self, device: DeviceId, session: u32, player_count: u8, now: Instant) {
        let listing = LobbyListing {
            device,
            session,
            player_count,
        };
        let now_ms = now.as_millis();
        if let Some(offer) = self
            .lobby_offers
            .iter_mut()
            .flatten()
            .find(|offer| offer.listing.device == device)
        {
            offer.listing = listing;
            offer.last_seen_ms = now_ms;
            return;
        }

        self.compact_lobby_offers();
        let slot = self
            .lobby_offers
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                self.lobby_offers
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, offer)| offer.map_or(u64::MAX, |offer| offer.last_seen_ms))
                    .map_or(0, |(index, _)| index)
            });
        self.lobby_offers[slot] = Some(LobbyOffer {
            listing,
            last_seen_ms: now_ms,
        });
    }

    /// Remove an entry when its lobby fills or stops accepting players.
    fn remove_lobby(&mut self, device: DeviceId) {
        for offer in &mut self.lobby_offers {
            if offer.is_some_and(|offer| offer.listing.device == device) {
                *offer = None;
            }
        }
        self.compact_lobby_offers();
        self.lobby_page = self.lobby_page.min(self.lobby_page_count() - 1);
    }

    /// Remove expired advertisements and close any gaps in the list.
    fn expire_lobby_offers(&mut self, now: Instant) {
        let now_ms = now.as_millis();
        for offer in &mut self.lobby_offers {
            if offer
                .is_some_and(|offer| now_ms.saturating_sub(offer.last_seen_ms) > LOBBY_OFFER_TTL_MS)
            {
                *offer = None;
            }
        }
        self.compact_lobby_offers();
        self.lobby_page = self.lobby_page.min(self.lobby_page_count() - 1);
    }

    /// Keep the visible list packed from its first row.
    fn compact_lobby_offers(&mut self) {
        let mut next = 0;
        for read in 0..MAX_PEERS {
            if let Some(offer) = self.lobby_offers[read] {
                self.lobby_offers[next] = Some(offer);
                next += 1;
            }
        }
        self.lobby_offers[next..].fill(None);
    }

    /// Notify the current host or guests before leaving or replacing a match.
    fn leave_duel(&mut self) {
        let packet = match self.network_role {
            NetworkRole::Host => Some(GamePacket {
                session: self.session,
                body: PacketBody::Abort,
            }),
            NetworkRole::Guest => Some(GamePacket {
                session: self.session,
                body: PacketBody::Forfeit,
            }),
            NetworkRole::None => None,
        };
        if let Some(packet) = packet {
            if self.network_role == NetworkRole::Host {
                let _ = self.network.broadcast(&packet);
            } else if let Some(opponent) = self.opponent {
                let _ = self.network.send_to(opponent, &packet);
            }
        }
    }

    /// Clear the match fields while keeping the board handles and solo record.
    fn reset_duel_state(&mut self) {
        self.tone.stop(&mut self.speaker);
        self.network_role = NetworkRole::None;
        self.player = None;
        self.opponent = None;
        self.guest_devices = [None; MAX_GUESTS];
        self.session = 0;
        self.revision = 0;
        self.pending_state_ack = None;
        self.pending_state_ack_mask = 0;
        self.pending_ack = None;
        self.pending_join = false;
        self.pending_move = None;
        self.turn = Player::One;
        self.round_start = Player::One;
        self.turns_remaining = Player::One.bit();
        self.player_count = 1;
        self.alive_mask = Player::One.bit();
        self.scores = [0; MAX_PLAYERS];
        self.loser = None;
        self.winner = None;
        self.sequence_len = 0;
        self.input_index = 0;
        self.lit = None;
    }

    /// Find the seat assigned to a remote board.
    fn player_for_device(&self, device: DeviceId) -> Option<Player> {
        self.guest_devices
            .iter()
            .position(|assigned| *assigned == Some(device))
            .and_then(|index| Player::from_index(index + 1))
    }

    /// Assign the first open guest seat, or return its existing seat.
    fn assign_guest(&mut self, device: DeviceId) -> Option<Player> {
        if let Some(player) = self.player_for_device(device) {
            return Some(player);
        }
        if usize::from(self.player_count) >= MAX_PLAYERS {
            return None;
        }

        let slot = self.guest_devices.iter().position(Option::is_none)?;
        let player = Player::from_index(slot + 1)?;
        self.guest_devices[slot] = Some(device);
        self.player_count += 1;
        self.alive_mask |= player.bit();
        self.turns_remaining |= player.bit();
        Some(player)
    }

    /// Remove a remote player who leaves while the lobby is still open.
    fn remove_guest(&mut self, device: DeviceId) -> bool {
        let Some(player) = self.player_for_device(device) else {
            return false;
        };
        self.guest_devices[player.index() - 1] = None;
        self.player_count = self.player_count.saturating_sub(1);
        self.alive_mask &= !player.bit();
        self.turns_remaining &= !player.bit();
        true
    }

    /// Find the next eligible seat after `player`, wrapping around the table.
    fn next_player_after(&self, player: Player, eligible: u8) -> Option<Player> {
        for offset in 1..=MAX_PLAYERS {
            let index = (player.index() + offset) % MAX_PLAYERS;
            let Some(candidate) = Player::from_index(index) else {
                continue;
            };
            if eligible & candidate.bit() != 0 {
                return Some(candidate);
            }
        }
        None
    }

    /// Start a match once the host has at least one guest.
    fn begin_match(&mut self, now: Instant) {
        if self.network_role != NetworkRole::Host
            || self.mode != Mode::Lobby
            || self.player_count < 2
        {
            return;
        }
        self.sequence_len = 0;
        self.append_color();
        self.turn = self
            .next_player_after(Player::Four, self.alive_mask)
            .unwrap_or(Player::One);
        self.round_start = self.turn;
        self.turns_remaining = self.alive_mask;
        self.input_index = 0;
        self.loser = None;
        self.winner = None;
        self.begin_playback(now, FIRST_FLASH_DELAY_MS);
        self.state_changed(now);
    }

    /// End a multiplayer match and leave only its winner active in the snapshot.
    fn finish_match(&mut self, winner: Player, loser: Option<Player>) {
        self.winner = Some(winner);
        self.loser = loser;
        self.alive_mask = winner.bit();
        self.turns_remaining = 0;
        self.mode = Mode::GameOver;
    }

    /// Pick the highest-scoring active player; seat order breaks a tie.
    fn leading_player(&self) -> Player {
        let mut winner = Player::ALL
            .into_iter()
            .find(|player| self.alive_mask & player.bit() != 0)
            .unwrap_or(Player::One);
        let mut best_score = self.scores[winner.score_index()];
        for player in Player::ALL {
            if self.alive_mask & player.bit() != 0 && self.scores[player.score_index()] > best_score
            {
                winner = player;
                best_score = self.scores[player.score_index()];
            }
        }
        winner
    }

    /// Read every message and apply only packets for the current duel.
    fn handle_network(&mut self, now: Instant) {
        while let Some(message) = self.network.next_message() {
            let sender = message.sender;
            if let Ok(packet) = message.decode::<GamePacket>() {
                self.handle_packet(sender, packet, now);
            }
        }
    }

    /// Handle one validated Simon Says packet from a nearby board.
    fn handle_packet(&mut self, sender: DeviceId, packet: GamePacket, now: Instant) {
        match packet.body {
            PacketBody::Lobby { player_count }
                if self.network_role == NetworkRole::Host && self.mode == Mode::Lobby =>
            {
                if player_count < MAX_PLAYERS as u8 {
                    self.remember_lobby(sender, packet.session, player_count, now);
                } else {
                    self.remove_lobby(sender);
                }
            }
            PacketBody::Join
                if self.network_role == NetworkRole::Host && packet.session == self.session =>
            {
                let existing = self.player_for_device(sender);
                let player = if self.mode == Mode::Lobby {
                    self.assign_guest(sender)
                } else {
                    existing
                };
                if let Some(player) = player {
                    if existing.is_none() {
                        if usize::from(self.player_count) == MAX_PLAYERS {
                            self.begin_match(now);
                        } else {
                            self.state_changed(now);
                        }
                    }
                    let _ = self.network.send_to(
                        sender,
                        &GamePacket {
                            session: self.session,
                            body: PacketBody::Joined {
                                player,
                                state: self.wire_state(),
                            },
                        },
                    );
                } else {
                    let _ = self.network.send_to(
                        sender,
                        &GamePacket {
                            session: self.session,
                            body: PacketBody::Busy,
                        },
                    );
                }
            }
            PacketBody::Busy
                if self.network_role == NetworkRole::Guest
                    && matches!(self.mode, Mode::Joining | Mode::WaitingStart)
                    && packet.session == self.session
                    && self.opponent == Some(sender) =>
            {
                self.return_to_lobby(now);
            }
            PacketBody::CancelJoin
                if self.network_role == NetworkRole::Host && packet.session == self.session =>
            {
                if self.mode == Mode::Lobby {
                    if self.remove_guest(sender) {
                        self.state_changed(now);
                    }
                } else if let Some(player) = self.player_for_device(sender) {
                    self.eliminate_player(player, now);
                    self.state_changed(now);
                }
            }
            PacketBody::CancelJoin
                if self.network_role == NetworkRole::Guest
                    && matches!(self.mode, Mode::Joining | Mode::WaitingStart)
                    && packet.session == self.session
                    && self.opponent == Some(sender) =>
            {
                self.return_to_lobby(now);
            }
            PacketBody::Joined { player, state }
                if self.network_role == NetworkRole::Guest
                    && packet.session == self.session
                    && self.opponent == Some(sender) =>
            {
                self.player = Some(player);
                if state.revision > self.revision {
                    self.apply_sync(state, now);
                } else {
                    self.pending_ack = Some(self.revision);
                    self.next_network_send = now;
                    if self.mode == Mode::WatchingOpponent
                        && self.turn == player
                        && self.alive_mask & player.bit() != 0
                    {
                        self.mode = Mode::Listening;
                    }
                }
            }
            PacketBody::Sync(state)
                if self.network_role == NetworkRole::Guest
                    && packet.session == self.session
                    && self.opponent == Some(sender) =>
            {
                self.apply_sync(state, now);
            }
            PacketBody::Move {
                revision,
                round,
                index,
                color,
            } if self.network_role == NetworkRole::Host && packet.session == self.session => {
                let Some(player) = self.player_for_device(sender) else {
                    return;
                };
                if revision == self.revision
                    && round as usize == self.sequence_len
                    && index as usize == self.input_index
                    && self.turn == player
                    && self.turns_remaining & player.bit() != 0
                {
                    self.record_move(color, player, now);
                    self.state_changed(now);
                } else {
                    self.resend_state(now);
                }
            }
            PacketBody::Ack { revision }
                if self.network_role == NetworkRole::Host
                    && packet.session == self.session
                    && self.pending_state_ack == Some(revision)
                    && self.revision == revision =>
            {
                if let Some(player) = self.player_for_device(sender) {
                    self.pending_state_ack_mask &= !player.bit();
                    if self.pending_state_ack_mask == 0 {
                        self.pending_state_ack = None;
                    }
                }
            }
            PacketBody::Forfeit
                if self.network_role == NetworkRole::Host && packet.session == self.session =>
            {
                if let Some(player) = self.player_for_device(sender) {
                    self.eliminate_player(player, now);
                    self.state_changed(now);
                }
            }
            PacketBody::Abort
                if self.network_role == NetworkRole::Guest
                    && packet.session == self.session
                    && self.opponent == Some(sender) =>
            {
                self.reset_duel_state();
                self.mode = Mode::Ready;
            }
            _ => {}
        }
    }

    /// Apply a newer full host snapshot and acknowledge it for reliable recovery.
    fn apply_sync(&mut self, state: WireState, now: Instant) {
        let sequence_len = usize::from(state.sequence_len);
        let input_index = usize::from(state.input_index);
        let lobby_snapshot = state.phase == WirePhase::Lobby;
        if sequence_len > MAX_SEQUENCE
            || (!lobby_snapshot && sequence_len == 0)
            || input_index > sequence_len
            || usize::from(state.player_count) > MAX_PLAYERS
        {
            return;
        }

        self.pending_ack = Some(state.revision);
        self.next_network_send = now;
        if state.revision <= self.revision {
            return;
        }

        self.revision = state.revision;
        self.sequence = state.sequence;
        self.sequence_len = sequence_len;
        self.input_index = input_index;
        self.turn = state.turn;
        self.scores = state.scores;
        self.loser = state.loser;
        self.winner = state.winner;
        self.player_count = state.player_count;
        self.alive_mask = state.alive_mask;
        self.turns_remaining = state.turns_remaining;
        self.round_start = state.round_start;
        self.pending_join = false;
        self.pending_move = None;
        self.lit = None;

        match state.phase {
            WirePhase::Lobby => self.mode = Mode::WaitingStart,
            WirePhase::Showing => {
                let delay_ms = if self.mode == Mode::NextRound {
                    GAP_MS
                } else {
                    FIRST_FLASH_DELAY_MS
                };
                self.begin_playback(now, delay_ms);
            }
            WirePhase::Listening => {
                let next_mode = if self.player == Some(state.turn)
                    && state.alive_mask & state.turn.bit() != 0
                {
                    Mode::Listening
                } else {
                    Mode::WatchingOpponent
                };
                if self.network_role == NetworkRole::Guest
                    && self.mode == Mode::Showing
                    && self.show_phase != ShowPhase::Finished
                {
                    self.after_playback = Some(next_mode);
                } else {
                    self.mode = next_mode;
                }
            }
            WirePhase::NextRound => {
                self.mode = Mode::NextRound;
                self.transition_at = now + Duration::from_millis(NEXT_ROUND_DELAY_MS);
            }
            WirePhase::GameOver => self.mode = Mode::GameOver,
        }
    }

    /// Send one due lobby advertisement, join, move, acknowledgement or state snapshot.
    fn send_network(&mut self, now: Instant) {
        if now < self.next_network_send {
            return;
        }

        match self.network_role {
            NetworkRole::Host if self.mode == Mode::Lobby => {
                let lobby = GamePacket {
                    session: self.session,
                    body: PacketBody::Lobby {
                        player_count: self.player_count,
                    },
                };
                let _ = self.network.broadcast(&lobby);
                if self.pending_state_ack.is_some() {
                    let sync = GamePacket {
                        session: self.session,
                        body: PacketBody::Sync(self.wire_state()),
                    };
                    let _ = self.network.broadcast(&sync);
                }
                self.next_network_send = now + Duration::from_millis(NETWORK_RETRY_MS);
            }
            NetworkRole::Host if self.pending_state_ack.is_some() => {
                let packet = GamePacket {
                    session: self.session,
                    body: PacketBody::Sync(self.wire_state()),
                };
                let _ = self.network.broadcast(&packet);
                self.next_network_send = now + Duration::from_millis(NETWORK_RETRY_MS);
            }
            NetworkRole::Guest if self.pending_join => {
                self.queue_to_opponent(
                    GamePacket {
                        session: self.session,
                        body: PacketBody::Join,
                    },
                    now,
                );
            }
            NetworkRole::Guest if self.pending_ack.is_some() => {
                let revision = self.pending_ack.unwrap_or(self.revision);
                if self.queue_to_opponent(
                    GamePacket {
                        session: self.session,
                        body: PacketBody::Ack { revision },
                    },
                    now,
                ) {
                    self.pending_ack = None;
                }
            }
            NetworkRole::Guest if self.pending_move.is_some() => {
                let (revision, round, index, color) =
                    self.pending_move.unwrap_or((0, 0, 0, Color::Red));
                self.queue_to_opponent(
                    GamePacket {
                        session: self.session,
                        body: PacketBody::Move {
                            revision,
                            round,
                            index,
                            color,
                        },
                    },
                    now,
                );
            }
            _ => {}
        }
    }

    /// Queue one packet to the selected opponent and schedule a retry.
    fn queue_to_opponent(&mut self, packet: GamePacket, now: Instant) -> bool {
        self.next_network_send = now + Duration::from_millis(NETWORK_RETRY_MS);
        let Some(opponent) = self.opponent else {
            return false;
        };
        self.network.send_to(opponent, &packet).is_ok()
    }

    /// Retry the latest state for every player currently assigned to the match.
    fn resend_state(&mut self, now: Instant) {
        self.pending_state_ack_mask = self.connected_guest_mask();
        self.pending_state_ack = (self.pending_state_ack_mask != 0).then_some(self.revision);
        self.next_network_send = now;
    }

    /// The acknowledgment bits for all assigned guest devices.
    fn connected_guest_mask(&self) -> u8 {
        self.guest_devices
            .iter()
            .enumerate()
            .fold(0, |mask, (slot, device)| {
                if device.is_some() {
                    mask | (1 << (slot + 1))
                } else {
                    mask
                }
            })
    }

    /// Advance a timed phase. The host controls multiplayer transitions.
    fn update(&mut self, now: Instant) {
        if self.mode == Mode::Lobby {
            self.expire_lobby_offers(now);
        }

        if self.mode == Mode::Showing && now >= self.transition_at {
            match self.show_phase {
                ShowPhase::Gap => {
                    let color = self.sequence[self.playback_index];
                    self.lit = Some(color);
                    self.tone_start(color);
                    self.show_phase = ShowPhase::Flash;
                    self.transition_at = now + Duration::from_millis(FLASH_MS);
                }
                ShowPhase::Flash => {
                    self.lit = None;
                    self.playback_index += 1;
                    if self.playback_index == self.sequence_len {
                        self.show_phase = ShowPhase::Finished;
                        if self.network_role == NetworkRole::Guest {
                            if let Some(next_mode) = self.after_playback.take() {
                                self.mode = next_mode;
                            }
                        } else {
                            self.input_index = 0;
                            self.mode = if self.network_role == NetworkRole::Host
                                && self.turn != Player::One
                            {
                                Mode::WatchingOpponent
                            } else {
                                Mode::Listening
                            };
                            if self.network_role == NetworkRole::Host {
                                self.state_changed(now);
                            }
                        }
                    } else {
                        self.show_phase = ShowPhase::Gap;
                        self.transition_at = now + Duration::from_millis(GAP_MS);
                    }
                }
                ShowPhase::Finished => {}
            }
        } else if self.mode == Mode::NextRound
            && now >= self.transition_at
            && self.network_role != NetworkRole::Guest
        {
            if self.network_role == NetworkRole::None || self.turns_remaining == 0 {
                if self.sequence_len == MAX_SEQUENCE {
                    if self.network_role == NetworkRole::None {
                        self.mode = Mode::Won;
                    } else {
                        let winner = self.leading_player();
                        self.finish_match(winner, None);
                    }
                } else {
                    self.append_color();
                    if self.network_role == NetworkRole::Host {
                        self.turns_remaining = self.alive_mask;
                    }
                    self.begin_playback(now, GAP_MS);
                }
            } else {
                self.mode = if self.turn == Player::One {
                    Mode::Listening
                } else {
                    Mode::WatchingOpponent
                };
            }
            if self.network_role == NetworkRole::Host {
                self.state_changed(now);
            }
        }

        if self.mode != Mode::Showing && self.lit.is_some() && now >= self.input_flash_until {
            self.lit = None;
        }

        if is_win_screen(self.mode, self.network_role, self.player, self.winner) {
            if now >= self.next_confetti_frame {
                self.confetti_frame = self.confetti_frame.wrapping_add(1);
                self.next_confetti_frame = now + Duration::from_millis(CONFETTI_FRAME_MS);
            }
        } else {
            self.confetti_frame = 0;
        }
    }

    /// Accept a local pad press or send a guest move to the host.
    fn accept_input(&mut self, color: Color, now: Instant) {
        if self.network_role == NetworkRole::Guest {
            self.lit = Some(color);
            self.input_flash_until = now + Duration::from_millis(INPUT_FLASH_MS);
            self.tone_start(color);
            self.pending_move = Some((
                self.revision,
                self.sequence_len as u8,
                self.input_index as u8,
                color,
            ));
            self.mode = Mode::AwaitingAck;
            self.next_network_send = now;
            return;
        }

        let player = self.player.unwrap_or(Player::One);
        self.record_move(color, player, now);
        if self.network_role == NetworkRole::Host {
            self.state_changed(now);
        }
    }

    /// Apply one move to the host's authoritative sequence state.
    fn record_move(&mut self, color: Color, player: Player, now: Instant) {
        self.lit = Some(color);
        self.input_flash_until = now + Duration::from_millis(INPUT_FLASH_MS);
        self.tone_start(color);

        if color != self.sequence[self.input_index] {
            if self.network_role == NetworkRole::None {
                self.mode = Mode::GameOver;
            } else {
                self.eliminate_player(player, now);
            }
            return;
        }

        self.input_index += 1;
        if self.input_index == self.sequence_len {
            if self.network_role == NetworkRole::None {
                self.best = self.best.max(self.sequence_len);
                if self.sequence_len == MAX_SEQUENCE {
                    self.mode = Mode::Won;
                } else {
                    self.mode = Mode::NextRound;
                    self.transition_at = now + Duration::from_millis(NEXT_ROUND_DELAY_MS);
                }
            } else {
                self.scores[player.score_index()] =
                    self.scores[player.score_index()].saturating_add(1);
                self.turns_remaining &= !player.bit();
                self.input_index = 0;
                if self.turns_remaining == 0 {
                    if self.sequence_len == MAX_SEQUENCE {
                        let winner = self.leading_player();
                        self.finish_match(winner, None);
                    } else {
                        self.turn = self
                            .next_player_after(self.round_start, self.alive_mask)
                            .unwrap_or(self.round_start);
                        self.round_start = self.turn;
                        self.mode = Mode::NextRound;
                        self.transition_at = now + Duration::from_millis(NEXT_ROUND_DELAY_MS);
                    }
                } else {
                    self.turn = self
                        .next_player_after(player, self.turns_remaining)
                        .unwrap_or(player);
                    self.mode = Mode::NextRound;
                    self.transition_at = now + Duration::from_millis(NEXT_ROUND_DELAY_MS);
                }
            }
        } else if self.network_role == NetworkRole::Host && player != Player::One {
            self.mode = Mode::WatchingOpponent;
        }
    }

    /// Eliminate a player after a mistake or a forfeit, then advance the match.
    fn eliminate_player(&mut self, player: Player, now: Instant) {
        if self.alive_mask & player.bit() == 0 {
            return;
        }
        self.alive_mask &= !player.bit();
        self.turns_remaining &= !player.bit();
        self.loser = Some(player);

        if self.alive_mask.count_ones() <= 1 {
            if let Some(winner) = Player::ALL
                .into_iter()
                .find(|candidate| self.alive_mask & candidate.bit() != 0)
            {
                self.finish_match(winner, Some(player));
            }
            return;
        }

        if self.turn == player {
            self.input_index = 0;
            if self.turns_remaining == 0 {
                let round_anchor = if self.round_start == player {
                    player
                } else {
                    self.round_start
                };
                self.turn = self
                    .next_player_after(round_anchor, self.alive_mask)
                    .unwrap_or(player);
                self.round_start = self.turn;
            } else {
                self.turn = self
                    .next_player_after(player, self.turns_remaining)
                    .unwrap_or(self.turn);
            }
            self.mode = Mode::NextRound;
            self.transition_at = now + Duration::from_millis(NEXT_ROUND_DELAY_MS);
        } else if self.round_start == player {
            self.round_start = self
                .next_player_after(player, self.alive_mask)
                .unwrap_or(self.turn);
        }
    }

    /// Advance the revision and schedule a fresh host snapshot.
    fn state_changed(&mut self, now: Instant) {
        self.revision = self.revision.wrapping_add(1);
        self.pending_state_ack_mask = self.connected_guest_mask();
        self.pending_state_ack = (self.pending_state_ack_mask != 0).then_some(self.revision);
        self.next_network_send = now;
    }

    /// Build the compact state snapshot sent by the host.
    fn wire_state(&self) -> WireState {
        WireState {
            revision: self.revision,
            sequence: self.sequence,
            sequence_len: self.sequence_len as u8,
            input_index: self.input_index as u8,
            turn: self.turn,
            phase: match self.mode {
                Mode::Lobby => WirePhase::Lobby,
                Mode::Showing => WirePhase::Showing,
                Mode::NextRound => WirePhase::NextRound,
                Mode::GameOver => WirePhase::GameOver,
                _ => WirePhase::Listening,
            },
            scores: self.scores,
            player_count: self.player_count,
            alive_mask: self.alive_mask,
            turns_remaining: self.turns_remaining,
            round_start: self.round_start,
            loser: self.loser,
            winner: self.winner,
        }
    }

    /// Start playback from the first colour after a quiet lead-in.
    fn begin_playback(&mut self, now: Instant, delay_ms: u64) {
        self.mode = Mode::Showing;
        self.show_phase = ShowPhase::Gap;
        self.after_playback = None;
        self.playback_index = 0;
        self.lit = None;
        self.transition_at = now + Duration::from_millis(delay_ms);
    }

    /// Add one pseudo-random colour to the fixed sequence.
    fn append_color(&mut self) {
        self.sequence[self.sequence_len] = self.random.next_color();
        self.sequence_len += 1;
    }

    /// Start the matching note. The main loop fills the speaker queue over time.
    fn tone_start(&mut self, color: Color) {
        self.tone.start(color);
    }

    /// Queue as much of the current tone as the speaker can accept.
    fn feed_speaker(&mut self) {
        self.tone.play(&mut self.speaker);
    }

    /// Redraw only when the visible game state has changed.
    fn draw_if_changed(&mut self) {
        let picture = Picture {
            mode: self.mode,
            sequence_len: self.sequence_len,
            best: self.best,
            input_index: self.input_index,
            lit: self.lit,
            network_role: self.network_role,
            player: self.player,
            turn: self.turn,
            scores: self.scores,
            loser: self.loser,
            winner: self.winner,
            opponent: self.opponent,
            player_count: self.player_count,
            turns_remaining: self.turns_remaining,
            lobbies: core::array::from_fn(|index| {
                self.lobby_offers[index].map(|offer| offer.listing)
            }),
            lobby_page: self.lobby_page,
            confetti_frame: self.confetti_frame,
        };
        if self.drawn == Some(picture) {
            return;
        }
        self.drawn = Some(picture);
        draw(&mut self.canvas, picture);
        self.canvas.show(&mut self.display.surface(SCREEN));
    }
}

/// Paint the title, round status and four touch pads.
fn draw(canvas: &mut Canvas, picture: Picture) {
    if is_win_screen(
        picture.mode,
        picture.network_role,
        picture.player,
        picture.winner,
    ) {
        draw_win_screen(canvas, picture);
        return;
    }
    if matches!(
        picture.mode,
        Mode::Lobby | Mode::Joining | Mode::WaitingStart
    ) {
        draw_lobby_screen(canvas, picture);
        return;
    }

    let background = Rgb565::new(1, 2, 5);
    canvas.clear(background);
    canvas.fill(HEADER, Rgb565::new(3, 7, 14));
    common::text(
        canvas,
        "SIMON SAYS",
        Point::new(10, 8),
        common::TITLE_FONT,
        theme::WHITE,
    );

    let mut stats = ArrayString::<32>::new();
    if picture.network_role == NetworkRole::None {
        let _ = write!(
            stats,
            "ROUND {:02}   BEST {:02}",
            picture.sequence_len, picture.best
        );
    } else {
        let _ = write!(
            stats,
            "R{:02} 1:{:02} 2:{:02} 3:{:02} 4:{:02}",
            picture.sequence_len,
            picture.scores[0],
            picture.scores[1],
            picture.scores[2],
            picture.scores[3]
        );
    }
    common::text(
        canvas,
        &stats,
        Point::new(10, 27),
        common::BODY_FONT,
        theme::LIGHT_BLUE,
    );

    let mut status = ArrayString::<32>::new();
    match picture.mode {
        Mode::Ready => status.push_str("1P SOLO OR 4P MATCH"),
        Mode::Lobby => status.push_str("4P LOBBY - CHOOSE A PLAYER"),
        Mode::Joining => status.push_str("CONNECTING TO GAME"),
        Mode::WaitingStart => status.push_str("WAITING FOR HOST"),
        Mode::Showing if picture.network_role != NetworkRole::None => {
            let _ = write!(status, "P{} WATCH THE LIGHTS", picture.turn.number());
        }
        Mode::Showing => status.push_str("WATCH THE LIGHTS"),
        Mode::Listening => {
            if picture.network_role == NetworkRole::None {
                let _ = write!(
                    status,
                    "YOUR TURN  {}/{}",
                    picture.input_index + 1,
                    picture.sequence_len
                );
            } else {
                let _ = write!(
                    status,
                    "P{} REPEAT {}/{}",
                    picture.turn.number(),
                    picture.input_index + 1,
                    picture.sequence_len
                );
            }
        }
        Mode::WatchingOpponent => {
            let _ = write!(status, "PLAYER {} REPEATS", picture.turn.number());
        }
        Mode::AwaitingAck => status.push_str("SENDING YOUR MOVE..."),
        Mode::NextRound
            if picture.network_role != NetworkRole::None && picture.turns_remaining != 0 =>
        {
            status.push_str("NEXT PLAYER...")
        }
        Mode::NextRound => status.push_str("NEXT ROUND..."),
        Mode::GameOver => match (picture.player, picture.winner) {
            (Some(player), Some(winner)) if player == winner => {
                status.push_str("YOU WIN! TAP 1P/4P")
            }
            (Some(_), Some(winner)) => {
                let _ = write!(status, "P{} WINS! TAP 1P/4P", winner.number());
            }
            _ => status.push_str("MATCH OVER! TAP 1P/4P"),
        },
        Mode::Won => status.push_str("PERFECT RUN! TAP 1P/4P"),
    }
    common::text(
        canvas,
        &status,
        Point::new(10, 49),
        common::BODY_FONT,
        theme::WHITE,
    );

    canvas.fill(SOLO_BUTTON, theme::DARK_BLUE);
    common::centered_text(canvas, SOLO_BUTTON, "1P", common::TITLE_FONT, theme::WHITE);
    canvas.fill(DUEL_BUTTON, theme::DARK_BLUE);
    common::centered_text(canvas, DUEL_BUTTON, "4P", common::TITLE_FONT, theme::WHITE);

    for color in Color::ALL {
        let lit = picture.lit == Some(color);
        let area = color.area();
        canvas.fill(area, color.fill(lit));
        common::centered_text(
            canvas,
            area,
            color.name(),
            common::TITLE_FONT,
            color.label(lit),
        );
    }
}

/// Draw the dedicated screen used to find and join multiplayer games.
fn draw_lobby_screen(canvas: &mut Canvas, picture: Picture) {
    canvas.clear(Rgb565::new(1, 2, 8));
    canvas.fill(HEADER, Rgb565::new(3, 7, 14));
    let joining = picture.mode == Mode::Joining;
    let waiting = picture.mode == Mode::WaitingStart;
    let hosting = picture.mode == Mode::Lobby;
    common::text(
        canvas,
        if joining {
            "CONNECTING"
        } else if waiting {
            "JOINED GAME"
        } else {
            "4 PLAYER LOBBY"
        },
        Point::new(10, 8),
        common::TITLE_FONT,
        theme::WHITE,
    );

    canvas.fill(LOBBY_EXIT_BUTTON, theme::DARK_BLUE);
    common::centered_text(
        canvas,
        LOBBY_EXIT_BUTTON,
        if hosting { "EXIT" } else { "LEAVE" },
        common::BODY_FONT,
        theme::WHITE,
    );

    if hosting {
        let can_start = picture.player_count >= 2;
        canvas.fill(
            LOBBY_START_BUTTON,
            if can_start {
                theme::DARK_BLUE
            } else {
                Rgb565::new(7, 11, 17)
            },
        );
        common::centered_text(
            canvas,
            LOBBY_START_BUTTON,
            if can_start { "START" } else { "WAIT" },
            common::BODY_FONT,
            if can_start {
                theme::WHITE
            } else {
                theme::CHARCOAL
            },
        );
        let mut summary = ArrayString::<16>::new();
        let _ = write!(summary, "PLAYERS {}/4", picture.player_count);
        common::centered_text(
            canvas,
            Rectangle::new(Point::new(8, 37), Size::new(SIZE.width - 16, 26)),
            &summary,
            common::TITLE_FONT,
            theme::LIGHT_BLUE,
        );
    } else if joining {
        common::text(
            canvas,
            "REQUESTING A SEAT FROM:",
            Point::new(12, 64),
            common::BODY_FONT,
            theme::LIGHT_BLUE,
        );
        let mut device = ArrayString::<20>::new();
        if let Some(opponent) = picture.opponent {
            let _ = write!(device, "{opponent}");
        }
        common::text(
            canvas,
            &device,
            Point::new(12, 91),
            common::TITLE_FONT,
            theme::WHITE,
        );
        common::text(
            canvas,
            "WAITING FOR THE HOST TO ACCEPT",
            Point::new(12, 132),
            common::BODY_FONT,
            theme::WHITE,
        );
        return;
    } else if waiting {
        let mut seat = ArrayString::<24>::new();
        if let Some(player) = picture.player {
            let _ = write!(seat, "YOU ARE PLAYER {}", player.number());
        } else {
            seat.push_str("JOINED THE LOBBY");
        }
        common::text(
            canvas,
            &seat,
            Point::new(12, 69),
            common::TITLE_FONT,
            theme::WHITE,
        );
        let mut players = ArrayString::<20>::new();
        let _ = write!(players, "PLAYERS {}/4", picture.player_count);
        common::centered_text(
            canvas,
            Rectangle::new(Point::new(12, 99), Size::new(SIZE.width - 24, 30)),
            &players,
            common::TITLE_FONT,
            theme::LIGHT_BLUE,
        );
        common::text(
            canvas,
            "WAITING FOR HOST TO START",
            Point::new(12, 145),
            common::BODY_FONT,
            theme::WHITE,
        );
        common::text(
            canvas,
            "HOST CAN START AT 2 PLAYERS",
            Point::new(12, 173),
            common::BODY_FONT,
            theme::LIGHT_BLUE,
        );
        return;
    }

    let count = picture
        .lobbies
        .iter()
        .filter(|entry| entry.is_some())
        .count();
    let page_count = count.div_ceil(LOBBY_ROWS_PER_PAGE).max(1);
    let first = picture.lobby_page * LOBBY_ROWS_PER_PAGE;
    if count == 0 {
        common::text(
            canvas,
            "YOUR LOBBY IS OPEN - WAITING FOR OTHERS",
            Point::new(10, 89),
            common::BODY_FONT,
            theme::WHITE,
        );
        common::text(
            canvas,
            "OPEN ANOTHER 4P LOBBY TO JOIN IT HERE",
            Point::new(10, 119),
            common::BODY_FONT,
            theme::LIGHT_BLUE,
        );
    } else {
        for row in 0..LOBBY_ROWS_PER_PAGE {
            let index = first + row;
            let Some(listing) = picture.lobbies.get(index).copied().flatten() else {
                break;
            };
            let area = Rectangle::new(
                Point::new(8, 75 + row as i32 * 27),
                Size::new(SIZE.width - 16, 26),
            );
            canvas.fill(area, Rgb565::new(5, 12, 22));
            let mut device = ArrayString::<24>::new();
            let _ = write!(device, "{}  {}/4", listing.device, listing.player_count);
            common::text(
                canvas,
                &device,
                Point::new(16, area.top_left.y + 6),
                common::BODY_FONT,
                theme::WHITE,
            );
            let join = Rectangle::new(
                Point::new(
                    area.top_left.x + area.size.width as i32 - 55,
                    area.top_left.y + 1,
                ),
                Size::new(48, 24),
            );
            canvas.fill(join, theme::DARK_BLUE);
            common::centered_text(canvas, join, "JOIN", common::BODY_FONT, theme::WHITE);
        }
    }

    canvas.fill(LOBBY_PREVIOUS_BUTTON, theme::DARK_BLUE);
    common::centered_text(
        canvas,
        LOBBY_PREVIOUS_BUTTON,
        "PREV",
        common::BODY_FONT,
        if picture.lobby_page > 0 {
            theme::WHITE
        } else {
            theme::CHARCOAL
        },
    );
    let mut page = ArrayString::<12>::new();
    let _ = write!(page, "{}/{}", picture.lobby_page + 1, page_count);
    common::centered_text(
        canvas,
        Rectangle::new(Point::new(90, 211), Size::new(140, 23)),
        &page,
        common::BODY_FONT,
        theme::LIGHT_BLUE,
    );
    canvas.fill(LOBBY_NEXT_BUTTON, theme::DARK_BLUE);
    common::centered_text(
        canvas,
        LOBBY_NEXT_BUTTON,
        "NEXT",
        common::BODY_FONT,
        if picture.lobby_page + 1 < page_count {
            theme::WHITE
        } else {
            theme::CHARCOAL
        },
    );
}

/// Whether the player at this board won the solo run or multiplayer match.
fn is_win_screen(
    mode: Mode,
    network_role: NetworkRole,
    player: Option<Player>,
    winner: Option<Player>,
) -> bool {
    mode == Mode::Won
        || (mode == Mode::GameOver
            && network_role != NetworkRole::None
            && matches!((player, winner), (Some(player), Some(winner)) if player == winner))
}

/// Draw the animated celebration instead of the four game pads.
fn draw_win_screen(canvas: &mut Canvas, picture: Picture) {
    canvas.clear(Rgb565::new(1, 2, 8));
    draw_confetti(canvas, picture.confetti_frame);

    canvas.fill(HEADER, Rgb565::new(3, 7, 14));
    common::text(
        canvas,
        "SIMON SAYS",
        Point::new(10, 8),
        common::TITLE_FONT,
        theme::WHITE,
    );
    canvas.fill(SOLO_BUTTON, theme::DARK_BLUE);
    common::centered_text(canvas, SOLO_BUTTON, "1P", common::TITLE_FONT, theme::WHITE);
    canvas.fill(DUEL_BUTTON, theme::DARK_BLUE);
    common::centered_text(canvas, DUEL_BUTTON, "4P", common::TITLE_FONT, theme::WHITE);

    let card = Rectangle::new(Point::new(18, 82), Size::new(SIZE.width - 36, 104));
    canvas.fill(card, Rgb565::new(3, 8, 16));
    canvas.fill(
        Rectangle::new(Point::new(18, 82), Size::new(SIZE.width - 36, 4)),
        theme::LIGHT_BLUE,
    );
    common::centered_text(
        canvas,
        Rectangle::new(Point::new(22, 94), Size::new(SIZE.width - 44, 34)),
        "YOU WIN!",
        common::TITLE_FONT,
        theme::WHITE,
    );

    let subtitle = if picture.network_role == NetworkRole::None {
        "PERFECT MEMORY!"
    } else {
        "MATCH CHAMPION!"
    };
    common::centered_text(
        canvas,
        Rectangle::new(Point::new(22, 131), Size::new(SIZE.width - 44, 22)),
        subtitle,
        common::BODY_FONT,
        theme::LIGHT_BLUE,
    );

    let mut result = ArrayString::<32>::new();
    if picture.network_role == NetworkRole::None {
        let _ = write!(result, "{} COLORS IN A ROW", picture.sequence_len);
    } else {
        let _ = write!(
            result,
            "1:{:02} 2:{:02} 3:{:02} 4:{:02}",
            picture.scores[0], picture.scores[1], picture.scores[2], picture.scores[3]
        );
    }
    common::centered_text(
        canvas,
        Rectangle::new(Point::new(22, 153), Size::new(SIZE.width - 44, 22)),
        &result,
        common::BODY_FONT,
        theme::WHITE,
    );
    common::centered_text(
        canvas,
        Rectangle::new(Point::new(8, 204), Size::new(SIZE.width - 16, 20)),
        "1P REPLAY     4P MATCH",
        common::BODY_FONT,
        theme::WHITE,
    );
}

/// Move saturated colour pieces down the screen and wrap them to the top.
fn draw_confetti(canvas: &mut Canvas, frame: u16) {
    let screen_width = SIZE.width as usize;
    let screen_height = SIZE.height as usize;
    let frame = usize::from(frame);

    for index in 0..CONFETTI_COUNT {
        let width = 3 + (index % 3) as u32;
        let height = 4 + (index % 4) as u32;
        let max_x = screen_width.saturating_sub(width as usize);
        let x = (index * 43 + frame * ((index % 3) + 1) / 3) % (max_x + 1);
        let fall_distance = (frame * (2 + index % 4) + index * 29) % (screen_height + 16);
        let y = fall_distance as i32 - 8;

        if y >= 0 && y + height as i32 <= SIZE.height as i32 {
            let piece = Rectangle::new(Point::new(x as i32, y), Size::new(width, height));
            canvas.fill(piece, Color::ALL[index % Color::ALL.len()].fill(true));
        }
    }
}

/// A small xorshift generator, seeded anew for each game.
struct SequenceRandom {
    /// Current xorshift state; zero is never used because it cannot advance.
    state: u32,
}

/// Mix the current tick count with the start-button position for a fresh run.
fn seed_from(now: Instant, point: Point) -> u32 {
    let seed = (now.as_millis() as u32)
        .wrapping_add((point.x as u32).rotate_left(11))
        .wrapping_add((point.y as u32).rotate_left(19));
    if seed == 0 { 0x9E37_79B9 } else { seed }
}

impl SequenceRandom {
    /// Start a generator with a nonzero seed.
    const fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9 } else { seed },
        }
    }

    /// Return the next colour from the low two generated bits.
    fn next_color(&mut self) -> Color {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.state = value;
        Color::ALL[(value & 3) as usize]
    }
}

/// Sine waves queued to the board speaker without blocking the game loop.
#[derive(Default)]
struct TonePlayer {
    /// Current oscillator, or `None` when no more samples need generating.
    wave: Option<SineWave>,
    /// Samples still to add to the speaker queue.
    frames_left: usize,
    /// Notes that arrived before the current note finished being generated.
    pending: ArrayVec<Color, MAX_SEQUENCE>,
}

impl TonePlayer {
    /// Drop sound from the previous game before entering a new mode.
    fn stop(&mut self, speaker: &mut Speaker) {
        self.wave = None;
        self.frames_left = 0;
        self.pending.clear();
        speaker.clear();
    }

    /// Play `color` after the active note so rapid presses never cut it short.
    fn start(&mut self, color: Color) {
        if self.frames_left > 0 || !self.pending.is_empty() {
            let _ = self.pending.try_push(color);
            return;
        }
        self.begin(color);
    }

    /// Start generating one note's samples.
    fn begin(&mut self, color: Color) {
        self.wave = Some(SineWave::new(color.frequency_hz()));
        self.frames_left = TONE_FRAMES;
    }

    /// Fill available speaker space a little at a time.
    fn play(&mut self, speaker: &mut Speaker) {
        let mut fed = 0;
        loop {
            if fed == AUDIO_FEED_FRAMES_PER_TICK {
                return;
            }
            if self.frames_left == 0 {
                self.wave = None;
                if self.pending.is_empty() {
                    return;
                }
                let color = self.pending.remove(0);
                self.begin(color);
            }

            let Some(wave) = &mut self.wave else {
                return;
            };
            let frames = speaker
                .available_frames()
                .min(AUDIO_CHUNK_FRAMES)
                .min(self.frames_left)
                .min(AUDIO_FEED_FRAMES_PER_TICK - fed);
            if frames == 0 {
                return;
            }

            let first_frame = TONE_FRAMES - self.frames_left;
            let mut chunk = [0i16; AUDIO_CHUNK_FRAMES * audio::CHANNELS];
            let samples = &mut chunk[..frames * audio::CHANNELS];
            for (offset, frame) in samples.chunks_exact_mut(audio::CHANNELS).enumerate() {
                let envelope = tone_envelope(first_frame + offset);
                frame.fill(wave.next_sample(TONE_VOLUME * envelope));
            }
            speaker.write(samples);
            self.frames_left -= frames;
            fed += frames;
            if self.frames_left == 0 {
                self.wave = None;
            }
        }
    }
}

/// Fade a note in and out using the ramp length of the built-in melody synth.
fn tone_envelope(position: usize) -> f32 {
    let rising = position.min(TONE_RAMP_FRAMES);
    let falling = TONE_FRAMES.saturating_sub(position).min(TONE_RAMP_FRAMES);
    rising.min(falling) as f32 / TONE_RAMP_FRAMES as f32
}
