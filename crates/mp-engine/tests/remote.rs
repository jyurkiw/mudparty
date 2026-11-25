//! @ commands (design §6.3), as pure logic: who may send them, on which channel the reply
//! goes, MudPlay's reply shapes, and the hard blocks. Replies are checked against the
//! regexes MudPlay reads them back with, copied here as fixtures.

use mp_config::{Automation, Channel};
use mp_engine::{
    Action, AutoSwitch, BotView, Effect, ExpKnown, Query, Remote, RemoteSettings, RoomInfo,
    exp_reply, status_reply, where_reply,
};
use mp_parse::{Event, Item, Rest, Vitals};
use regex::Regex;

fn s(v: &str) -> String {
    v.to_owned()
}

fn settings() -> RemoteSettings {
    RemoteSettings {
        name: s("Ermias"),
        allow: vec![s("Raijin")],
        channels: vec![Channel::Telepath, Channel::Gangpath],
        warn_denied: false,
        version: s("0.1.0"),
    }
}

fn view() -> BotView {
    BotView {
        vitals: Some(Vitals {
            hp: 43,
            max_hp: Some(43),
            mp: Some(15),
            max_mp: Some(34),
            rest: Rest::Standing,
        }),
        automation: Automation::default(),
        all_off: false,
        stopped: false,
        following: None,
    }
}

fn tele(who: &str, text: &str) -> Item {
    Item::Event(Event::Telepath {
        who: s(who),
        text: s(text),
    })
}

fn gang(who: &str, text: &str) -> Item {
    Item::Event(Event::Gangpath {
        who: s(who),
        text: s(text),
    })
}

/// The lines the effects put on the wire, replies included.
fn wire(effects: &[Effect]) -> Vec<Action> {
    effects.iter().filter_map(Effect::action).collect()
}

fn reply_to_raijin(text: &str) -> Action {
    Action::Telepath {
        to: s("Raijin"),
        text: format!("{{{text}}}"),
    }
}

fn run(item: Item) -> Vec<Effect> {
    Remote::new(settings()).on_item(&item, &view())
}

// MudPlay's PartyPoller.HealthReply, which reads a member's @health answer.
const MUDPLAY_HEALTH: &str = r"(?i)^\{?HP=(?<hp>-?\d+)/(?<hpmax>\d+)(?:,(?<mpkind>MA|KAI)=(?<mp>-?\d+)/(?<mpmax>\d+))?(?:,[^}]*)?\}?\s*$";

#[test]
fn health_answers_in_mudplays_shape() {
    let fx = run(tele("Raijin", "@health"));
    assert_eq!(wire(&fx), vec![reply_to_raijin("HP=43/43,MA=15/34")]);
    let re = Regex::new(MUDPLAY_HEALTH).unwrap();
    let c = re
        .captures("{HP=43/43,MA=15/34}")
        .expect("MudPlay reads it");
    assert_eq!(
        (&c["hp"], &c["hpmax"], &c["mp"], &c["mpmax"]),
        ("43", "43", "15", "34")
    );
}

#[test]
fn health_says_resting_and_leaves_out_mana_a_class_has_none_of() {
    let mut v = view();
    v.vitals = Some(Vitals {
        hp: 20,
        max_hp: Some(40),
        mp: None,
        max_mp: None,
        rest: Rest::Resting,
    });
    let fx = Remote::new(settings()).on_item(&tele("Raijin", "@health"), &v);
    assert_eq!(wire(&fx), vec![reply_to_raijin("HP=20/40, Resting")]);
    assert!(
        Regex::new(MUDPLAY_HEALTH)
            .unwrap()
            .is_match("{HP=20/40, Resting}")
    );
}

#[test]
fn a_gangpath_is_answered_on_the_gang_channel_and_our_own_echo_ignored() {
    let fx = run(gang("Raijin", "@health"));
    assert_eq!(wire(&fx), vec![Action::Send(s("bg {HP=43/43,MA=15/34}"))]);
    assert!(
        run(gang("Ermias", "@health")).is_empty(),
        "our own gangpath echo"
    );
}

