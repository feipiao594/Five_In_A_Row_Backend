use server::rooms::{Coord, RoomService, SeatKind};


#[tokio::test]
async fn service_flow_create_ready_move() {
  let svc = RoomService::default();

  let (_room_id, snap) = svc.create_room("alice", "t".to_string()).await;
  assert_eq!(
    snap.seats.black.as_ref().map(|s| s.username.as_str()),
    Some("alice")
  );

  let room_id = snap.room_id.parse().unwrap();
  let snap = svc.join_room("bob", room_id).await.unwrap();
  assert!(snap.spectators.iter().any(|u| u == "bob"));

  let (_room_id, snap) = svc.take_seat("bob", SeatKind::White).await.unwrap();
  assert_eq!(snap.seats.white.as_ref().map(|s| s.username.as_str()), Some("bob"));

  let (_room_id, _snap, start_evt) = svc.set_ready("alice", true).await.unwrap();
  assert!(start_evt.is_none());
  let (_room_id, _snap, start_evt) = svc.set_ready("bob", true).await.unwrap();
  assert!(start_evt.is_some());

  let (_room_id, payload, _events) = svc
    .match_move("alice", Coord { row: 7, col: 7 })
    .await
    .unwrap();
  assert_eq!(payload.get("accepted").and_then(|v| v.as_bool()), Some(true));

  // Wrong side tries again
  let (_room_id, payload, _events) = svc
    .match_move("alice", Coord { row: 7, col: 8 })
    .await
    .unwrap();
  assert_eq!(payload.get("accepted").and_then(|v| v.as_bool()), Some(false));
}

#[tokio::test]
async fn win_by_moves_emits_match_over() {
  let svc = RoomService::default();
  let (_room_id, snap) = svc.create_room("alice", "t".to_string()).await;
  let room_id = snap.room_id.parse().unwrap();
  let _ = svc.join_room("bob", room_id).await.unwrap();
  let _ = svc.take_seat("bob", SeatKind::White).await.unwrap();

  let _ = svc.set_ready("alice", true).await.unwrap();
  let _ = svc.set_ready("bob", true).await.unwrap();

  // Black: (7,3..7), White: elsewhere.
  let black_moves = [3, 4, 5, 6, 7];
  for (i, col) in black_moves.iter().enumerate() {
    let (_room_id, payload, events) = svc
      .match_move("alice", Coord { row: 7, col: *col })
      .await
      .unwrap();
    assert_eq!(payload.get("accepted").and_then(|v| v.as_bool()), Some(true));

    if i == black_moves.len() - 1 {
      let over = events.iter().find(|e| e.r#type == "match.over");
      assert!(over.is_some());
      let over = over.unwrap();
      assert_eq!(
        over.payload.get("winner").and_then(|v| v.as_str()),
        Some("black")
      );
      break;
    }

    let (_room_id, payload, _events) = svc
      .match_move("bob", Coord { row: 0, col: i as i32 })
      .await
      .unwrap();
    assert_eq!(payload.get("accepted").and_then(|v| v.as_bool()), Some(true));
  }
}
