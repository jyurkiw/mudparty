//! The server side, as the proxy's seat speaks to MudPlay (design §2.1): silvermere's own
//! offer and answers, so MudPlay can't tell the seat from silvermere.

use mp_telnet::{
    BINARY, ClientCodec, DO, DONT, ECHO, IAC, NAWS, SB, SE, SGA, ServerCodec, TTYPE, WILL, WONT,
};

/// silvermere's opening offer (`silvermere-net/src/telnet/options.rs` `OFFERS`).
const SILVERMERE_OFFERS: [u8; 12] = [
    IAC, WILL, ECHO, IAC, WILL, SGA, IAC, WILL, BINARY, IAC, DO, NAWS,
];

fn decode(codec: &mut ServerCodec, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut data, mut reply) = (Vec::new(), Vec::new());
    codec.decode(input, &mut data, &mut reply);
    (data, reply)
}

#[test]
fn the_offer_is_silvermeres_in_its_order() {
    assert_eq!(ServerCodec::OFFERS, SILVERMERE_OFFERS);
}

#[test]
fn a_client_ends_up_where_silvermere_would_leave_it() {
    // The same client codec, fed the seat's offer, answers as it answers silvermere.
    let mut direct = ClientCodec::new(80, 24);
    let mut seated = ClientCodec::new(80, 24);
    let (mut d1, mut r1, mut d2, mut r2) = (vec![], vec![], vec![], vec![]);
    direct.decode(&SILVERMERE_OFFERS, &mut d1, &mut r1);
    seated.decode(&ServerCodec::OFFERS, &mut d2, &mut r2);
    assert_eq!(r1, r2);
    assert!(seated.server_echoes());
}

#[test]
fn supported_requests_are_answered_once() {
    let mut s = ServerCodec::new();
    assert_eq!(decode(&mut s, &[IAC, DO, ECHO]).1, [IAC, WILL, ECHO]);
    assert_eq!(decode(&mut s, &[IAC, DO, ECHO]).1, []);
    assert_eq!(decode(&mut s, &[IAC, WILL, NAWS]).1, [IAC, DO, NAWS]);
}

#[test]
fn everything_else_is_refused() {
    let mut s = ServerCodec::new();
    // MudPlay offers TERM-TYPE and GMCP; silvermere refuses both.
    assert_eq!(decode(&mut s, &[IAC, WILL, TTYPE]).1, [IAC, DONT, TTYPE]);
    assert_eq!(decode(&mut s, &[IAC, WILL, 201]).1, [IAC, DONT, 201]);
    assert_eq!(decode(&mut s, &[IAC, DO, TTYPE]).1, [IAC, WONT, TTYPE]);
}

#[test]
fn refusals_are_taken_silently_and_reset_the_answer() {
    let mut s = ServerCodec::new();
    decode(&mut s, &[IAC, DO, ECHO]);
    assert_eq!(decode(&mut s, &[IAC, DONT, ECHO]).1, []);
    assert_eq!(decode(&mut s, &[IAC, DO, ECHO]).1, [IAC, WILL, ECHO]);
}

#[test]
fn the_window_size_is_read_from_naws() {
    let mut s = ServerCodec::new();
    assert_eq!(s.window(), None);
    let (data, _) = decode(&mut s, &[b'a', IAC, SB, NAWS, 0, 132, 0, 50, IAC, SE, b'b']);
    assert_eq!(data, b"ab");
    assert_eq!(s.window(), Some((132, 50)));
    // A doubled 0xFF in the payload is one byte, split across reads.
    decode(&mut s, &[IAC, SB, NAWS, 1, IAC]);
    decode(&mut s, &[IAC, 0, 24, IAC, SE]);
    assert_eq!(s.window(), Some((0x01FF, 24)));
}

#[test]
fn typed_data_passes_through_with_iac_undoubled() {
    let mut s = ServerCodec::new();
    let (data, reply) = decode(&mut s, &[b'l', b'o', IAC, IAC, b'k', b'\r', b'\n']);
    assert_eq!(data, [b'l', b'o', IAC, b'k', b'\r', b'\n']);
    assert!(reply.is_empty());
}