#[test]
fn a_channel_turned_off_is_not_read() {
    let mut set = settings();
    set.channels = vec![Channel::Telepath];
    assert!(
        Remote::new(set)
            .on_item(&gang("Raijin", "@health"), &view())
            .is_empty()
    );
}

#[test]
fn a_sender_not_allowed_gets_nothing_or_the_failure_reply() {
    assert!(run(tele("Mallory", "@health")).is_empty());
    let mut set = settings();
    set.warn_denied = true;
    let fx = Remote::new(set).on_item(&tele("Mallory", "@health"), &view());
    assert_eq!(
        wire(&fx),
        vec![Action::Telepath {
            to: s("Mallory"),
            text: s("{command invalid or not allowed}"),
        }]
    );
}

#[test]
fn chat_and_unknown_commands_are_ignored() {
    assert!(run(tele("Raijin", "hello there")).is_empty());
    assert!(run(tele("Raijin", "@because I said so")).is_empty());
}

#[test]
fn the_command_word_ignores_case_and_a_trailing_bang() {
    assert_eq!(wire(&run(tele("Raijin", "@HEALTH!"))).len(), 1);
}

#[test]
fn version_names_mudparty() {
    assert_eq!(
        wire(&run(tele("Raijin", "@version"))),
        vec![reply_to_raijin("mudparty 0.1.0")]
    );
}

#[test]
fn help_lists_the_commands_then_says_how_to_ask_about_one() {
    let fx = wire(&run(tele("Raijin", "@help")));
    assert!(fx.len() >= 2, "{fx:?}");
    assert_eq!(
        fx.last(),
        Some(&reply_to_raijin("(type @help <command> for its syntax)"))
    );
    let Action::Telepath { text, .. } = &fx[0] else {
        panic!("{fx:?}")
    };
    assert!(text.contains("@health") && text.contains("@do"), "{text}");
    for a in &fx {
        let Action::Telepath { text, .. } = a else {
            panic!("{a:?}")
        };
        assert!(
            text.len() <= 202,
            "a reply of at most 200 characters: {text}"
        );
    }
    assert_eq!(
        wire(&run(tele("Raijin", "@help @kill"))),
        vec![reply_to_raijin(
            "@kill <target> - makes the named monster the target until it dies"
        )]
    );
    let mut set = settings();
    set.warn_denied = true;
    let fx = Remote::new(set).on_item(&tele("Raijin", "@help @fly"), &view());
    assert_eq!(
        wire(&fx),
        vec![reply_to_raijin(
            "no such command '@fly' - try @help for the list"
        )]
    );
}

#[test]
fn do_sends_the_command_and_answers_ok() {
    let fx = run(tele("Raijin", "@do  get   sword"));
    assert_eq!(
        wire(&fx),
        vec![Action::Send(s("get sword")), reply_to_raijin("ok")]
    );
}

#[test]
fn do_never_sends_suicide_or_reroll_and_says_nothing() {
    let mut set = settings();
    set.warn_denied = true;
    let r = Remote::new(set);
    assert!(
        r.on_item(&tele("Raijin", "@do suicide"), &view())
            .is_empty()
    );
    assert!(
        r.on_item(&tele("Raijin", "@do say reroll me"), &view())
            .is_empty()
    );
    assert!(
        r.on_item(&tele("Raijin", "@help rerolling"), &view()).len() == 1,
        "@help is a query"
    );
}

#[test]
fn kill_retargets_silently() {
    let fx = run(tele("Raijin", "@kill giant rat"));
    assert_eq!(fx, vec![Effect::Kill(s("giant rat"))]);
    assert!(wire(&fx).is_empty(), "no reply on success");
}

