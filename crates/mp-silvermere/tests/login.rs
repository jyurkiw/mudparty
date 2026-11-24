//! design §10.4: a real silvermere in-process, reached over TCP with mudparty's own telnet
//! codec and parser.

use mp_parse::{Event, Item};
use mp_silvermere::{Player, RawClient, Relay, Silvermere};

#[tokio::test(flavor = "multi_thread")]
async fn a_character_logs_in_to_the_game_prompt() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1").hp(35)])
        .await
        .unwrap();
    let c = RawClient::log_in(sm.telnet_addr(), "Aldric", "aldric1")
        .await
        .unwrap();
    // A blank class has no mana, so the Full statline prints `[HP=35]:`.
    let Some(Item::Prompt(v)) = c.seen.last() else {
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
            c.seen.contains(&Item::Event(want.clone())),
            "no {want:?} in {:#?}",
            c.seen
        );
    }
    sm.stop().await.expect("silvermere stops cleanly");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_is_refused() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let mut c = RawClient::connect(sm.telnet_addr()).await.unwrap();
    let got = c
        .answer_until(
            |item| match item {
                Item::Event(Event::UsernamePrompt) => Some("Aldric".into()),
                Item::Event(Event::PasswordPrompt) => Some("wrong".into()),
                _ => None,
            },
            |item| matches!(item, Item::Event(Event::InvalidCredentials)),
        )
        .await
        .unwrap();
    assert_eq!(got, Item::Event(Event::InvalidCredentials));
    sm.stop().await.expect("silvermere stops cleanly");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_carries_a_login_and_its_cut_drops_the_link() {
    let sm = Silvermere::start(&[Player::new("Aldric", "aldric1")])
        .await
        .unwrap();
    let relay = Relay::start(sm.telnet_addr()).await.unwrap();
    let mut c = RawClient::log_in(relay.addr(), "Aldric", "aldric1")
        .await
        .unwrap();
    relay.cut();
    c.read_to_close().await.unwrap();
    sm.stop().await.expect("silvermere stops cleanly");
}
