//! The spike for design §10.4: a real silvermere in-process, reached over TCP with
//! mudparty's own telnet codec and parser, and logged in to the game prompt.

use std::time::Duration;

use mp_parse::{Event, Item, Reader, Statline};
use mp_silvermere::{Player, Silvermere};
use mp_telnet::ClientCodec;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(30);

/// Reads until `stop` says so, answering telnet and handing every item to `on_item`, which
/// may return a line to send.
async fn drive(
    stream: &mut TcpStream,
    reader: &mut Reader,
    mut on_item: impl FnMut(&Item) -> Option<String>,
    mut stop: impl FnMut(&Item) -> bool,
) -> Vec<Item> {
    let mut telnet = ClientCodec::new(80, 24);
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = timeout(WAIT, stream.read(&mut buf))
            .await
            .expect("silvermere answered in time")
            .expect("read");
        assert!(n > 0, "silvermere closed the connection; saw {seen:#?}");
        let (mut data, mut reply) = (Vec::new(), Vec::new());
        telnet.decode(&buf[..n], &mut data, &mut reply);
        stream.write_all(&reply).await.unwrap();
        for item in reader.feed(&data) {
            if let Some(line) = on_item(&item) {
                stream
                    .write_all(format!("{line}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            let done = stop(&item);
            seen.push(item);
            if done {
                return seen;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_character_logs_in_to_the_game_prompt() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1").hp(35)])
        .await
        .expect("silvermere starts");
    let mut stream = TcpStream::connect(sm.telnet_addr()).await.unwrap();
    // A blank class has no mana, so the Full statline prints `[HP=35]:`.
    let mut reader = Reader::new(Some(Statline::compile("[HP=%h]:%r").unwrap()));
    let seen = drive(
        &mut stream,
        &mut reader,
        |item| match item {
            Item::Event(Event::UsernamePrompt) => Some("Aldric".into()),
            Item::Event(Event::PasswordPrompt) => Some("aldric1".into()),
            Item::Event(Event::MainMenu) => Some("M".into()),
            Item::Event(Event::MudMenu) => Some("E".into()),
            _ => None,
        },
        |item| matches!(item, Item::Prompt(_)),
    )
    .await;
    let Some(Item::Prompt(v)) = seen.last() else {
        unreachable!()
    };
    assert_eq!(v.hp, 35);
    for want in [
        Event::UsernamePrompt,
        Event::PasswordPrompt,
        Event::MainMenu,
        Event::MudMenu,
    ] {
        assert!(
            seen.contains(&Item::Event(want.clone())),
            "no {want:?} in {seen:#?}"
        );
    }
    sm.stop().await.expect("silvermere stops cleanly");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_is_refused() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let mut stream = TcpStream::connect(sm.telnet_addr()).await.unwrap();
    let mut reader = Reader::new(None);
    let seen = drive(
        &mut stream,
        &mut reader,
        |item| match item {
            Item::Event(Event::UsernamePrompt) => Some("Aldric".into()),
            Item::Event(Event::PasswordPrompt) => Some("wrong".into()),
            _ => None,
        },
        |item| matches!(item, Item::Event(Event::InvalidCredentials)),
    )
    .await;
    assert_eq!(seen.last(), Some(&Item::Event(Event::InvalidCredentials)));
    sm.stop().await.expect("silvermere stops cleanly");
}