#[test]
fn auto_toggles_flip_or_set_and_say_the_new_state() {
    let fx = run(tele("Raijin", "@auto-combat"));
    assert_eq!(
        fx,
        vec![
            Effect::SetAuto(AutoSwitch::Combat, false),
            Effect::Reply {
                channel: Channel::Telepath,
                to: s("Raijin"),
                text: s("@auto-combat: off"),
            },
        ]
    );
    let fx = run(tele("Raijin", "@auto-rest on"));
    assert_eq!(fx[0], Effect::SetAuto(AutoSwitch::Rest, true));
    assert_eq!(wire(&fx), vec![reply_to_raijin("@auto-rest: on")]);
    let fx = run(tele("Raijin", "@auto-nuke OFF"));
    assert_eq!(fx[0], Effect::SetAuto(AutoSwitch::Nuke, false));
    let fx = run(tele("Raijin", "@auto-all off"));
    assert_eq!(fx[0], Effect::AllOff(true));
    assert_eq!(wire(&fx), vec![reply_to_raijin("@auto-all: off")]);
}

#[test]
fn a_toggle_with_a_bad_argument_is_refused() {
    let mut set = settings();
    set.warn_denied = true;
    let fx = Remote::new(set).on_item(&tele("Raijin", "@auto-rest maybe"), &view());
    assert_eq!(wire(&fx), vec![reply_to_raijin("?")]);
}

#[test]
fn stop_pauses_and_says_so_once() {
    let fx = run(tele("Raijin", "@stop"));
    assert_eq!(fx[0], Effect::Stop(true));
    assert_eq!(wire(&fx), vec![reply_to_raijin("movement paused")]);
    let mut v = view();
    v.stopped = true;
    let fx = Remote::new(settings()).on_item(&tele("Raijin", "@stop"), &v);
    assert_eq!(wire(&fx), vec![reply_to_raijin("already @stopped")]);
    let fx = Remote::new(settings()).on_item(&tele("Raijin", "@rego"), &v);
    assert_eq!(fx[0], Effect::Stop(false));
    assert_eq!(wire(&fx), vec![reply_to_raijin("resuming")]);
}

#[test]
fn wait_and_ok_hold_and_release_without_a_reply() {
    assert_eq!(
        run(tele("Raijin", "@wait")),
        vec![Effect::Wait(s("Raijin"))]
    );
    assert_eq!(run(tele("Raijin", "@ok")), vec![Effect::Ok(s("Raijin"))]);
}

#[test]
fn invite_invites_the_sender_unless_the_bot_follows_someone() {
    assert_eq!(
        wire(&run(tele("Raijin", "@invite"))),
        vec![Action::Send(s("invite Raijin"))]
    );
    let mut set = settings();
    set.warn_denied = true;
    let mut v = view();
    v.following = Some(s("Kaelen"));
    let fx = Remote::new(set).on_item(&tele("Raijin", "@invite"), &v);
    assert_eq!(
        wire(&fx),
        vec![reply_to_raijin("I'm following Kaelen; denied.")]
    );
}

#[test]
fn join_joins_the_sender_or_the_leader_named() {
    assert_eq!(
        run(tele("Raijin", "@join")),
        vec![Effect::Join(s("Raijin"))]
    );
    assert_eq!(
        run(tele("Raijin", "@join Kaelen")),
        vec![Effect::Join(s("Kaelen"))]
    );
    let mut set = settings();
    set.warn_denied = true;
    let mut v = view();
    v.following = Some(s("Kaelen"));
    let r = Remote::new(set);
    let fx = r.on_item(&tele("Raijin", "@join"), &v);
    assert_eq!(
        wire(&fx),
        vec![reply_to_raijin("I'm following Kaelen; denied.")]
    );
    assert_eq!(
        wire(&r.on_item(&tele("Kaelen", "@join"), &v)),
        vec![Action::Telepath {
            to: s("Kaelen"),
            text: s("{command invalid or not allowed}"),
        }],
        "Kaelen isn't allowed"
    );
}

