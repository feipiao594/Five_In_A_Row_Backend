use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::protocol::EnvelopeOut;

pub const BOARD_SIZE: usize = 15;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Color {
  Black,
  White,
}

impl Color {
  pub fn other(self) -> Self {
    match self {
      Color::Black => Color::White,
      Color::White => Color::Black,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coord {
  pub row: i32,
  pub col: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Move {
  pub color: Color,
  pub coord: Coord,
}

#[derive(Debug, Clone)]
pub struct RoomService {
  rooms: Arc<dashmap::DashMap<Uuid, Arc<Mutex<Room>>>>,
  user_room: Arc<dashmap::DashMap<String, Uuid>>,
}

impl Default for RoomService {
  fn default() -> Self {
    Self {
      rooms: Arc::new(dashmap::DashMap::new()),
      user_room: Arc::new(dashmap::DashMap::new()),
    }
  }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RoomState {
  Waiting,
  Playing,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeatInfo {
  pub username: String,
  pub ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomSnapshot {
  #[serde(rename = "roomId")]
  pub room_id: String,
  pub title: String,
  pub seats: SeatsSnapshot,
  pub spectators: Vec<String>,
  pub state: RoomState,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeatsSnapshot {
  pub black: Option<SeatInfo>,
  pub white: Option<SeatInfo>,
}

#[derive(Debug, Clone)]
struct Seat {
  username: String,
  ready: bool,
}

#[derive(Debug, Clone)]
struct Seats {
  black: Option<Seat>,
  white: Option<Seat>,
}

#[derive(Debug, Clone)]
struct Match {
  match_id: Uuid,
  turn: Color,
  moves: Vec<Move>,
  board: [[u8; BOARD_SIZE]; BOARD_SIZE],
}

#[derive(Debug, Clone)]
struct Room {
  room_id: Uuid,
  title: String,
  seats: Seats,
  spectators: Vec<String>,
  state: RoomState,
  current_match: Option<Match>,
}

#[derive(Debug, Clone, Copy)]
pub enum SeatKind {
  Black,
  White,
  Spectator,
}

impl RoomService {
  pub fn debug_room_ids(&self) -> Vec<String> {
    let mut ids: Vec<String> = self.rooms.iter().map(|e| e.key().to_string()).collect();
    ids.sort();
    ids
  }

  pub fn debug_room_id_for_user(&self, username: &str) -> Option<String> {
    self.user_room.get(username).map(|v| v.value().to_string())
  }

  pub async fn create_room(&self, username: &str, title: String) -> (Uuid, RoomSnapshot) {
    let room_id = Uuid::new_v4();
    let room = Room {
      room_id,
      title: if title.trim().is_empty() {
        "房间".to_string()
      } else {
        title.trim().to_string()
      },
      seats: Seats {
        black: Some(Seat {
          username: username.to_string(),
          ready: false,
        }),
        white: None,
      },
      spectators: vec![],
      state: RoomState::Waiting,
      current_match: None,
    };

    self.user_room.insert(username.to_string(), room_id);
    self.rooms.insert(room_id, Arc::new(Mutex::new(room)));
    let snapshot = self.snapshot(room_id).await.unwrap();
    (room_id, snapshot)
  }

  pub async fn join_room(&self, username: &str, room_id: Uuid) -> Result<RoomSnapshot, &'static str> {
    let room = self.rooms.get(&room_id).ok_or("room_not_found")?.clone();
    let mut room = room.lock().await;

    if room.seats.black.as_ref().map(|s| s.username.as_str()) == Some(username)
      || room.seats.white.as_ref().map(|s| s.username.as_str()) == Some(username)
      || room.spectators.iter().any(|u| u == username)
    {
      self.user_room.insert(username.to_string(), room_id);
      return Ok(room.snapshot());
    }

    room.spectators.push(username.to_string());
    self.user_room.insert(username.to_string(), room_id);
    Ok(room.snapshot())
  }

  pub async fn leave_room(&self, username: &str) -> Option<(RoomSnapshot, Vec<EnvelopeOut>)> {
    let room_id = self.user_room.remove(username).map(|(_, id)| id)?;
    let room = self.rooms.get(&room_id)?.clone();
    let mut room = room.lock().await;

    // Remove from seats/spectators
    if room.seats.black.as_ref().map(|s| s.username.as_str()) == Some(username) {
      room.seats.black = None;
    }
    if room.seats.white.as_ref().map(|s| s.username.as_str()) == Some(username) {
      room.seats.white = None;
    }
    room.spectators.retain(|u| u != username);

    let mut events = vec![];

    // If match is playing and leaver was a seat, end match as disconnect.
    if matches!(room.state, RoomState::Playing) && room.current_match.is_some() {
      if let Some(m) = &room.current_match {
        // Determine winner: remaining seat if any; else draw.
        let winner = if room.seats.black.is_some() && room.seats.white.is_none() {
          Some(Color::Black)
        } else if room.seats.white.is_some() && room.seats.black.is_none() {
          Some(Color::White)
        } else {
          None
        };
        events.push(EnvelopeOut::event(
          "match.over",
          serde_json::json!({
            "matchId": m.match_id.to_string(),
            "result": match winner {
              Some(Color::Black) => "black_win",
              Some(Color::White) => "white_win",
              None => "draw",
            },
            "winner": winner.map(|c| match c { Color::Black => "black", Color::White => "white" }),
            "reason": "disconnect"
          }),
        ));
      }
      room.state = RoomState::Waiting;
      room.current_match = None;
      if let Some(s) = &mut room.seats.black {
        s.ready = false;
      }
      if let Some(s) = &mut room.seats.white {
        s.ready = false;
      }
    }

    // If room becomes empty, drop it.
    let empty = room.seats.black.is_none() && room.seats.white.is_none() && room.spectators.is_empty();
    let snapshot = room.snapshot();
    drop(room);
    if empty {
      tracing::info!(
        username = %username,
        room_id = %room_id,
        "room.leave: removing empty room"
      );
      self.rooms.remove(&room_id);
    }
    Some((snapshot, events))
  }

  pub async fn take_seat(
    &self,
    username: &str,
    seat: SeatKind,
  ) -> Result<(Uuid, RoomSnapshot), &'static str> {
    let room_id = *self.user_room.get(username).ok_or("not_in_room")?;
    let room = self.rooms.get(&room_id).ok_or("room_not_found")?.clone();
    let mut room = room.lock().await;

    if matches!(room.state, RoomState::Playing) {
      return Err("invalid_room_state");
    }

    // Remove from current seat/spectators first
    if room.seats.black.as_ref().map(|s| s.username.as_str()) == Some(username) {
      room.seats.black = None;
    }
    if room.seats.white.as_ref().map(|s| s.username.as_str()) == Some(username) {
      room.seats.white = None;
    }
    room.spectators.retain(|u| u != username);

    match seat {
      SeatKind::Black => {
        if room.seats.black.is_some() {
          return Err("seat_taken");
        }
        room.seats.black = Some(Seat {
          username: username.to_string(),
          ready: false,
        });
      }
      SeatKind::White => {
        if room.seats.white.is_some() {
          return Err("seat_taken");
        }
        room.seats.white = Some(Seat {
          username: username.to_string(),
          ready: false,
        });
      }
      SeatKind::Spectator => {
        room.spectators.push(username.to_string());
      }
    }

    Ok((room_id, room.snapshot()))
  }

  pub async fn set_ready(
    &self,
    username: &str,
    ready: bool,
  ) -> Result<(Uuid, RoomSnapshot, Option<EnvelopeOut>), &'static str> {
    let room_id = *self.user_room.get(username).ok_or("not_in_room")?;
    let room = self.rooms.get(&room_id).ok_or("room_not_found")?.clone();
    let mut room = room.lock().await;

    if matches!(room.state, RoomState::Playing) {
      return Err("invalid_room_state");
    }

    let mut is_seat = false;
    if let Some(s) = &mut room.seats.black {
      if s.username == username {
        s.ready = ready;
        is_seat = true;
      }
    }
    if let Some(s) = &mut room.seats.white {
      if s.username == username {
        s.ready = ready;
        is_seat = true;
      }
    }
    if !is_seat {
      return Err("forbidden");
    }

    let mut match_start_event = None;
    if let (Some(b), Some(w)) = (&room.seats.black, &room.seats.white) {
      if b.ready && w.ready {
        let match_id = Uuid::new_v4();
        room.state = RoomState::Playing;
        room.current_match = Some(Match {
          match_id,
          turn: Color::Black,
          moves: vec![],
          board: [[0u8; BOARD_SIZE]; BOARD_SIZE],
        });
        match_start_event = Some(EnvelopeOut::event(
          "match.start",
          serde_json::json!({
            "matchId": match_id.to_string(),
            "boardSize": BOARD_SIZE,
            "turn": "black",
            "moves": []
          }),
        ));
      }
    }

    Ok((room_id, room.snapshot(), match_start_event))
  }

  pub async fn match_move(
    &self,
    username: &str,
    coord: Coord,
  ) -> Result<(Uuid, serde_json::Value, Vec<EnvelopeOut>), (&'static str, &'static str)> {
    let room_id = *self.user_room.get(username).ok_or(("not_in_room", "未加入房间"))?;
    let room = self.rooms.get(&room_id).ok_or(("room_not_found", "房间不存在"))?.clone();
    let mut room = room.lock().await;

    if !matches!(room.state, RoomState::Playing) {
      return Err(("invalid_room_state", "房间未在对局中"));
    }
    let (match_id, turn) = match room.current_match.as_ref() {
      Some(m) => (m.match_id, m.turn),
      None => return Err(("match_not_found", "对局不存在")),
    };

    let seat_username = match turn {
      Color::Black => room.seats.black.as_ref().map(|s| s.username.as_str()),
      Color::White => room.seats.white.as_ref().map(|s| s.username.as_str()),
    };
    if seat_username != Some(username) {
      return Ok((
        room_id,
        serde_json::json!({ "accepted": false, "reason": "not_your_turn" }),
        vec![],
      ));
    }

    let Some(m) = &mut room.current_match else {
      return Err(("match_not_found", "对局不存在"));
    };

    if coord.row < 0
      || coord.col < 0
      || coord.row as usize >= BOARD_SIZE
      || coord.col as usize >= BOARD_SIZE
    {
      return Ok((
        room_id,
        serde_json::json!({ "accepted": false, "reason": "out_of_range" }),
        vec![],
      ));
    }

    let r = coord.row as usize;
    let c = coord.col as usize;
    if m.board[r][c] != 0 {
      return Ok((
        room_id,
        serde_json::json!({ "accepted": false, "reason": "overlap" }),
        vec![],
      ));
    }

    m.board[r][c] = match turn {
      Color::Black => 1,
      Color::White => 2,
    };
    m.moves.push(Move {
      color: turn,
      coord: coord.clone(),
    });

    let mut events = vec![];
    events.push(EnvelopeOut::event(
      "match.moved",
      serde_json::json!({
        "matchId": match_id.to_string(),
        "move": { "color": match turn { Color::Black => "black", Color::White => "white" }, "coord": coord },
        "turn": match turn.other() { Color::Black => "black", Color::White => "white" }
      }),
    ));

    let mut over_event = None;
    if is_win(&m.board, r, c, m.board[r][c]) {
      let winner = turn;
      over_event = Some(EnvelopeOut::event(
        "match.over",
        serde_json::json!({
          "matchId": match_id.to_string(),
          "result": match winner { Color::Black => "black_win", Color::White => "white_win" },
          "winner": match winner { Color::Black => "black", Color::White => "white" },
          "reason": "five_in_a_row"
        }),
      ));
    } else if m.moves.len() >= BOARD_SIZE * BOARD_SIZE {
      over_event = Some(EnvelopeOut::event(
        "match.over",
        serde_json::json!({
          "matchId": match_id.to_string(),
          "result": "draw",
          "winner": null,
          "reason": "board_full"
        }),
      ));
    }

    if let Some(evt) = over_event {
      events.push(evt);
      // Reset room to waiting for next match.
      room.state = RoomState::Waiting;
      room.current_match = None;
      if let Some(s) = &mut room.seats.black {
        s.ready = false;
      }
      if let Some(s) = &mut room.seats.white {
        s.ready = false;
      }
      events.push(EnvelopeOut::event("room.snapshot", serde_json::to_value(room.snapshot()).unwrap()));
    } else {
      m.turn = turn.other();
    }

    Ok((
      room_id,
      serde_json::json!({
        "accepted": true,
        "turn": match turn.other() { Color::Black => "black", Color::White => "white" },
        "move": { "color": match turn { Color::Black => "black", Color::White => "white" }, "coord": coord }
      }),
      events,
    ))
  }

  pub async fn snapshot(&self, room_id: Uuid) -> Option<RoomSnapshot> {
    let room = self.rooms.get(&room_id)?.clone();
    let room = room.lock().await;
    Some(room.snapshot())
  }

  pub fn room_id_for_user(&self, username: &str) -> Option<Uuid> {
    self.user_room.get(username).map(|v| *v)
  }

  pub async fn participants(&self, room_id: Uuid) -> Vec<String> {
    let room = self.rooms.get(&room_id).map(|v| v.clone());
    let Some(room) = room else { return vec![]; };
    let room = room.lock().await;
    let mut users = vec![];
    if let Some(s) = &room.seats.black {
      users.push(s.username.clone());
    }
    if let Some(s) = &room.seats.white {
      users.push(s.username.clone());
    }
    users.extend(room.spectators.iter().cloned());
    users.sort();
    users.dedup();
    users
  }
}

impl Room {
  fn snapshot(&self) -> RoomSnapshot {
    RoomSnapshot {
      room_id: self.room_id.to_string(),
      title: self.title.clone(),
      seats: SeatsSnapshot {
        black: self.seats.black.as_ref().map(|s| SeatInfo {
          username: s.username.clone(),
          ready: s.ready,
        }),
        white: self.seats.white.as_ref().map(|s| SeatInfo {
          username: s.username.clone(),
          ready: s.ready,
        }),
      },
      spectators: self.spectators.clone(),
      state: self.state.clone(),
    }
  }
}

fn is_win(board: &[[u8; BOARD_SIZE]; BOARD_SIZE], r: usize, c: usize, v: u8) -> bool {
  let dirs: &[(i32, i32)] = &[(0, 1), (1, 0), (1, 1), (1, -1)];
  for (dr, dc) in dirs {
    let mut count = 1i32;
    for step in 1..5 {
      let rr = r as i32 + dr * step;
      let cc = c as i32 + dc * step;
      if rr < 0 || cc < 0 || rr as usize >= BOARD_SIZE || cc as usize >= BOARD_SIZE {
        break;
      }
      if board[rr as usize][cc as usize] != v {
        break;
      }
      count += 1;
    }
    for step in 1..5 {
      let rr = r as i32 - dr * step;
      let cc = c as i32 - dc * step;
      if rr < 0 || cc < 0 || rr as usize >= BOARD_SIZE || cc as usize >= BOARD_SIZE {
        break;
      }
      if board[rr as usize][cc as usize] != v {
        break;
      }
      count += 1;
    }
    if count >= 5 {
      return true;
    }
  }
  false
}
