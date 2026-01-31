use axum::{
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
  auth,
  config::Config,
  protocol::{EnvelopeIn, EnvelopeOut},
  rooms::{Coord, RoomService, SeatKind},
};

async fn broadcast_room_event(hub: &Hub, rooms: &RoomService, room_id: Uuid, evt: &EnvelopeOut) {
  for u in rooms.participants(room_id).await {
    hub.send_json(&u, evt);
  }
}

async fn broadcast_room_snapshot(
  hub: &Hub,
  rooms: &RoomService,
  room_id: Uuid,
  snapshot: serde_json::Value,
) {
  let evt = EnvelopeOut::event("room.snapshot", snapshot);
  broadcast_room_event(hub, rooms, room_id, &evt).await;
}

async fn leave_room_with_broadcast(
  hub: &Hub,
  rooms: &RoomService,
  room_id: Uuid,
  username: &str,
) -> bool {
  let Some((snapshot, extra_events)) = rooms.leave_room(username).await else {
    return false;
  };
  let participants = rooms.participants(room_id).await;
  for evt in extra_events {
    for u in &participants {
      hub.send_json(u, &evt);
    }
  }
  let snap_evt = EnvelopeOut::event("room.snapshot", serde_json::to_value(snapshot).unwrap());
  for u in participants {
    hub.send_json(&u, &snap_evt);
  }
  true
}

async fn handle_room_create(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  // Enforce single-room: leaving previous room avoids "ghost rooms" where the creator
  // is still occupying a seat but can no longer interact with that room.
  if let Some(old_room_id) = rooms.room_id_for_user(username) {
    tracing::info!(
      username = %username,
      old_room_id = %old_room_id,
      "room.create: leaving previous room first"
    );
    let _ = leave_room_with_broadcast(hub, rooms, old_room_id, username).await;
  }

  let title = req
    .payload
    .get("title")
    .and_then(|v| v.as_str())
    .unwrap_or("房间")
    .to_string();
  let (room_id, snapshot) = rooms.create_room(username, title).await;
  tracing::info!(
    username = %username,
    room_id = %room_id,
    rooms = ?rooms.debug_room_ids(),
    "room.create: created"
  );

  let resp = EnvelopeOut::resp_ok(
    req,
    serde_json::json!({ "roomId": room_id.to_string(), "room": snapshot }),
  );
  hub.send_json(username, &resp);
  let evt = EnvelopeOut::event("room.snapshot", serde_json::to_value(snapshot).unwrap());
  hub.send_json(username, &evt);
}

async fn handle_room_join(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  let Some(room_id) = req
    .payload
    .get("roomId")
    .and_then(|v| v.as_str())
    .and_then(|s| s.parse::<Uuid>().ok())
  else {
    hub.send_json(username, &EnvelopeOut::resp_err(req, "bad_request", "缺少 roomId"));
    return;
  };

  // If user is already in another room, leave it first to keep user_room mapping sane.
  if let Some(old_room_id) = rooms.room_id_for_user(username) {
    if old_room_id != room_id {
      tracing::info!(
        username = %username,
        old_room_id = %old_room_id,
        new_room_id = %room_id,
        "room.join: leaving previous room first"
      );
      let _ = leave_room_with_broadcast(hub, rooms, old_room_id, username).await;
    }
  }

  tracing::info!(
    username = %username,
    room_id = %room_id,
    rooms = ?rooms.debug_room_ids(),
    "room.join: attempt"
  );

  match rooms.join_room(username, room_id).await {
    Ok(snapshot) => {
      tracing::info!(
        username = %username,
        room_id = %room_id,
        user_room = ?rooms.debug_room_id_for_user(username),
        "room.join: ok"
      );
      hub.send_json(
        username,
        &EnvelopeOut::resp_ok(req, serde_json::json!({ "room": snapshot })),
      );
      broadcast_room_snapshot(hub, rooms, room_id, serde_json::to_value(snapshot).unwrap()).await;
    }
    Err(code) => {
      tracing::info!(
        username = %username,
        room_id = %room_id,
        rooms = ?rooms.debug_room_ids(),
        code = %code,
        "room.join: err"
      );
      let msg = match code {
        "room_not_found" => "房间不存在",
        _ => "加入房间失败",
      };
      hub.send_json(username, &EnvelopeOut::resp_err(req, code, msg));
    }
  }
}

