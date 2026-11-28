//! Names EVTC logs carry for players outside the squad: the WvW rank title on WvW maps, else the
//! elite specialization or profession name.
//!
//! EVTC logs decode these from the game's text (localized); the tables here are the English ones,
//! from the API (`/v2/wvw/ranks`, `/v2/specializations`).

/// Whether a MumbleLink map type is a WvW map: Eternal Battlegrounds (9), the three
/// borderlands (10-12), Fortune's Vale (13), Obsidian Sanctum (14) and the Edge of the Mists
/// (15); not the Armistice Bastion lounge (18). EVTC logs check the map def's type against 7 WvW
/// types; the exact set is not recovered.
pub fn is_wvw_map(map_type: u32) -> bool {
    (9..=15).contains(&map_type)
}

/// Lowest rank of each rank def id 1..=105 (`/v2/wvw/ranks` `min_rank`, strictly increasing,
/// a lower-bound table).
const MIN_RANK: [u16; 105] = [
    1, 5, 10, 15, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, //
    150, 180, 210, 240, 270, 300, 330, 360, 390, 420, 450, 480, 510, 540, 570, //
    620, 670, 720, 770, 820, 870, 920, 970, 1020, 1070, 1120, 1170, 1220, 1270, 1320, //
    1395, 1470, 1545, 1620, 1695, 1770, 1845, 1920, 1995, 2070, 2145, 2220, 2295, 2370,
    2445, //
    2545, 2645, 2745, 2845, 2945, 3045, 3145, 3245, 3345, 3445, 3545, 3645, 3745, 3845,
    3945, //
    4095, 4245, 4395, 4545, 4695, 4845, 4995, 5145, 5295, 5445, 5595, 5745, 5895, 6045,
    6195, //
    6445, 6695, 6945, 7195, 7445, 7695, 7945, 8195, 8445, 8695, 8945, 9195, 9445, 9695, 9945,
];
/// Title = tier + word: id `i` is `TIER[(i-1)/15] + WORD[(i-1)%15]`.
const TIER: [&str; 7] = [
    "",
    "Bronze ",
    "Silver ",
    "Gold ",
    "Platinum ",
    "Mithril ",
    "Diamond ",
];
const WORD: [&str; 15] = [
    "Invader",
    "Assaulter",
    "Raider",
    "Recruit",
    "Scout",
    "Soldier",
    "Squire",
    "Footman",
    "Knight",
    "Major",
    "Colonel",
    "General",
    "Veteran",
    "Champion",
    "Legend",
];

/// The rank def id (1..=105) for a rank number, or the def index the DLL read when the rank is
/// 0; `None` without either.
pub fn wvw_rank_id(rank: u32, def: u8) -> Option<u8> {
    if rank == 0 {
        return (1..=MIN_RANK.len() as u8).contains(&def).then_some(def);
    }
    let n = MIN_RANK.partition_point(|&m| u32::from(m) <= rank);
    u8::try_from(n).ok().filter(|&n| n > 0)
}

/// The title of a rank def id (1..=105), e.g. "Mithril Legend".
pub fn wvw_title(id: u8) -> Option<String> {
    let i = usize::from(id).checked_sub(1)?;
    Some(format!("{}{}", TIER.get(i / 15)?, WORD.get(i % 15)?))
}

/// The EVTC non-WvW name for a player: the elite spec's name, else the profession's; "" for an
/// unknown profession.
pub fn spec_name(prof: u32, elite: u32) -> &'static str {
    let elite = match elite {
        5 => "Druid",
        7 => "Daredevil",
        18 => "Berserker",
        27 => "Dragonhunter",
        34 => "Reaper",
        40 => "Chronomancer",
        43 => "Scrapper",
        48 => "Tempest",
        52 => "Herald",
        55 => "Soulbeast",
        56 => "Weaver",
        57 => "Holosmith",
        58 => "Deadeye",
        59 => "Mirage",
        60 => "Scourge",
        61 => "Spellbreaker",
        62 => "Firebrand",
        63 => "Renegade",
        64 => "Harbinger",
        65 => "Willbender",
        66 => "Virtuoso",
        67 => "Catalyst",
        68 => "Bladesworn",
        69 => "Vindicator",
        70 => "Mechanist",
        71 => "Specter",
        72 => "Untamed",
        73 => "Troubadour",
        74 => "Paragon",
        75 => "Amalgam",
        76 => "Ritualist",
        77 => "Antiquary",
        78 => "Galeshot",
        79 => "Conduit",
        80 => "Evoker",
        81 => "Luminary",
        _ => "",
    };
    if !elite.is_empty() {
        return elite;
    }
    match prof {
        1 => "Guardian",
        2 => "Warrior",
        3 => "Engineer",
        4 => "Ranger",
        5 => "Thief",
        6 => "Elementalist",
        7 => "Mesmer",
        8 => "Necromancer",
        9 => "Revenant",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wvw_map_types() {
        for t in 9..=15 {
            assert!(is_wvw_map(t), "{t}");
        }
        for t in [0, 1, 4, 5, 8, 16, 18] {
            assert!(!is_wvw_map(t), "{t}");
        }
    }

    #[test]
    fn rank_to_id_by_threshold() {
        assert_eq!(wvw_rank_id(0, 0), None);
        assert_eq!(wvw_rank_id(1, 0), Some(1));
        assert_eq!(wvw_rank_id(4, 0), Some(1));
        assert_eq!(wvw_rank_id(5, 0), Some(2));
        assert_eq!(wvw_rank_id(149, 0), Some(15));
        assert_eq!(wvw_rank_id(150, 0), Some(16));
        assert_eq!(wvw_rank_id(6000, 0), Some(88));
        assert_eq!(wvw_rank_id(9944, 0), Some(104));
        assert_eq!(wvw_rank_id(9945, 0), Some(105));
        assert_eq!(wvw_rank_id(250_000, 0), Some(105));
    }

    /// The rank number wins; the def index stands in when no rank was read.
    #[test]
    fn def_index_is_the_fallback() {
        assert_eq!(wvw_rank_id(6000, 12), Some(88));
        assert_eq!(wvw_rank_id(0, 12), Some(12));
        assert_eq!(wvw_rank_id(0, 105), Some(105));
        assert_eq!(wvw_rank_id(0, 106), None);
    }

    #[test]
    fn titles() {
        assert_eq!(wvw_title(1).as_deref(), Some("Invader"));
        assert_eq!(wvw_title(15).as_deref(), Some("Legend"));
        assert_eq!(wvw_title(16).as_deref(), Some("Bronze Invader"));
        assert_eq!(wvw_title(88).as_deref(), Some("Mithril Veteran"));
        assert_eq!(wvw_title(90).as_deref(), Some("Mithril Legend"));
        assert_eq!(wvw_title(105).as_deref(), Some("Diamond Legend"));
        assert_eq!(wvw_title(0), None);
        assert_eq!(wvw_title(106), None);
    }

    #[test]
    fn spec_names() {
        assert_eq!(spec_name(1, 0), "Guardian");
        assert_eq!(spec_name(1, 62), "Firebrand");
        assert_eq!(spec_name(9, 0), "Revenant");
        assert_eq!(spec_name(8, 76), "Ritualist");
        assert_eq!(spec_name(6, 80), "Evoker");
        assert_eq!(spec_name(1, 81), "Luminary");
        assert_eq!(spec_name(4, 5), "Druid");
        // An elite id the table does not know: the profession.
        assert_eq!(spec_name(2, 200), "Warrior");
        assert_eq!(spec_name(0, 0), "");
        assert_eq!(spec_name(10, 0), "");
    }
}
