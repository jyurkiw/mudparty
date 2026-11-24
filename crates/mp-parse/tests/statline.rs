//! The prompt matcher, generated from a statline template and checked against the prompt
//! silvermere renders from the same template (design §6.1).

use mp_parse::{Rest, Statline, Vitals};
use silvermere_text::{Palette, PromptInput, StatlineCtx, render_prompt};
use silvermere_types::enums::{Magery, StatlineMode};

const BOT: &str = "[HP=%h/%H,MA=%m/%M]:%r";

struct Char {
    hp: i32,
    maxhp: i32,
    mana: i32,
    maxmana: i32,
    resting: bool,
    meditating: bool,
}

/// The prompt silvermere sends, as plain text, for `template` in full custom mode.
fn rendered(template: &str, c: &Char, palette: u8) -> String {
    let palette = Palette::new(palette).unwrap();
    let ctx = StatlineCtx {
        hp: c.hp,
        maxhp: c.maxhp,
        mana: c.mana,
        maxmana: c.maxmana,
        exp: 1234,
        exp_needed: Some(500),
        wealth_copper: 77,
        first_name: "Ermias",
        evil_warn: false,
        resting: c.resting,
        meditating: c.meditating,
        pronoun_possessive: "his",
        pronoun_object: "him",
        title: None,
        palette,
    };
    let input = PromptInput {
        hp: c.hp,
        maxhp: c.maxhp,
        mana: c.mana,
        maxmana: c.maxmana,
        magery: Some(Magery::Mage),
        resting: c.resting,
        meditating: c.meditating,
        mode: StatlineMode::FullCustom,
        template: Some(template),
        palette,
        sysop_debug: None,
    };
    let bytes = render_prompt(&input, &ctx);
    let mut x = mp_text::Extractor::new();
    assert!(x.feed(&bytes).is_empty(), "a prompt has no line end");
    x.pending().expect("the prompt is pending").plain
}

fn standing(hp: i32, mana: i32) -> Char {
    Char {
        hp,
        maxhp: 120,
        mana,
        maxmana: 40,
        resting: false,
        meditating: false,
    }
}

#[test]
fn the_bot_statline_reads_current_and_maximum_values_in_every_palette() {
    let s = Statline::compile(BOT).unwrap();
    for palette in 0..4 {
        let plain = rendered(BOT, &standing(87, 12), palette);
        let (v, end) = s
            .read(&plain)
            .unwrap_or_else(|| panic!("no match: {plain:?}"));
        assert_eq!(
            v,
            Vitals {
                hp: 87,
                max_hp: Some(120),
                mp: Some(12),
                max_mp: Some(40),
                rest: Rest::Standing,
            }
        );
        assert_eq!(end, plain.len());
    }
}

#[test]
fn the_rest_flag_is_read() {
    let s = Statline::compile(BOT).unwrap();
    let mut c = standing(50, 10);
    c.resting = true;
    let (v, _) = s.read(&rendered(BOT, &c, 0)).unwrap();
    assert_eq!(v.rest, Rest::Resting);
    c.resting = false;
    c.meditating = true;
    let (v, _) = s.read(&rendered(BOT, &c, 0)).unwrap();
    assert_eq!(v.rest, Rest::Meditating);
}

#[test]
fn hit_points_below_zero_are_read() {
    let s = Statline::compile(BOT).unwrap();
    let (v, _) = s.read(&rendered(BOT, &standing(-4, 0), 0)).unwrap();
    assert_eq!(v.hp, -4);
}

#[test]
fn four_digit_values_are_read() {
    let s = Statline::compile(BOT).unwrap();
    let mut c = standing(1234, 1500);
    c.maxhp = 2000;
    c.maxmana = 1600;
    let (v, _) = s.read(&rendered(BOT, &c, 0)).unwrap();
    assert_eq!(
        (v.hp, v.max_hp, v.mp, v.max_mp),
        (1234, Some(2000), Some(1500), Some(1600))
    );
}

#[test]
fn colour_tokens_print_nothing_the_matcher_must_see() {
    let t = "%f2[%dHP=%h/%H%N MA=%m/%M]:%r";
    let s = Statline::compile(t).unwrap();
    let (v, _) = s.read(&rendered(t, &standing(87, 12), 2)).unwrap();
    assert_eq!((v.hp, v.mp), (87, Some(12)));
}