async fn handle_room_leave(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  let Some(room_id) = rooms.room_id_for_user(username) else {
    hub.send_json(username, &EnvelopeOut::resp_err(req, "not_in_room", "未加入房间"));
    return;
  };

  if !leave_room_with_broadcast(hub, rooms, room_id, username).await {
    hub.send_json(username, &EnvelopeOut::resp_err(req, "leave_room_failed", "退出房间失败"));
    return;
  }
  hub.send_json(username, &EnvelopeOut::resp_ok(req, serde_json::json!({})));
}

async fn handle_room_take_seat(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  let seat_str = req
    .payload
    .get("seat")
    .and_then(|v| v.as_str())
    .unwrap_or("spectator");
  let seat = match seat_str {
    "black" => SeatKind::Black,
    "white" => SeatKind::White,
    "spectator" => SeatKind::Spectator,
    _ => {
      hub.send_json(
        username,
        &EnvelopeOut::resp_err(req, "bad_request", "seat 只能是 black/white/spectator"),
      );
      return;
    }
  };

  match rooms.take_seat(username, seat).await {
    Ok((room_id, snapshot)) => {
      hub.send_json(
        username,
        &EnvelopeOut::resp_ok(req, serde_json::json!({ "room": snapshot })),
      );
      if let Ok(snap) = serde_json::to_value(snapshot) {
        broadcast_room_snapshot(hub, rooms, room_id, snap).await;
      }
    }
    Err(code) => hub.send_json(username, &EnvelopeOut::resp_err(req, code, "换座失败")),
  }
}

async fn handle_room_ready(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  let ready = req.payload.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
  match rooms.set_ready(username, ready).await {
    Ok((room_id, snapshot, match_start_evt)) => {
      hub.send_json(
        username,
        &EnvelopeOut::resp_ok(req, serde_json::json!({ "room": snapshot })),
      );
      let snap_evt = EnvelopeOut::event("room.snapshot", serde_json::to_value(snapshot).unwrap());
      let participants = rooms.participants(room_id).await;
      for u in &participants {
        hub.send_json(u, &snap_evt);
      }
      if let Some(evt) = match_start_evt {
        for u in participants {
          hub.send_json(&u, &evt);
        }
      }
    }
    Err(code) => hub.send_json(username, &EnvelopeOut::resp_err(req, code, "准备失败")),
  }
}

async fn handle_match_move(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  let coord = req
    .payload
    .get("coord")
    .and_then(|v| serde_json::from_value::<Coord>(v.clone()).ok());
  let Some(coord) = coord else {
    hub.send_json(username, &EnvelopeOut::resp_err(req, "bad_request", "缺少 coord"));
    return;
  };

  match rooms.match_move(username, coord).await {
    Ok((room_id, resp_payload, events)) => {
      hub.send_json(username, &EnvelopeOut::resp_ok(req, resp_payload));
      let participants = rooms.participants(room_id).await;
      for evt in events {
        for u in &participants {
          hub.send_json(u, &evt);
        }
      }
    }
    Err((code, msg)) => hub.send_json(username, &EnvelopeOut::resp_err(req, code, msg)),
  }
}

async fn dispatch_ws_req(hub: &Hub, rooms: &RoomService, username: &str, req: &EnvelopeIn) {
  match req.r#type.as_str() {
    "room.create" => handle_room_create(hub, rooms, username, req).await,
    "room.join" => handle_room_join(hub, rooms, username, req).await,
    "room.leave" => handle_room_leave(hub, rooms, username, req).await,
    "room.takeSeat" => handle_room_take_seat(hub, rooms, username, req).await,
    "room.ready" => handle_room_ready(hub, rooms, username, req).await,
    "match.move" => handle_match_move(hub, rooms, username, req).await,
    _ => hub.send_json(username, &EnvelopeOut::resp_err(req, "bad_request", "未知消息类型")),
  }
}

#[derive(Default, Clone)]
pub struct Hub {
  conns: std::sync::Arc<DashMap<String, mpsc::UnboundedSender<Message>>>,
}

impl Hub {
    pub fn send(&self, username: &str, msg: Message) {
        if let Some(tx) = self.conns.get(username) {
            let _ = tx.value().send(msg);
        }
    }

