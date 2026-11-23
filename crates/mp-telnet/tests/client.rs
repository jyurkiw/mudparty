//! The client side of telnet, as a session or the interactive client speaks it to silvermere.

use mp_telnet::{
    BINARY, ClientCodec, DO, DONT, ECHO, IAC, NAWS, NOP, SB, SE, SGA, TTYPE, WILL, WONT, escape,
};

const GMCP: u8 = 201;

fn decode(codec: &mut ClientCodec, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut data, mut reply) = (Vec::new(), Vec::new());
    codec.decode(input, &mut data, &mut reply);
    (data, reply)
}

#[test]
fn plain_data_passes_through() {
    let mut c = ClientCodec::new(80, 24);
    let (data, reply) = decode(&mut c, b"Username: ");
    assert_eq!(data, b"Username: ");
    assert!(reply.is_empty());
}

#[test]
fn doubled_iac_is_one_data_byte() {
    let mut c = ClientCodec::new(80, 24);
    let (data, _) = decode(&mut c, &[b'a', IAC, IAC, b'b']);
    assert_eq!(data, [b'a', 0xFF, b'b']);
}

// silvermere's opening offer, crates/silvermere-net/src/telnet/options.rs `OFFERS`.
#[test]
fn silvermere_offer_is_accepted_and_naws_sent() {
    let mut c = ClientCodec::new(80, 24);
    let offer = [
        IAC, WILL, ECHO, IAC, WILL, SGA, IAC, WILL, BINARY, IAC, DO, NAWS,
    ];
    let (data, reply) = decode(&mut c, &offer);
    assert!(data.is_empty());
    assert_eq!(
        reply,
        [
            IAC, DO, ECHO, IAC, DO, SGA, IAC, DO, BINARY, IAC, WILL, NAWS, IAC, SB, NAWS, 0, 80, 0,
            24, IAC, SE,
        ]
    );
    assert!(c.server_echoes());
}

#[test]
fn a_repeated_offer_is_not_answered_again() {
    let mut c = ClientCodec::new(80, 24);
    decode(&mut c, &[IAC, WILL, ECHO]);
    let (_, reply) = decode(&mut c, &[IAC, WILL, ECHO]);
    assert!(reply.is_empty(), "{reply:?}");
}

#[test]
fn other_options_are_refused() {
    let mut c = ClientCodec::new(80, 24);
    let (_, reply) = decode(&mut c, &[IAC, DO, TTYPE, IAC, WILL, GMCP]);
    assert_eq!(reply, [IAC, WONT, TTYPE, IAC, DONT, GMCP]);
}

#[test]
fn a_refusal_of_an_agreed_option_is_acknowledged_once() {
    let mut c = ClientCodec::new(80, 24);
    decode(&mut c, &[IAC, WILL, ECHO]);
    let (_, reply) = decode(&mut c, &[IAC, WONT, ECHO]);
    assert_eq!(reply, [IAC, DONT, ECHO]);
    assert!(!c.server_echoes());
    let (_, reply) = decode(&mut c, &[IAC, WONT, ECHO]);
    assert!(reply.is_empty());
}

#[test]
fn commands_split_across_reads_still_parse() {
    let mut c = ClientCodec::new(80, 24);
    let (d1, r1) = decode(&mut c, &[b'x', IAC]);
    let (d2, r2) = decode(&mut c, &[WILL]);
    let (d3, r3) = decode(&mut c, &[ECHO, b'y']);
    assert_eq!([d1, d2, d3].concat(), b"xy");
    assert_eq!([r1, r2, r3].concat(), [IAC, DO, ECHO]);
}

#[test]
fn subnegotiations_are_dropped_and_data_around_them_kept() {
    let mut c = ClientCodec::new(80, 24);
    let (data, reply) = decode(&mut c, &[b'a', IAC, SB, TTYPE, 1, IAC, SE, b'b']);
    assert_eq!(data, b"ab");
    assert!(reply.is_empty());
}

#[test]
fn nop_is_ignored() {
    let mut c = ClientCodec::new(80, 24);
    let (data, reply) = decode(&mut c, &[b'a', IAC, NOP, b'b']);
    assert_eq!(data, b"ab");
    assert!(reply.is_empty());
}

#[test]
fn naws_doubles_a_255_in_its_payload() {
    let mut c = ClientCodec::new(255, 24);
    let (_, reply) = decode(&mut c, &[IAC, DO, NAWS]);
    assert_eq!(
        reply,
        [IAC, WILL, NAWS, IAC, SB, NAWS, 0, IAC, IAC, 0, 24, IAC, SE]
    );
}

#[test]
fn a_window_change_is_sent_only_once_naws_is_agreed() {
    let mut c = ClientCodec::new(80, 24);
    let mut reply = Vec::new();
    c.set_window_size(100, 40, &mut reply);
    assert!(reply.is_empty());
    decode(&mut c, &[IAC, DO, NAWS]);
    c.set_window_size(120, 50, &mut reply);
    assert_eq!(reply, [IAC, SB, NAWS, 0, 120, 0, 50, IAC, SE]);
}

#[test]
fn escape_doubles_iac_in_outgoing_data() {
    let mut out = Vec::new();
    escape(&[b'a', 0xFF, b'b'], &mut out);
    assert_eq!(out, [b'a', IAC, IAC, b'b']);
}
