//! Telnet for mudparty: the IAC codec and option negotiation.
//!
//! [`ClientCodec`] is the client side, as a session or the interactive client speaks to
//! silvermere. silvermere offers `WILL ECHO`, `WILL SGA`, `WILL BINARY` and `DO NAWS`
//! (`silvermere-net/src/telnet/options.rs`); the client agrees to exactly those and refuses
//! the rest. Each answer is given only when it changes an option's state, so two sides that
//! acknowledge acknowledgements never loop.

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