#[test]
fn a_template_without_mana_reads_hp_only() {
    let t = "[HP=%h/%H]:%r";
    let s = Statline::compile(t).unwrap();
    let (v, _) = s.read(&rendered(t, &standing(30, 0), 0)).unwrap();
    assert_eq!(
        (v.hp, v.max_hp, v.mp, v.max_mp),
        (30, Some(120), None, None)
    );
}

#[test]
fn the_match_is_anchored_at_the_start_and_reports_where_it_ends() {
    let s = Statline::compile(BOT).unwrap();
    let line = "[HP=87/120,MA=12/40]:--- Telepath Sent to Raijin ---";
    let (_, end) = s.read(line).unwrap();
    assert_eq!(&line[end..], "--- Telepath Sent to Raijin ---");
    assert!(s.read("You see [HP=87/120,MA=12/40]:").is_none());
}

#[test]
fn a_template_that_breaks_the_line_is_refused() {
    assert!(Statline::compile("[HP=%h]%n[MA=%m]:").is_err());
}

#[test]
fn a_template_with_no_hit_points_is_refused() {
    assert!(Statline::compile("[MA=%m]:").is_err());
}

/// The prompt silvermere sends in its default (Full) mode.
fn rendered_default(c: &Char, magery: Option<Magery>, palette: u8) -> String {
    let palette = Palette::new(palette).unwrap();
    let ctx = StatlineCtx {
        hp: c.hp,
        maxhp: c.maxhp,
        mana: c.mana,
        maxmana: c.maxmana,
        exp: 0,
        exp_needed: Some(0),
        wealth_copper: 0,
        first_name: "Aldric",
        evil_warn: false,
        resting: c.resting,
        meditating: c.meditating,
        pronoun_possessive: "his",
        pronoun_object: "him",
        title: None,
        palette,
    };
    let input = PromptInput {
        hp: c.hp,
        maxhp: c.maxhp,
        mana: c.mana,
        maxmana: c.maxmana,
        magery,
        resting: c.resting,
        meditating: c.meditating,
        mode: StatlineMode::Full,
        template: None,
        palette,
        sysop_debug: None,
    };
    let bytes = render_prompt(&input, &ctx);
    let mut x = mp_text::Extractor::new();
    assert!(x.feed(&bytes).is_empty());
    x.pending().expect("the prompt is pending").plain
}

fn read_any(plain: &str) -> Option<(Vitals, usize)> {
    Statline::silvermere_defaults()
        .iter()
        .find_map(|s| s.read(plain))
}

#[test]
fn silvermere_default_prompts_are_read_for_every_kind_of_class() {
    for palette in 0..4 {
        // A mana class: `[HP=87/MA=12]:`
        let (v, _) = read_any(&rendered_default(
            &standing(87, 12),
            Some(Magery::Mage),
            palette,
        ))
        .unwrap();
        assert_eq!((v.hp, v.mp, v.max_hp), (87, Some(12), None));
        // A kai class: `[HP=87/KAI=12]:`
        let (v, _) = read_any(&rendered_default(
            &standing(87, 12),
            Some(Magery::Kai),
            palette,
        ))
        .unwrap();
        assert_eq!((v.hp, v.mp), (87, Some(12)));
        // No mana: `[HP=35]:`
        let mut c = standing(35, 0);
        c.maxmana = 0;
        let (v, _) = read_any(&rendered_default(&c, None, palette)).unwrap();
        assert_eq!((v.hp, v.mp), (35, None));
    }
}

#[test]
fn silvermere_default_prompts_carry_the_rest_mark() {
    let mut c = standing(50, 10);
    c.resting = true;
    let (v, _) = read_any(&rendered_default(&c, Some(Magery::Mage), 0)).unwrap();
    assert_eq!(v.rest, Rest::Resting);
    c.maxmana = 0;
    let (v, _) = read_any(&rendered_default(&c, None, 0)).unwrap();
    assert_eq!(v.rest, Rest::Resting);
}
