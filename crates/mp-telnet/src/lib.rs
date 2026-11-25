//! Telnet for mudparty: the IAC codec and option negotiation.
//!
//! [`ClientCodec`] is the client side, as a session or the interactive client speaks to
//! silvermere. silvermere offers `WILL ECHO`, `WILL SGA`, `WILL BINARY` and `DO NAWS`
//! (`silvermere-net/src/telnet/options.rs`); the client agrees to exactly those and refuses
//! the rest. Each answer is given only when it changes an option's state, so two sides that
//! acknowledge acknowledgements never loop.
//!
//! [`ServerCodec`] is the server side, as the proxy's seat speaks to MudPlay: silvermere's
//! own offer and answers, so MudPlay can't tell the seat from silvermere.

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const NOP: u8 = 241;
pub const SE: u8 = 240;

pub const BINARY: u8 = 0;
pub const ECHO: u8 = 1;
pub const SGA: u8 = 3;
pub const TTYPE: u8 = 24;
pub const NAWS: u8 = 31;

/// Appends `data` to `out` with every 0xFF doubled.
pub fn escape(data: &[u8], out: &mut Vec<u8>) {
    for &b in data {
        out.push(b);
        if b == IAC {
            out.push(IAC);
        }
    }
}

/// Options the server may perform for us.
fn server_may(option: u8) -> bool {
    matches!(option, ECHO | SGA | BINARY)
}

/// Options we perform when the server asks.
fn client_will(option: u8) -> bool {
    matches!(option, NAWS | BINARY)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    Verb(u8),
    Sb,
    SbIac,
}

/// The client side of a telnet connection.
#[derive(Debug, Clone)]
pub struct ClientCodec {
    state: State,
    /// Options the server has agreed to perform, by option byte.
    server_on: [bool; 256],
    /// Options we have agreed to perform.
    client_on: [bool; 256],
    cols: u16,
    rows: u16,
}

impl ClientCodec {
    /// A codec that will report a `cols` × `rows` window once the server asks for NAWS.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            state: State::Data,
            server_on: [false; 256],
            client_on: [false; 256],
            cols,
            rows,
        }
    }

    /// Decodes `input`, appending its data bytes to `data` and any negotiation answers to
    /// `reply`, which the caller writes back to the server. A command split across two
    /// reads is completed by the second.
    pub fn decode(&mut self, input: &[u8], data: &mut Vec<u8>, reply: &mut Vec<u8>) {
        for &b in input {
            self.state = match self.state {
                State::Data if b == IAC => State::Iac,
                State::Data => {
                    data.push(b);
                    State::Data
                }
                State::Iac => match b {
                    IAC => {
                        data.push(IAC);
                        State::Data
                    }
                    WILL | WONT | DO | DONT => State::Verb(b),
                    SB => State::Sb,
                    _ => State::Data,
                },
                State::Verb(verb) => {
                    self.negotiate(verb, b, reply);
                    State::Data
                }
                // Every option we agree to sends us no subnegotiation, so the payload is
                // dropped rather than buffered.
                State::Sb if b == IAC => State::SbIac,
                State::Sb => State::Sb,
                State::SbIac if b == SE => State::Data,
                State::SbIac => State::Sb,
            };
        }
    }

    /// Records a new window size, and appends a NAWS report to `reply` if NAWS is agreed.
    pub fn set_window_size(&mut self, cols: u16, rows: u16, reply: &mut Vec<u8>) {
        self.cols = cols;
        self.rows = rows;
        if self.client_on[usize::from(NAWS)] {
            self.naws(reply);
        }
    }

    /// Whether the server has agreed to echo, so the client must not echo locally.
    pub fn server_echoes(&self) -> bool {
        self.server_on[usize::from(ECHO)]
    }

    fn negotiate(&mut self, verb: u8, option: u8, reply: &mut Vec<u8>) {
        let i = usize::from(option);
        // Guards on the current state keep an already-settled option from being answered
        // again.
        match verb {
            WILL if !server_may(option) => reply.extend([IAC, DONT, option]),
            WILL if !self.server_on[i] => {
                self.server_on[i] = true;
                reply.extend([IAC, DO, option]);
            }
            WONT if self.server_on[i] => {
                self.server_on[i] = false;
                reply.extend([IAC, DONT, option]);
            }
            DO if !client_will(option) => reply.extend([IAC, WONT, option]),
            DO if !self.client_on[i] => {
                self.client_on[i] = true;
                reply.extend([IAC, WILL, option]);
                if option == NAWS {
                    self.naws(reply);
                }
            }
            DONT if self.client_on[i] => {
                self.client_on[i] = false;
                reply.extend([IAC, WONT, option]);
            }
            _ => {}
        }
    }

    /// `IAC SB NAWS <w16> <h16> IAC SE`, big-endian, 0xFF doubled in the payload.
    fn naws(&self, reply: &mut Vec<u8>) {
        reply.extend([IAC, SB, NAWS]);
        let [w1, w2] = self.cols.to_be_bytes();
        let [h1, h2] = self.rows.to_be_bytes();
        escape(&[w1, w2, h1, h2], reply);
        reply.extend([IAC, SE]);
    }
}

