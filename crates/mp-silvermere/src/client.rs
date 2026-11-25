//! A hand-driven game client for tests: what a player at a terminal would do, with
//! mudparty's own telnet codec and parser.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, bail};
use mp_parse::{Event, Item, Reader, Statline};
use mp_telnet::ClientCodec;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// How long a test waits for silvermere before failing.
pub const WAIT: Duration = Duration::from_secs(30);

/// mudparty's bot statline (`mp_config::BOT_STATLINE`).
const BOT_STATLINE: &str = "[HP=%h/%H,MA=%m/%M]:%r";

/// The prompts a test client reads: silvermere's defaults, and the bot statline. silvermere
/// saves a character's statline with the character, so one a bot has played comes back with
/// the bot's statline on its next login.
fn prompt_shapes() -> Vec<Statline> {
    let mut shapes = Statline::silvermere_defaults();
    shapes.push(Statline::compile(BOT_STATLINE).expect("the bot statline compiles"));
    shapes
}

/// A connection to the game, read item by item.
pub struct RawClient {
    stream: TcpStream,
    telnet: ClientCodec,
    reader: Reader,
    /// Everything read so far.
    pub seen: Vec<Item>,
    /// Every data byte read so far, telnet commands taken out.
    pub data: Vec<u8>,
}

impl RawClient {
    pub async fn connect(addr: SocketAddr) -> anyhow::Result<Self> {
        Ok(Self {
            stream: TcpStream::connect(addr).await?,
            telnet: ClientCodec::new(80, 24),
            reader: Reader::with_statlines(prompt_shapes()),
            seen: Vec::new(),
            data: Vec::new(),
        })
    }

    /// Connects and logs `name` in with `password`, answering Y to a takeover, up to the game
    /// prompt.
    pub async fn log_in(addr: SocketAddr, name: &str, password: &str) -> anyhow::Result<Self> {
        let mut c = Self::connect(addr).await?;
        c.answer_until(
            |item| match item {
                Item::Event(Event::UsernamePrompt) => Some(name.to_owned()),
                Item::Event(Event::PasswordPrompt) => Some(password.to_owned()),
                Item::Event(Event::TakeoverQuestion) => Some("Y".into()),
                Item::Event(Event::MainMenu) => Some("M".into()),
                Item::Event(Event::MudMenu) => Some("E".into()),
                _ => None,
            },
            |item| matches!(item, Item::Prompt(_)),
        )
        .await
        .with_context(|| format!("logging {name} in"))?;
        Ok(c)
    }

    /// Sends one line.
    pub async fn send(&mut self, line: &str) -> anyhow::Result<()> {
        let mut out = Vec::new();
        mp_telnet::escape(format!("{line}\r\n").as_bytes(), &mut out);
        self.stream.write_all(&out).await?;
        Ok(())
    }

    /// Reads until an item satisfies `stop`, sending what `answer` returns for each item on
    /// the way. Fails if the game closes the link or says nothing for [`WAIT`].
    pub async fn answer_until(
        &mut self,
        mut answer: impl FnMut(&Item) -> Option<String>,
        mut stop: impl FnMut(&Item) -> bool,
    ) -> anyhow::Result<Item> {
        let mut buf = [0u8; 4096];
        loop {
            let Ok(read) = timeout(WAIT, self.stream.read(&mut buf)).await else {
                bail!("silvermere said nothing more in time; saw {:#?}", self.seen);
            };
            let n = read?;
            if n == 0 {
                bail!("silvermere closed the link; saw {:#?}", self.seen);
            }
            let (mut data, mut reply) = (Vec::new(), Vec::new());
            self.telnet.decode(&buf[..n], &mut data, &mut reply);
            self.stream.write_all(&reply).await?;
            self.data.extend_from_slice(&data);
            for item in self.reader.feed(&data) {
                if let Some(line) = answer(&item) {
                    self.send(&line).await?;
                }
                self.seen.push(item.clone());
                if stop(&item) {
                    return Ok(item);
                }
            }
        }
    }

    /// Reads until an item satisfies `stop`, sending nothing.
    pub async fn read_until(&mut self, stop: impl FnMut(&Item) -> bool) -> anyhow::Result<Item> {
        self.answer_until(|_| None, stop).await
    }

    /// Reads until the game closes the link, or [`WAIT`] passes.
    pub async fn read_to_close(&mut self) -> anyhow::Result<()> {
        let mut buf = [0u8; 4096];
        loop {
            let n = timeout(WAIT, self.stream.read(&mut buf))
                .await
                .context("the link stayed open")??;
            if n == 0 {
                return Ok(());
            }
            let (mut data, mut reply) = (Vec::new(), Vec::new());
            self.telnet.decode(&buf[..n], &mut data, &mut reply);
            self.data.extend_from_slice(&data);
            self.seen.extend(self.reader.feed(&data));
        }
    }
}
