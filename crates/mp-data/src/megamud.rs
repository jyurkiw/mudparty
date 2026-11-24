//! MegaMud's room hash (design §8.1), as MudPlay reproduces it (`MegaMudHash.cs`): a 3-digit
//! name hash and a 5-digit exits code, 8 hex characters in all.

use crate::{RoomExit, ShowsAs};

/// The low 12 bits of the sum of (1-based position × character code), as 3 hex digits;
/// `FFF` for an empty name.
pub fn name_hash(name: &str) -> String {
    if name.is_empty() {
        return "FFF".into();
    }
    let sum = name.chars().enumerate().fold(0u32, |acc, (i, c)| {
        acc.wrapping_add((i as u32 + 1).wrapping_mul(c as u32))
    });
    format!("{:03X}", sum & 0xFFF)
}

/// The exits code. Directions in silvermere's order (N S E W NE NW SE SW U D) pair up into
/// the five digits from the right: N and S in the last, E and W the one before, and so on
/// to U and D in the first. The first of each pair adds 1 and the second 4; a door counts
/// double; a hidden exit, or one never shown, counts nothing.
pub fn exits_code(exits: &[RoomExit]) -> String {
    let mut digits = [0u32; 5];
    for x in exits {
        let weight = match x.shows_as {
            ShowsAs::Plain => 1,
            ShowsAs::Door => 2,
            ShowsAs::Hidden | ShowsAs::None => continue,
        };
        if x.to_room == 0 || x.dir > 9 {
            continue;
        }
        let pair = usize::from(x.dir / 2);
        let value = if x.dir % 2 == 0 { 1 } else { 4 };
        digits[4 - pair] += value * weight;
    }
    digits.iter().map(|d| format!("{:X}", d & 0xF)).collect()
}

/// The 8-character hashExits for a room called `name` with `exits`.
pub fn mm_hash(name: &str, exits: &[RoomExit]) -> String {
    format!("{}{}", name_hash(name), exits_code(exits))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(dir: u8, shows_as: ShowsAs) -> RoomExit {
        RoomExit {
            dir,
            to_map: 1,
            to_room: 2,
            exit_type: 0,
            par1: 0,
            par2: 0,
            par3: 0,
            par4: 0,
            shows_as,
        }
    }

    // The values MudPlay's MegaMudHashTests pin.
    #[test]
    fn name_hashes_match_megamuds() {
        assert_eq!(name_hash(""), "FFF");
        assert_eq!(name_hash("Stone Street"), "E99");
        assert_eq!(name_hash("Start"), "66A");
        assert_eq!(name_hash("Town Square"), "9D4");
    }

    #[test]
    fn each_direction_has_its_own_digit() {
        let want = [
            "00001", "00004", "00010", "00040", "00100", "00400", "01000", "04000", "10000",
            "40000",
        ];
        for (dir, w) in want.iter().enumerate() {
            assert_eq!(
                exits_code(&[exit(dir as u8, ShowsAs::Plain)]),
                *w,
                "dir {dir}"
            );
        }
    }

    #[test]
    fn every_exit_fills_every_digit() {
        let all: Vec<_> = (0..10).map(|d| exit(d, ShowsAs::Plain)).collect();
        assert_eq!(exits_code(&all), "55555");
    }

    #[test]
    fn a_door_counts_double_and_a_hidden_exit_nothing() {
        assert_eq!(exits_code(&[exit(0, ShowsAs::Door)]), "00002");
        assert_eq!(
            exits_code(&[exit(0, ShowsAs::Hidden), exit(1, ShowsAs::Plain)]),
            "00004"
        );
        assert_eq!(exits_code(&[exit(0, ShowsAs::None)]), "00000");
    }
}