#[test]
fn a_join_from_the_leader_already_followed_is_taken() {
    let mut v = view();
    v.following = Some(s("Raijin"));
    assert_eq!(
        Remote::new(settings()).on_item(&tele("Raijin", "@join"), &v),
        vec![Effect::Join(s("Raijin"))]
    );
}

#[test]
fn replies_are_plain_ascii() {
    let mut set = settings();
    set.version = s("0.1.0–dé…");
    let fx = Remote::new(set).on_item(&tele("Raijin", "@version"), &view());
    assert_eq!(wire(&fx), vec![reply_to_raijin("mudparty 0.1.0-d?...")]);
}

// ----- the queries that need the game first (MVP 6b)

#[test]
fn where_status_and_exp_ask_the_game_before_answering() {
    for (cmd, q) in [
        ("@where", Query::Where),
        ("@status", Query::Status),
        ("@exp", Query::Exp),
    ] {
        assert_eq!(
            run(tele("Raijin", cmd)),
            vec![Effect::Ask {
                query: q,
                channel: Channel::Telepath,
                to: s("Raijin"),
            }],
            "{cmd}"
        );
    }
}

fn arena() -> RoomInfo {
    RoomInfo {
        name: s("Arena Practice Room"),
        exits: vec![8, 0],
    }
}

// MudPlay's WhereReplyParser.ReplyPattern.
const MUDPLAY_WHERE: &str = r"(?i)\{[^}]*\(map (\d+), room (\d+)\)";

#[test]
fn where_names_the_room_its_key_and_exits_in_mudplays_shape() {
    let text = where_reply((1, 206), Some(&arena()));
    assert_eq!(
        text,
        "Arena Practice Room (map 1, room 206); exits: north, up"
    );
    let braced = format!("{{{text}}}");
    let re = Regex::new(MUDPLAY_WHERE).unwrap();
    let c = re.captures(&braced).expect("MudPlay reads it");
    assert_eq!((&c[1], &c[2]), ("1", "206"));
    assert_eq!(
        where_reply((1, 9999), None),
        "map 1, room 9999 (not in map data)"
    );
    let shut = RoomInfo {
        name: String::new(),
        exits: vec![],
    };
    assert_eq!(
        where_reply((1, 5), Some(&shut)),
        "??? (map 1, room 5); exits: none"
    );
}

#[test]
fn status_says_what_where_and_what_ails() {
    assert_eq!(
        status_reply("idle", Some(((1, 206), Some(&arena()))), &[]),
        "idle; Arena Practice Room (map 1, room 206); no ailments"
    );
    assert_eq!(
        status_reply("fighting", None, &["poisoned"]),
        "fighting; location unknown; ailments: poisoned"
    );
}

#[test]
fn exp_is_megamuds_progress_line() {
    let known = ExpKnown {
        level: 1,
        left: 1304,
    };
    assert_eq!(
        exp_reply(0, 0.0, None),
        "Made: 0  Rate: unknown (type exp for needed + time to level)"
    );
    assert_eq!(
        exp_reply(0, 0.0, Some(known)),
        "Made: 0  Needed: 1,304 (L2)  Rate: unknown"
    );
    assert_eq!(
        exp_reply(1500, 652.0, Some(known)),
        "Made: 1,500  Needed: 1,304 (L2)  Rate: 652/hr  Will level in: 2h 0m"
    );
    assert_eq!(
        exp_reply(250_000, 180_000.0, Some(ExpKnown { level: 9, left: 0 })),
        "Made: 250,000  Needed: 0 (L10)  Rate: 180 k/hr  Will level in: ready to level"
    );
    assert_eq!(
        exp_reply(
            9,
            1_240_000.0,
            Some(ExpKnown {
                level: 9,
                left: 2_500_000
            })
        ),
        "Made: 9  Needed: 2,500,000 (L10)  Rate: 1.2 m/hr  Will level in: 2h 0m"
    );
}