    pub fn send_json(&self, username: &str, out: &EnvelopeOut) {
        if let Ok(s) = serde_json::to_string(out) {
            self.send(username, Message::Text(s.into()));
        }
    }

    pub async fn kick(&self, username: &str) {
        if let Some((_, tx)) = self.conns.remove(username) {
            let _ = tx.send(Message::Text(
                serde_json::to_string(&Envelope {
                    v: 1,
                    r#type: "auth.kicked",
                    payload: serde_json::json!({ "reason": "single_session" }),
                })
                .unwrap_or_else(|_| {
                    "{\"v\":1,\"type\":\"auth.kicked\",\"payload\":{\"reason\":\"single_session\"}}"
                        .to_string()
                })
                .into(),
            ));
            let _ = tx.send(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: "single_session".into(),
            })));
        }
    }

    fn register(&self, username: String, tx: mpsc::UnboundedSender<Message>) {
        // Replace existing connection if any (single-session).
        if let Some(old) = self.conns.insert(username.clone(), tx) {
            let _ = old.send(Message::Text(
                serde_json::to_string(&Envelope {
                    v: 1,
                    r#type: "auth.kicked",
                    payload: serde_json::json!({ "reason": "single_session" }),
                })
                .unwrap_or_default()
                .into(),
            ));
            let _ = old.send(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: "single_session".into(),
            })));
        }
    }

    fn unregister(&self, username: &str) {
        self.conns.remove(username);
    }
}

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    #[serde(rename = "accessToken")]
    pub access_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct Envelope {
    v: i32,
    #[serde(rename = "type")]
    r#type: &'static str,
    payload: serde_json::Value,
}

pub async fn ws_handler(
    State(cfg): State<Config>,
    State(hub): State<Hub>,
    State(rooms): State<RoomService>,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Access token from query or Authorization header.
    let token = q
        .access_token
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    let Ok(claims) = auth::verify_access_token(&cfg, &token) else {
        // Cannot return JSON here; just refuse upgrade by returning 401.
        return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };

    let username = claims.sub;
    ws.on_upgrade(move |socket| handle_socket(socket, hub, rooms, username))
}

async fn handle_socket(socket: WebSocket, hub: Hub, rooms: RoomService, username: String) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let out_tx = tx.clone();
    hub.register(username.clone(), tx);

    let (mut sender, mut receiver) = socket.split();

    let username_for_tx = username.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // On connect, if already in a room, push current snapshot.
    if let Some(room_id) = rooms.room_id_for_user(&username) {
        if let Some(snapshot) = rooms.snapshot(room_id).await {
            let evt = EnvelopeOut::event("room.snapshot", serde_json::to_value(snapshot).unwrap());
            let _ = out_tx.send(Message::Text(serde_json::to_string(&evt).unwrap().into()));
        }
    }

    // Message loop.
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(t) => {
                if t == "ping" {
                    let _ = out_tx.send(Message::Text("pong".into()));
                    continue;
                }

                let req: EnvelopeIn = match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if req.v != 1 {
                    if req.req_id.is_some() {
                        hub.send_json(
                            &username,
                            &EnvelopeOut::resp_err(&req, "bad_request", "协议版本不支持"),
                        );
                    }
                    continue;
                }

                // Dispatch.
                dispatch_ws_req(&hub, &rooms, &username, &req).await;
            }
            Message::Ping(v) => {
                let _ = out_tx.send(Message::Pong(v));
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Treat WS disconnect as leaving current room.
    tracing::info!(
      username = %username_for_tx,
      user_room = ?rooms.debug_room_id_for_user(&username_for_tx),
      "ws: disconnected, leaving room"
    );
    let left = rooms.leave_room(&username_for_tx).await;
    if let Some((snapshot, _)) = &left {
        tracing::info!(
          username = %username_for_tx,
          room_id = %snapshot.room_id,
          rooms = ?rooms.debug_room_ids(),
          "ws: left room"
        );
    } else {
        tracing::info!(
          username = %username_for_tx,
          rooms = ?rooms.debug_room_ids(),
          "ws: not in room"
        );
    }
    hub.unregister(&username_for_tx);
    send_task.abort();
}