/// The server side of a telnet connection, as the proxy's seat speaks to MudPlay (design
/// §2.1). It mirrors silvermere's option table (`silvermere-net/src/telnet/options.rs`) so
/// a client can't tell the seat from silvermere: [`ServerCodec::OFFERS`] on accept, `DO`
/// agreed only for ECHO, SGA and BINARY, `WILL` only for NAWS, everything else refused, and
/// a refusal taken silently. The same answer isn't repeated for an option until a refusal
/// resets it, so a client that acknowledges acknowledgements can't start a loop.
#[derive(Debug, Clone)]
pub struct ServerCodec {
    state: State,
    /// The last answer given for each option, until a refusal resets it.
    last_reply: [Option<u8>; 256],
    /// A NAWS payload being read.
    sb: Vec<u8>,
    window: Option<(u16, u16)>,
}

impl Default for ServerCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerCodec {
    /// Sent on accept, in silvermere's order.
    pub const OFFERS: [u8; 12] = [
        IAC, WILL, ECHO, IAC, WILL, SGA, IAC, WILL, BINARY, IAC, DO, NAWS,
    ];

    pub fn new() -> Self {
        Self {
            state: State::Data,
            last_reply: [None; 256],
            sb: Vec::new(),
            window: None,
        }
    }

    /// The client's window, once it has sent NAWS.
    pub fn window(&self) -> Option<(u16, u16)> {
        self.window
    }

    /// Decodes `input` from the client, appending its data bytes to `data` and any answers
    /// to `reply`, which the caller writes back. A command split across two reads is
    /// completed by the second.
    pub fn decode(&mut self, input: &[u8], data: &mut Vec<u8>, reply: &mut Vec<u8>) {
        for &b in input {
            self.state = match self.state {
                State::Data if b == IAC => State::Iac,
                State::Data => {
                    data.push(b);
                    State::Data
                }
                State::Iac => match b {
                    IAC => {
                        data.push(IAC);
                        State::Data
                    }
                    WILL | WONT | DO | DONT => State::Verb(b),
                    SB => {
                        self.sb.clear();
                        State::Sb
                    }
                    _ => State::Data,
                },
                State::Verb(verb) => {
                    self.respond(verb, b, reply);
                    State::Data
                }
                State::Sb if b == IAC => State::SbIac,
                State::Sb => {
                    self.sb.push(b);
                    State::Sb
                }
                State::SbIac if b == SE => {
                    self.subnegotiation();
                    State::Data
                }
                State::SbIac => {
                    // `IAC IAC` inside a payload is one 0xFF.
                    self.sb.push(b);
                    State::Sb
                }
            };
        }
    }

    fn subnegotiation(&mut self) {
        if let [NAWS, w1, w2, h1, h2] = self.sb[..] {
            self.window = Some((u16::from_be_bytes([w1, w2]), u16::from_be_bytes([h1, h2])));
        }
    }

    fn respond(&mut self, verb: u8, option: u8, reply: &mut Vec<u8>) {
        let slot = &mut self.last_reply[usize::from(option)];
        let answer = match verb {
            DO if server_may(option) => WILL,
            DO => WONT,
            WILL if option == NAWS => DO,
            WILL => DONT,
            _ => {
                *slot = None;
                return;
            }
        };
        if *slot != Some(answer) {
            *slot = Some(answer);
            reply.extend([IAC, answer, option]);
        }
    }
}
