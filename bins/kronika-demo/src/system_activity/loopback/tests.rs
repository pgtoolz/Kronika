use super::{DATAGRAM_BYTES, connected_pair, transfer};

#[test]
fn an_exact_payload_stays_inside_loopback() {
    let (sender, receive_socket) = connected_pair().unwrap();
    assert!(sender.local_addr().unwrap().ip().is_loopback());
    assert!(sender.peer_addr().unwrap().ip().is_loopback());
    assert!(receive_socket.local_addr().unwrap().ip().is_loopback());
    assert!(receive_socket.peer_addr().unwrap().ip().is_loopback());
    let payload = vec![0x4b; DATAGRAM_BYTES];
    let mut receive_buffer = vec![0; DATAGRAM_BYTES];

    let totals = transfer(
        &sender,
        &receive_socket,
        u64::try_from(DATAGRAM_BYTES * 2 + 17).unwrap(),
        &payload,
        &mut receive_buffer,
    )
    .unwrap();

    assert_eq!(totals, (16_401, 16_401));
}
