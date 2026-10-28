//! The build layer on top of the spells: magic schools and the statuses they leave on the horde,
//! runes that change how one spell behaves, talents (and duo talents that need two schools),
//! relics from elites and bosses, racial traits and each class's own resource.
//!
//! Every school does something on its own, so any two spells already talk to each other through
//! the enemy they share:
//!
//! - Frost chills; five chill freeze, and a frozen enemy takes more from everything (Shatter).
//! - Fire ignites: a rolling burn that grows with each fire hit.
//! - Nature builds static; three static discharge into the enemy's neighbours.
//! - Shadow hits harder for every damage-over-time on its target (Shadow Weaving).
//! - Holy judges; a judged enemy takes more physical damage and heals the hero on death.
//! - Physical sunders; every stack makes the enemy take more from all sources.
//! - Arcane charges the hero: every arcane cast raises all damage for a while.

use super::data::Kind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum School {
    Physical,
    Holy,
    Fire,
    Nature,
    Frost,
    Shadow,
    Arcane,
}

impl School {
    pub const ALL: [School; 7] = [
        School::Physical,
        School::Holy,
        School::Fire,
        School::Nature,
        School::Frost,
        School::Shadow,
        School::Arcane,
    ];

    pub const fn bit(self) -> u8 {
        1 << self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            School::Physical => "Physical",
            School::Holy => "Holy",
            School::Fire => "Fire",
            School::Nature => "Nature",
            School::Frost => "Frost",
            School::Shadow => "Shadow",
            School::Arcane => "Arcane",
        }
    }

    /// The floating-number colour a hit of this school prints in.
    pub fn color(self) -> u32 {
        match self {
            School::Physical => 0xFFFF_FFFF,
            School::Holy => 0xFFFF_E680,
            School::Fire => 0xFFFF_8020,
            School::Nature => 0xFF60_FF60,
            School::Frost => 0xFF80_D0FF,
            School::Shadow => 0xFFB0_70FF,
            School::Arcane => 0xFFFF_80FF,
        }
    }
}

pub(crate) fn has(mask: u8, s: School) -> bool {
    mask & s.bit() != 0
}

/// The first school in a mask, for colour and naming.
pub(crate) fn primary(mask: u8) -> School {
    School::ALL
        .into_iter()
        .find(|s| has(mask, *s))
        .unwrap_or(School::Physical)
}

/// Each spell's school, as WoW has it; a few carry a second (Holy Fire burns).
pub(crate) fn school_of(key: &str) -> u8 {
    use School::*;
    let one = |s: School| s.bit();
    match key {
        "frostbolt" | "frost_nova" | "blizzard" | "cone_of_cold" | "frost_shock" => one(Frost),
        "fireball" | "flamestrike" | "fire_blast" | "blast_wave" | "pyroblast" | "immolate"
        | "rain_of_fire" | "hellfire" | "searing_pain" | "explosive_trap" | "flame_shock"
        | "searing_totem" | "magma_totem" => one(Fire),
        "arcane_missiles" | "arcane_explosion" | "arcane_shot" | "volley" | "moonfire"
        | "starfire" => one(Arcane),
        "shadow_word_pain" | "mind_blast" | "psychic_scream" | "shadow_bolt" | "corruption"
        | "curse_of_agony" | "death_coil" | "shadowburn" => one(Shadow),
        "holy_fire" => one(Holy) | one(Fire),
        "smite" | "holy_nova" | "power_word_shield" | "hammer_of_wrath" | "consecration"
        | "holy_shock" | "exorcism" | "holy_wrath" | "hammer_of_justice" | "divine_shield"
        | "judgement" => one(Holy),
        "serpent_sting" | "lightning_bolt" | "chain_lightning" | "earth_shock"
        | "lightning_shield" | "wrath" | "hurricane" | "insect_swarm" | "entangling_roots"
        | "thorns" | "rejuvenation" => one(Nature),
        "bladestorm" | "shockwave" | "adrenaline_rush" | "bloodthirst" | "shield_slam"
        | "ambush" | "hemorrhage" | "swipe" | "feral_charge" | "bestial_wrath" => one(Physical),
        "meteor" => one(Fire) | one(Frost),
        "seal_of_righteousness" | "retribution_aura" | "lay_on_hands" | "renew" => one(Holy),
        "immolation_trap" | "scorch" => one(Fire),
        "frost_trap" | "ice_barrier" => one(Frost),
        "mana_burn" => one(Arcane),
        "touch_of_weakness" | "drain_life" | "howl_of_terror" => one(Shadow),
        "healing_wave" | "purge" | "faerie_fire" | "tranquility" => one(Nature),
        "frost_blast" | "frostbolt_volley" => one(Frost),
        "fireball_volley" | "inferno" | "wrath_of_ragnaros" | "soul_fire" | "conflagrate"
        | "fire_nova_totem" | "explosive_shot" => one(Fire),
        "crusader_strike" | "seal_of_command" => one(Holy) | one(Physical),
        "holy_shield" => one(Holy),
        "devouring_plague" | "shadow_bolt_volley" | "corrupted_blood" | "mind_flay"
        | "siphon_life" => one(Shadow),
        "ball_lightning" | "lightning_cloud" | "earthquake" | "wyvern_sting" | "chain_heal"
        | "swiftmend" => one(Nature),
        "stormstrike" => one(Physical) | one(Nature),
        "starfall" => one(Arcane) | one(Nature),
        "starshards" | "arcane_blast" | "moonkin_form" => one(Arcane),
        _ => one(Physical),
    }
}

/// The spells a rogue spends combo points on.
pub(crate) fn finisher(key: &str) -> bool {
    matches!(key, "eviscerate" | "kidney_shot" | "rupture" | "execute")
}

/// The kinds that hit an enemy (not the self buffs).
pub(crate) fn offensive(kind: Kind) -> bool {
    !matches!(
        kind,
        Kind::Shield | Kind::Immune | Kind::Thorns | Kind::Heal
    )
}

/// Kinds whose target count grows with rank.
pub(crate) fn counted(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Bolt | Kind::Strike | Kind::Melee | Kind::Dot | Kind::Chain | Kind::Orbit
    )
}

// --- Hit flags: what a rune or a proc asks of a hit as it lands. ---------------------------------

/// May trigger on-hit procs (relics, static discharge). Proc hits never do, so nothing loops.
pub(crate) const F_PROC: u16 = 1;
/// On kill, splits into two lesser hits at the nearest others.
pub(crate) const F_SPLIT: u16 = 1 << 1;
/// Its damage-over-time spreads when the enemy dies.
pub(crate) const F_CONTAGION: u16 = 1 << 2;
/// Its damage-over-time heals the hero.
pub(crate) const F_HAUNT: u16 = 1 << 3;
/// Heals the hero a slice of their health per hit.
pub(crate) const F_BLOODTHIRST: u16 = 1 << 4;
/// Double damage below 30 % health.
pub(crate) const F_EXECUTIONER: u16 = 1 << 5;
/// Applies its statuses twice.
pub(crate) const F_DOUBLE_STATUS: u16 = 1 << 6;
/// A rogue's finisher: crowd control lasts longer.
pub(crate) const F_FINISHER: u16 = 1 << 7;

// --- Runes ----------------------------------------------------------------------------------------

/// A rune: a change to how one spell works. A spell takes one at rank 2, a second at rank 5 and a
/// third at rank 8.
pub(crate) struct RuneDef {
    pub key: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    /// The icon.
    pub spell: u32,
    /// The kinds it fits; empty for any offensive kind.
    pub kinds: &'static [Kind],
    /// An infusion adds this school to the spell.
    pub infuse: Option<School>,
}

const MISSILE: &[Kind] = &[Kind::Bolt, Kind::Strike];
const AREA: &[Kind] = &[
    Kind::Nova,
    Kind::Cone,
    Kind::Ground,
    Kind::GroundSelf,
    Kind::Aura,
];
const SELF: &[Kind] = &[Kind::Shield, Kind::Immune, Kind::Thorns, Kind::Heal];

pub(crate) static RUNES: &[RuneDef] = &[
    RuneDef {
        key: "split",
        name: "Multi-Shot",
        desc: "Killing blows split into 2 bolts at nearby enemies for 50% damage.",
        spell: 7268,
        kinds: MISSILE,
        infuse: None,
    },
    RuneDef {
        key: "barrage",
        name: "Sweeping Strikes",
        desc: "+2 targets, but each hit deals 20% less.",
        spell: 2643,
        kinds: &[
            Kind::Bolt,
            Kind::Strike,
            Kind::Melee,
            Kind::Dot,
            Kind::Orbit,
        ],
        infuse: None,
    },
    RuneDef {
        key: "overload",
        name: "Presence of Mind",
        desc: "Every 4th cast deals triple damage and always crits.",
        spell: 16166,
        kinds: &[Kind::Bolt, Kind::Strike, Kind::Melee, Kind::Chain],
        infuse: None,
    },
    RuneDef {
        key: "executioner",
        name: "Execute",
        desc: "Double damage to enemies below 30% health.",
        spell: 5308,
        kinds: &[Kind::Bolt, Kind::Strike, Kind::Melee, Kind::Chain],
        infuse: None,
    },
    RuneDef {
        key: "rampage",
        name: "Rampage",
        desc: "Every kill with this spell adds 1% damage to it, up to 80%.",
        spell: 12292,
        kinds: &[],
        infuse: None,
    },
    RuneDef {
        key: "bloodthirst",
        name: "Lifestealing",
        desc: "Each hit heals you for 1% of your maximum health.",
        spell: 23881,
        kinds: &[Kind::Melee, Kind::Strike],
        infuse: None,
    },
    RuneDef {
        key: "echo",
        name: "Windfury",
        desc: "Casts again a moment later for 60% damage.",
        spell: 1449,
        kinds: AREA,
        infuse: None,
    },
    RuneDef {
        key: "widen",
        name: "Aura Mastery",
        desc: "+40% area, 10% less damage.",
        spell: 11113,
        kinds: &[
            Kind::Nova,
            Kind::Cone,
            Kind::Ground,
            Kind::GroundSelf,
            Kind::Aura,
            Kind::Chain,
            Kind::Orbit,
        ],
        infuse: None,
    },
    RuneDef {
        key: "tether",
        name: "Retribution Aura",
        desc: "The area forms on you and follows you, and lasts 25% longer.",
        spell: 26573,
        kinds: &[Kind::Ground],
        infuse: None,
    },
    RuneDef {
        key: "twin",
        name: "Mirror Image",
        desc: "Places a second one at another pack.",
        spell: 10,
        kinds: &[Kind::Ground, Kind::Totem],
        infuse: None,
    },
    RuneDef {
        key: "contagion",
        name: "Seed of Corruption",
        desc: "When an afflicted enemy dies, its damage over time spreads to 2 nearby.",
        spell: 172,
        kinds: &[Kind::Dot, Kind::Strike, Kind::Bolt],
        infuse: None,
    },
    RuneDef {
        key: "haunt",
        name: "Drain Life",
        desc: "Its damage over time heals you for 20% of the damage dealt.",
        spell: 6789,
        kinds: &[Kind::Dot, Kind::Strike, Kind::Bolt],
        infuse: None,
    },
    RuneDef {
        key: "forked",
        name: "Forked Lightning",
        desc: "+3 jumps, and the jumps no longer weaken.",
        spell: 421,
        kinds: &[Kind::Chain],
        infuse: None,
    },
    RuneDef {
        key: "reactive",
        name: "Reckoning",
        desc: "Also fires the moment you are struck (once every 2 sec).",
        spell: 20230,
        kinds: &[
            Kind::Nova,
            Kind::Aura,
            Kind::Thorns,
            Kind::Shield,
            Kind::Cone,
        ],
        infuse: None,
    },
    RuneDef {
        key: "radiant",
        name: "Sanctity Aura",
        desc: "On cast, deals 150% of its value as Holy damage to all within 8 yd.",
        spell: 15237,
        kinds: SELF,
        infuse: None,
    },
    RuneDef {
        key: "totemic",
        name: "Totemic Mastery",
        desc: "Totems last 50% longer and attack twice as often.",
        spell: 16190,
        kinds: &[Kind::Totem],
        infuse: None,
    },
    RuneDef {
        key: "resonance",
        name: "Curse of the Elements",
        desc: "Applies its debuffs twice: double Winter's Chill, Ignite, Shock or Sunder Armor.",
        spell: 324,
        kinds: &[],
        infuse: None,
    },
    RuneDef {
        key: "quickened",
        name: "Nature's Swiftness",
        desc: "25% shorter cooldown, 10% less damage.",
        spell: 12043,
        kinds: &[],
        infuse: None,
    },
    RuneDef {
        key: "infuse_frost",
        name: "Icy Chill",
        desc: "Adds Frost damage: applies Winter's Chill, which freezes at 5. +10% damage.",
        spell: 116,
        kinds: &[],
        infuse: Some(School::Frost),
    },
    RuneDef {
        key: "infuse_fire",
        name: "Fiery Weapon",
        desc: "Adds Fire damage: applies Ignite. +10% damage.",
        spell: 133,
        kinds: &[],
        infuse: Some(School::Fire),
    },
    RuneDef {
        key: "infuse_shadow",
        name: "Unholy Weapon",
        desc:
            "Adds Shadow damage: hits harder for every damage over time on the target. +10% damage.",
        spell: 686,
        kinds: &[],
        infuse: Some(School::Shadow),
    },
    RuneDef {
        key: "infuse_nature",
        name: "Deadly Poison",
        desc: "Adds Nature damage: applies Shock; at 3, Chain Lightning leaps out. +10% damage.",
        spell: 403,
        kinds: &[],
        infuse: Some(School::Nature),
    },
    RuneDef {
        key: "infuse_holy",
        name: "Crusader",
        desc: "Adds Holy damage: casts Judgement on what it hits. +10% damage.",
        spell: 585,
        kinds: &[],
        infuse: Some(School::Holy),
    },
    RuneDef {
        key: "infuse_arcane",
        name: "Brilliant Wizard Oil",
        desc: "Adds Arcane damage: grants you Arcane Charges. +10% damage.",
        spell: 1449,
        kinds: &[],
        infuse: Some(School::Arcane),
    },
];

pub(crate) fn rune_fits(r: &RuneDef, kind: Kind, schools: u8) -> bool {
    if let Some(s) = r.infuse {
        return offensive(kind) && !has(schools, s);
    }
    if r.kinds.is_empty() {
        offensive(kind)
    } else {
        r.kinds.contains(&kind)
    }
}

/// How many runes a spell of this rank may hold.
pub(crate) fn rune_slots(level: u8) -> usize {
    match level {
        0..=1 => 0,
        2..=4 => 1,
        5..=7 => 2,
        _ => 3,
    }
}

// --- Talents --------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Req {
    None,
    /// Owned spells of both damage types: a hybrid talent.
    Duo(School, School),
    /// A class's talent tree: class, tree (0..3) and tier (0 is the specialization).
    Tree(u8, u8, u8),
}

pub(crate) struct TalentDef {
    pub key: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub spell: u32,
    pub req: Req,
    /// Power at a price.
    pub cursed: bool,
}

pub(crate) const MAX_TALENTS: usize = 10;

const fn t(
    key: &'static str,
    name: &'static str,
    spell: u32,
    req: Req,
    desc: &'static str,
) -> TalentDef {
    TalentDef {
        key,
        name,
        desc,
        spell,
        req,
        cursed: false,
    }
}

#[allow(clippy::too_many_arguments)]
const fn tt(
    key: &'static str,
    name: &'static str,
    spell: u32,
    class: u8,
    tree: u8,
    tier: u8,
    desc: &'static str,
) -> TalentDef {
    TalentDef {
        key,
        name,
        desc,
        spell,
        req: Req::Tree(class, tree, tier),
        cursed: false,
    }
}

const fn cursed(
    key: &'static str,
    name: &'static str,
    spell: u32,
    desc: &'static str,
) -> TalentDef {
    TalentDef {
        key,
        name,
        desc,
        spell,
        req: Req::None,
        cursed: true,
    }
}

use School::*;

pub(crate) static TALENTS: &[TalentDef] = &[
    // --- General: any class may draw them. ---
    t("clearcasting", "Clearcasting", 12536, Req::None,
      "Each cast has a 10% chance to be ready again at once."),
    t("critical_mass", "Critical Mass", 11115, Req::None,
      "+10% critical strike chance; critical strikes deal 250% instead of 200%."),
    t("blood_frenzy", "Blood Frenzy", 16952, Req::None,
      "Below half health: +30% haste and +20% damage."),
    t("vampiric", "Vampiric Embrace", 15286, Req::None,
      "Heals you for 2% of all damage you deal."),
    t("specialist", "Lone Wolf", 14751, Req::None,
      "+15% damage for every empty spell slot. Pairs well with unlearning spells."),
    t("polymath", "Elemental Precision", 1459, Req::None,
      "+6% damage for every different damage type among your spells."),
    t("second_wind", "Second Wind", 16487, Req::None,
      "When a boss falls you are fully healed and gain an extra reroll, banish and unlearn."),
    t("hourglass", "Permafrost", 23645, Req::None,
      "Your stuns, roots, fears and freezes last 50% longer."),
    // --- Cursed: more power, more danger. ---
    cursed("glass_cannon", "Arcane Power", 12042,
      "+45% damage. -35% maximum health."),
    cursed("death_wish", "Death Wish", 12328,
      "+30% damage and +15% movement speed. You take 25% more damage."),
    cursed("hunger", "Heroic Difficulty", 18288,
      "+40% experience. Enemies have 15% more health."),
    // --- Hybrid talents: two damage types that feed each other. ---
    t("frostfire", "Frostfire", 133, Req::Duo(Frost, Fire),
      "Fire deals +50% to chilled and frozen enemies. Fire striking a frozen enemy shatters it: 60% of the hit to all within 5 yd."),
    t("shadowflame", "Shadowflame", 17877, Req::Duo(Shadow, Fire),
      "Ignite counts as 2 damage over time for Shadow Weaving, and Shadow hits rekindle Ignite."),
    t("frozen_circuit", "Frostbrand Weapon", 8056, Req::Duo(Nature, Frost),
      "Chain Lightning discharges apply Winter's Chill twice. Frozen enemies discharge on every Nature hit."),
    t("twilight", "Penance", 25912, Req::Duo(Holy, Shadow),
      "Judgement counts as a damage over time. Killing an afflicted enemy casts Judgement on all within 8 yd."),
    t("arcane_storm", "Wrath of Elune", 2912, Req::Duo(Arcane, Nature),
      "Each Arcane Charge adds a target to Chain Lightning discharges and 10% to their damage."),
    t("searing_light", "Searing Light", 14914, Req::Duo(Holy, Fire),
      "Holy deals +30% to enemies with Ignite, and every hit on an enemy under Judgement ignites it."),
    t("frostblade", "Frostmourne", 120, Req::Duo(Physical, Frost),
      "Physical hits apply Winter's Chill. Physical hits on frozen enemies cleave everything within 5 yd for 50%."),
    t("spellblade", "Arcanite Reaper", 1680, Req::Duo(Physical, Arcane),
      "Physical hits build Arcane Charges. At full charges, +20% critical strike chance."),
    t("rending_storm", "Doomhammer", 17364, Req::Duo(Physical, Nature),
      "Chain Lightning discharges apply Sunder Armor twice. Sundered enemies count 1 extra Shock."),
    t("plague", "Scourge Invasion", 5570, Req::Duo(Shadow, Nature),
      "Every damage over time spreads to 1 nearby enemy when its victim dies."),

    // --- Talent trees: three per class. The first talent of a tree is your specialization (it may
    // change your starting spell); the second needs the first; both together teach the tree's
    // deep spell. ---
    // Warrior
    tt("deep_wounds", "Deep Wounds", 12162, 1, 0, 0,
      "Physical critical strikes make the target bleed for 50% of the hit over 6 sec. Sunder Armor stacks to 8."),
    tt("impale", "Impale", 16493, 1, 0, 1,
      "Critical strikes deal 50% more damage."),
    tt("unbridled_wrath", "Unbridled Wrath", 12322, 1, 1, 0,
      "Rage builds twice as fast, and Enrage also makes every hit crit."),
    tt("flurry", "Flurry", 12319, 1, 1, 1,
      "After a critical strike: +30% haste for 3 sec."),
    tt("toughness", "Toughness", 12299, 1, 2, 0,
      "+25% maximum health."),
    tt("anticipation", "Anticipation", 12297, 1, 2, 1,
      "+10% chance to dodge a blow. Rage builds 50% faster from blows you take."),
    // Paladin
    tt("zeal", "Judgement of Light", 20185, 2, 0, 0,
      "Enemies under Judgement take +30% physical damage instead of +15%. Their deaths heal 3 and grant 4% haste for 6 sec, stacking 5 times."),
    tt("spiritual", "Spiritual Focus", 20205, 2, 0, 1,
      "Regenerate 1.5 health per sec."),
    tt("toughness", "Toughness", 20143, 2, 1, 0,
      "+25% maximum health."),
    tt("anticipation", "Redoubt", 20127, 2, 1, 1,
      "+10% chance to dodge a blow."),
    tt("vengeance", "Vengeance", 20049, 2, 2, 0,
      "Holy Power stacks to 12, and each stack also grants 2% damage."),
    tt("conviction", "Conviction", 20117, 2, 2, 1,
      "+8% critical strike chance."),
    // Hunter
    tt("flurry", "Frenzy", 19621, 3, 0, 0,
      "After a critical strike: +30% haste for 3 sec."),
    tt("toughness", "Endurance Training", 19583, 3, 0, 1,
      "+25% maximum health."),
    tt("trueshot", "Improved Hunter's Mark", 19421, 3, 1, 0,
      "Hunter's Mark marks 3 enemies, and marked enemies take +60% instead of +35%."),
    tt("impale", "Mortal Shots", 19485, 3, 1, 1,
      "Critical strikes deal 50% more damage."),
    tt("anticipation", "Deflection", 19295, 3, 2, 0,
      "+10% chance to dodge a blow."),
    tt("conviction", "Killer Instinct", 19370, 3, 2, 1,
      "+8% critical strike chance."),
    // Rogue
    tt("relentless", "Relentless Strikes", 14179, 4, 0, 0,
      "Finishers cut the cooldown of your other spells by 30%."),
    tt("impale", "Lethality", 14128, 4, 0, 1,
      "Critical strikes deal 50% more damage."),
    tt("flurry", "Lightning Reflexes", 13712, 4, 1, 0,
      "After a critical strike: +30% haste for 3 sec."),
    tt("conviction", "Precision", 13705, 4, 1, 1,
      "+8% critical strike chance."),
    tt("deep_wounds", "Serrated Blades", 14171, 4, 2, 0,
      "Physical critical strikes make the target bleed for 50% of the hit over 6 sec. Sunder Armor stacks to 8."),
    tt("anticipation", "Camouflage", 13975, 4, 2, 1,
      "+10% chance to dodge a blow."),
    // Priest
    tt("spiritual", "Inner Focus", 14751, 5, 0, 0,
      "Regenerate 1.5 health per sec."),
    tt("conviction", "Mental Strength", 18551, 5, 0, 1,
      "+8% critical strike chance."),
    tt("zeal", "Holy Specialization", 14889, 5, 1, 0,
      "Enemies under Judgement take +30% physical damage instead of +15%. Their deaths heal 3 and grant 4% haste for 6 sec, stacking 5 times."),
    tt("clearcasting", "Holy Concentration", 14898, 5, 1, 1,
      "Each cast has a 10% chance to be ready again at once."),
    tt("weaving", "Shadow Weaving", 15257, 5, 2, 0,
      "Shadow deals +15% per damage over time on the target instead of +8%, up to 6."),
    tt("spirit_tap", "Spirit Tap", 15270, 5, 2, 1,
      "Atonement heals twice as much, and Shadow Weaving stacks once more."),
    // Shaman
    tt("conduction", "Lightning Mastery", 16578, 7, 0, 0,
      "Chain Lightning discharges at 2 Shock instead of 3, strike 3 enemies, and deal 50% instead of 35%."),
    tt("elemental_mastery", "Elemental Mastery", 16166, 7, 0, 1,
      "Elemental Overload triggers every 3rd cast instead of every 5th."),
    tt("flurry", "Flurry", 16256, 7, 1, 0,
      "After a critical strike: +30% haste for 3 sec."),
    tt("impale", "Elemental Weapons", 16266, 7, 1, 1,
      "Critical strikes deal 50% more damage."),
    tt("spiritual", "Restorative Totems", 16187, 7, 2, 0,
      "Regenerate 1.5 health per sec."),
    tt("toughness", "Ancestral Healing", 16176, 7, 2, 1,
      "+25% maximum health."),
    // Mage
    tt("arcane_mind", "Arcane Mind", 11232, 8, 0, 0,
      "Arcane Charges stack to 8 instead of 4, and each grants 6% damage instead of 4%."),
    tt("conviction", "Arcane Instability", 15058, 8, 0, 1,
      "+8% critical strike chance."),
    tt("ignite", "Ignite", 11119, 8, 1, 0,
      "Ignite deals double damage, and a burning enemy's flames leap to 2 nearby when it dies."),
    tt("combustion", "Combustion", 11129, 8, 1, 1,
      "Master of Elements grants +45% instead of +20%."),
    tt("shatter", "Shatter", 11170, 8, 2, 0,
      "Enemies freeze at 4 Winter's Chill instead of 5, and frozen enemies take +100% damage instead of +40%."),
    tt("impale", "Ice Shards", 11207, 8, 2, 1,
      "Critical strikes deal 50% more damage."),
    // Warlock
    tt("weaving", "Shadow Mastery", 18271, 9, 0, 0,
      "Shadow deals +15% per damage over time on the target instead of +8%, up to 6."),
    tt("soul_harvest", "Improved Drain Soul", 18213, 9, 0, 1,
      "Soul Shards come twice as fast, and Soul Fire deals double damage."),
    tt("toughness", "Demonic Embrace", 18697, 9, 1, 0,
      "+25% maximum health."),
    tt("spiritual", "Fel Stamina", 18748, 9, 1, 1,
      "Regenerate 1.5 health per sec."),
    tt("ignite", "Emberstorm", 17954, 9, 2, 0,
      "Ignite deals double damage, and a burning enemy's flames leap to 2 nearby when it dies."),
    tt("impale", "Ruin", 17959, 9, 2, 1,
      "Critical strikes deal 50% more damage."),
    // Druid
    tt("arcane_mind", "Moonglow", 16845, 11, 0, 0,
      "Arcane Charges stack to 8 instead of 4, and each grants 6% damage instead of 4%."),
    tt("moonkin", "Moonfury", 16896, 11, 0, 1,
      "Eclipses last twice as long and grant +70% instead of +40%."),
    tt("flurry", "Feral Instinct", 16947, 11, 1, 0,
      "After a critical strike: +30% haste for 3 sec."),
    tt("impale", "Sharpened Claws", 16942, 11, 1, 1,
      "Critical strikes deal 50% more damage."),
    tt("spiritual", "Furor", 17056, 11, 2, 0,
      "Regenerate 1.5 health per sec."),
    tt("toughness", "Nature's Focus", 17063, 11, 2, 1,
      "+25% maximum health."),
];

// --- Talent trees ---------------------------------------------------------------------------------

pub(crate) struct TreeDef {
    pub class: u8,
    pub name: &'static str,
    pub icon: u32,
    /// The specialization's starting spell: taking the tree's first talent turns the class's
    /// starting spell into this one.
    pub swap: Option<&'static str>,
    /// The deep spell both talents of the tree teach.
    pub capstone: &'static str,
}

const fn tree(
    class: u8,
    name: &'static str,
    icon: u32,
    swap: Option<&'static str>,
    capstone: &'static str,
) -> TreeDef {
    TreeDef {
        class,
        name,
        icon,
        swap,
        capstone,
    }
}

/// Three per class, in the order the talents above number them.
pub(crate) static TREES: &[TreeDef] = &[
    tree(1, "Arms", 12294, Some("heroic_strike"), "mortal_strike"),
    tree(1, "Fury", 23881, Some("whirlwind"), "bloodthirst"),
    tree(1, "Protection", 23922, Some("thunder_clap"), "shield_slam"),
    tree(2, "Holy", 20473, Some("exorcism"), "holy_shock"),
    tree(2, "Protection", 20925, Some("consecration"), "holy_shield"),
    tree(
        2,
        "Retribution",
        20375,
        Some("judgement"),
        "seal_of_command",
    ),
    tree(
        3,
        "Beast Mastery",
        19574,
        Some("multi_shot"),
        "bestial_wrath",
    ),
    tree(3, "Marksmanship", 19506, None, "aimed_shot"),
    tree(3, "Survival", 19386, Some("serpent_sting"), "wyvern_sting"),
    tree(4, "Assassination", 14177, Some("eviscerate"), "ambush"),
    tree(4, "Combat", 13877, None, "blade_flurry"),
    tree(4, "Subtlety", 16511, Some("gouge"), "hemorrhage"),
    tree(5, "Discipline", 10060, None, "starshards"),
    tree(5, "Holy", 15237, Some("holy_fire"), "holy_nova"),
    tree(5, "Shadow", 15473, Some("shadow_word_pain"), "mind_flay"),
    tree(7, "Elemental", 16166, None, "earthquake"),
    tree(
        7,
        "Enhancement",
        17364,
        Some("lightning_shield"),
        "stormstrike",
    ),
    tree(7, "Restoration", 16190, Some("searing_totem"), "chain_heal"),
    tree(8, "Arcane", 12042, Some("arcane_missiles"), "arcane_blast"),
    tree(8, "Fire", 11129, Some("fireball"), "pyroblast"),
    tree(8, "Frost", 11958, None, "frostbolt_volley"),
    tree(9, "Affliction", 18265, Some("corruption"), "siphon_life"),
    tree(9, "Demonology", 19028, None, "soul_fire"),
    tree(9, "Destruction", 17962, Some("immolate"), "conflagrate"),
    tree(11, "Balance", 24858, Some("moonfire"), "moonkin_form"),
    tree(11, "Feral Combat", 16979, Some("swipe"), "feral_charge"),
    tree(11, "Restoration", 18562, None, "swiftmend"),
];

/// The class's trees, with their index.
pub(crate) fn trees(class: u8) -> impl Iterator<Item = (u8, &'static TreeDef)> {
    TREES
        .iter()
        .filter(move |t| t.class == class)
        .enumerate()
        .map(|(i, t)| (i as u8, t))
}

pub(crate) fn tree_def(class: u8, tree: u8) -> Option<&'static TreeDef> {
    trees(class).find(|(i, _)| *i == tree).map(|(_, t)| t)
}

// --- Combinations -----------------------------------------------------------------------------------

/// What a combination asks for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Need {
    /// This spell, owned at its highest rank.
    Max(&'static str),
    /// Any owned spell of this damage type at its highest rank.
    MaxSchool(School),
    /// This item carried.
    Relic(&'static str),
}

/// Two mastered spells (or a mastered spell and a legendary item) that join into a third.
pub(crate) struct ComboDef {
    /// The spell it teaches.
    pub key: &'static str,
    pub needs: &'static [Need],
    pub desc: &'static str,
}

pub(crate) static COMBOS: &[ComboDef] = &[
    ComboDef { key: "meteor", needs: &[Need::Max("blizzard"), Need::Max("flamestrike")],
      desc: "Fire and ice from the sky: a Meteor crashes into the densest pack, stunning everything it lands on. Frost and Fire at once." },
    ComboDef { key: "frost_blast", needs: &[Need::Max("frostbolt"), Need::Max("frost_nova")],
      desc: "Kel'Thuzad's Frost Blast: freezes everything around you solid and shatters it." },
    ComboDef { key: "fireball_volley", needs: &[Need::Max("fireball"), Need::Max("pyroblast")],
      desc: "Five Fireballs at once, each one exploding." },
    ComboDef { key: "bladestorm", needs: &[Need::Max("whirlwind"), Need::Max("sweeping_strikes")],
      desc: "Become a whirlwind of steel: hits everything around you, over and over." },
    ComboDef { key: "shockwave", needs: &[Need::Max("mortal_strike"), Need::Max("execute")],
      desc: "A Shockwave rips through everything in front of you and stuns it." },
    ComboDef { key: "crusader_strike", needs: &[Need::Max("hammer_of_wrath"), Need::Max("judgement")],
      desc: "Holy-forged blows that strike three enemies at once, Holy and Physical both." },
    ComboDef { key: "explosive_shot", needs: &[Need::Max("multi_shot"), Need::Max("explosive_trap")],
      desc: "Arrows that burst into flame on impact." },
    ComboDef { key: "adrenaline_rush", needs: &[Need::Max("sinister_strike"), Need::Max("blade_flurry")],
      desc: "A blur of blades: strikes everything near you several times a second." },
    ComboDef { key: "devouring_plague", needs: &[Need::Max("shadow_word_pain"), Need::Max("mind_flay")],
      desc: "A plague that eats five enemies at once and feeds you their life." },
    ComboDef { key: "fire_nova_totem", needs: &[Need::Max("searing_totem"), Need::Max("magma_totem")],
      desc: "A totem that erupts in a huge Fire Nova." },
    ComboDef { key: "ball_lightning", needs: &[Need::Max("lightning_bolt"), Need::Max("chain_lightning")],
      desc: "Four orbs of lightning at once, each one crackling with Shock." },
    ComboDef { key: "shadow_bolt_volley", needs: &[Need::Max("shadow_bolt"), Need::Max("corruption")],
      desc: "Six Shadow Bolts at once." },
    ComboDef { key: "inferno", needs: &[Need::Max("rain_of_fire"), Need::Max("hellfire")],
      desc: "Calls down an Infernal on the densest pack: stuns them, then burns the ground." },
    ComboDef { key: "starfall", needs: &[Need::Max("moonfire"), Need::Max("starfire")],
      desc: "Stars rain on every enemy in a wide area. Arcane and Nature at once." },
    // Legendary items join any mastered spell of their element.
    ComboDef { key: "wrath_of_ragnaros", needs: &[Need::Relic("sulfuras"), Need::MaxSchool(School::Fire)],
      desc: "BY FIRE BE PURGED! Ragnaros's wrath hurls back everything around you in flame." },
    ComboDef { key: "lightning_cloud", needs: &[Need::Relic("thunderfury"), Need::MaxSchool(School::Nature)],
      desc: "The Windseeker's storm: a thundercloud that follows the fight and lashes everything beneath it." },
    ComboDef { key: "corrupted_blood", needs: &[Need::Relic("hakkar"), Need::MaxSchool(School::Shadow)],
      desc: "Hakkar's plague: six enemies at once bleed shadow, and you drink it." },
];

/// Spells that come from a talent tree or a combination: they take no spell slot.
pub(crate) fn bonus_spell(key: &str) -> bool {
    TREES.iter().any(|t| t.capstone == key) || COMBOS.iter().any(|c| c.key == key)
}

// --- Relics ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tier {
    Rare,
    Epic,
    Legendary,
}

pub(crate) struct RelicDef {
    pub key: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub spell: u32,
    pub tier: Tier,
    pub cursed: bool,
    /// Only a boss drops it.
    pub boss: bool,
    /// Never dropped: forged from two others.
    pub forged: bool,
}

const fn relic(
    key: &'static str,
    name: &'static str,
    spell: u32,
    tier: Tier,
    desc: &'static str,
) -> RelicDef {
    RelicDef {
        key,
        name,
        desc,
        spell,
        tier,
        cursed: false,
        boss: false,
        forged: false,
    }
}

pub(crate) static RELICS: &[RelicDef] = &[
    relic("ephemeral", "Talisman of Ephemeral Power", 23271, Tier::Rare,
      "Every 20 sec: +40% damage for 6 sec."),
    relic("hero_charm", "Zandalarian Hero Charm", 24659, Tier::Rare,
      "On level up: +60% damage, fading over 10 sec."),
    relic("onyxia", "Onyxia Tooth Pendant", 18803, Tier::Rare,
      "+8% critical strike chance; critical strikes deal 50% more."),
    relic("drake_fang", "Drake Fang Talisman", 24427, Tier::Rare,
      "12% chance to dodge a blow entirely."),
    relic("lifegiving", "Lifegiving Gem", 23725, Tier::Rare,
      "Falling below 30% health restores 40% of it. 90 sec cooldown."),
    relic("kiss_spider", "Kiss of the Spider", 28866, Tier::Rare,
      "+12% haste and +10% movement speed."),
    relic("gift_arthas", "Gift of Arthas", 11374, Tier::Rare,
      "Enemies that strike you are diseased for 20 Shadow damage per sec for 5 sec."),
    relic("shroud", "Cloak of the Shrouded Mists", 1784, Tier::Rare,
      "+100% pickup range and +8% movement speed."),
    relic("dalrend_r", "Dal'Rend's Sacred Charge", 20375, Tier::Rare,
      "+10% haste. Set with Dal'Rend's Tribal Guardian: +25% physical damage and +3 Sunder Armor."),
    relic("dalrend_l", "Dal'Rend's Tribal Guardian", 20925, Tier::Rare,
      "+4 armor. Set with Dal'Rend's Sacred Charge: +25% physical damage and +3 Sunder Armor."),
    relic("binding_l", "Left Binding of the Windseeker", 21992, Tier::Epic,
      "+15% Nature damage. Find the right binding and the two forge Thunderfury."),
    relic("binding_r", "Right Binding of the Windseeker", 21992, Tier::Epic,
      "+15% Nature damage. Find the left binding and the two forge Thunderfury."),
    relic("eye_sulfuras", "Eye of Sulfuras", 2120, Tier::Epic,
      "+15% Fire damage. With a Sulfuron Ingot it forges Sulfuras."),
    relic("sulfuron", "Sulfuron Ingot", 2136, Tier::Epic,
      "+15% Fire damage. With the Eye of Sulfuras it forges Sulfuras."),
    relic("hoj", "Hand of Justice", 20007, Tier::Epic,
      "12% chance for a spell to fire twice."),
    relic("accuria", "Band of Accuria", 3044, Tier::Epic,
      "+1 target for every spell that strikes several."),
    relic("neltharion", "Neltharion's Tear", 12043, Tier::Epic,
      "+20% area and +10% spell damage."),
    relic("quelserrar", "Quel'Serrar", 20925, Tier::Epic,
      "Every 6 sec, the next blow against you is absorbed entirely."),
    // Boss relics: the strongest, and the cursed.
    RelicDef {
        boss: true,
        ..relic("atiesh", "Atiesh, Greatstaff of the Guardian", 10060, Tier::Legendary,
          "Every spell fights one rank above its own.")
    },
    RelicDef {
        boss: true,
        cursed: true,
        ..relic("ashbringer", "Corrupted Ashbringer", 20271, Tier::Legendary,
          "+50% damage and every kill heals 1. You take 20% more damage.")
    },
    RelicDef {
        boss: true,
        cursed: true,
        ..relic("hakkar", "Heart of Hakkar", 24324, Tier::Legendary,
          "+1 spell slot and +1 talent slot. Enemies have 25% more health. With a mastered Shadow spell: Corrupted Blood.")
    },
    RelicDef {
        boss: true,
        cursed: true,
        ..relic("cthun", "Eye of C'Thun", 26476, Tier::Epic,
          "+50% experience, +10% critical strike chance. Enemies deal 20% more damage.")
    },
    RelicDef {
        boss: true,
        ..relic("hourglass", "Hourglass Sand", 23645, Tier::Epic,
          "Rerolls, banishes and unlearns: +2 of each.")
    },
    // Forged.
    RelicDef {
        forged: true,
        ..relic("thunderfury", "Thunderfury, Blessed Blade of the Windseeker", 21992,
          Tier::Legendary,
          "15% chance on hit: lightning arcs to 5 enemies for 60% of the hit, slowing them. +30% Nature damage. With a mastered Nature spell: Lightning Cloud.")
    },
    RelicDef {
        forged: true,
        ..relic("sulfuras", "Sulfuras, Hand of Ragnaros", 2120, Tier::Legendary,
          "Every 8 sec a meteor strikes the densest pack, igniting all within 6 yd. +30% Fire damage. With a mastered Fire spell: Wrath of Ragnaros.")
    },
];

pub(crate) fn relic_def(key: &str) -> &'static RelicDef {
    RELICS.iter().find(|r| r.key == key).expect("relic")
}

/// Two relics that forge into a third.
pub(crate) static FORGES: &[(&str, &str, &str)] = &[
    ("binding_l", "binding_r", "thunderfury"),
    ("eye_sulfuras", "sulfuron", "sulfuras"),
];

// --- Races ----------------------------------------------------------------------------------------

/// The race's trait: its icon, name and what it does.
pub(crate) fn race_trait(race: u8) -> (u32, &'static str, &'static str) {
    match race {
        1 => (20600, "Diplomacy", "+1 reroll and +1 banish. +10% experience."),
        2 => (20572, "Blood Fury", "Every 45 sec in battle: +30% damage for 12 sec."),
        3 => (20594, "Stoneform", "+3 armor. Below 35% health: take 50% less damage for 8 sec. 60 sec cooldown."),
        4 => (20580, "Shadowmeld", "10% chance to dodge a blow. +5% movement speed."),
        5 => (20577, "Cannibalize", "Kills heal 0.5. +10% Shadow damage."),
        6 => (20549, "War Stomp", "+15% maximum health. When 5 enemies close in, stun everything within 7 yd for 2 sec. 20 sec cooldown."),
        7 => (20589, "Expansive Mind", "+8% haste. Your movement spell recovers 25% faster."),
        8 => (20554, "Berserking", "Up to +30% haste as your health falls. +0.6 health per sec."),
        _ => (0, "", ""),
    }
}

/// The class resource: its name and what it does.
pub(crate) fn class_trait(class: u8) -> (u32, &'static str, &'static str) {
    match class {
        1 => (2687, "Rage", "Blows taken and physical hits build rage; each point is +0.3% damage. At 100: Enrage, +25% haste and +15% damage for 8 sec."),
        2 => (20049, "Holy Power", "Each Holy hit grants Holy Power: +3% haste for 6 sec, up to 8 stacks."),
        3 => (1130, "Hunter's Mark", "The toughest enemy near you is marked and takes +35% damage. Killing it grants Rapid Fire: +40% haste for 5 sec."),
        4 => (14177, "Combo Points", "Each strike adds a combo point. At 5, the next finisher (or any spell, without one) deals triple damage and its stun lasts twice as long."),
        5 => (15237, "Atonement", "5% of your Holy damage heals you. Shadow Weaving stacks once more."),
        7 => (16246, "Elemental Overload", "Every 5th cast fires twice."),
        8 => (12042, "Master of Elements", "A spell of a different element than your last deals +20% damage: weave Frost, Fire and Arcane."),
        9 => (6353, "Soul Shards", "Enemies dying under your damage over time yield soul shards (+5% damage over time each). At 5, Soul Fire hurls itself at the toughest foe."),
        11 => (24858, "Eclipse", "5 Nature casts bring a Lunar Eclipse (+40% Arcane), 5 Arcane casts a Solar Eclipse (+40% Nature), 10 sec each."),
        _ => (0, "", ""),
    }
}

// --- Run state --------------------------------------------------------------------------------------

/// Timers and counters the build layer keeps for one run.
#[derive(Default)]
pub(crate) struct BuildState {
    /// Arcane charges and the seconds until they fade.
    pub arcane: u8,
    pub arcane_t: f32,
    /// The class resource: rage, combo points, soul shards or zeal stacks.
    pub res: f32,
    /// Seconds since the last fight, for rage decay; or the zeal stacks' time left.
    pub res_t: f32,
    /// Warrior Enrage, Hunter Rapid Fire: seconds left.
    pub surge_t: f32,
    /// Mage: the school of the last cast.
    pub last_school: u8,
    /// Every cast in the run, for the every-nth effects.
    pub casts: u32,
    /// Druid: casts toward each eclipse, and each eclipse's time left.
    pub nature_n: u8,
    pub arcane_n: u8,
    pub lunar_t: f32,
    pub solar_t: f32,
    /// Warlock: seconds until the next shard can come.
    pub shard_cd: f32,
    /// Holy talent zeal: stacks and time left.
    pub zeal: u8,
    pub zeal_t: f32,
    /// Relic and race timers.
    pub ephemeral_cd: f32,
    pub ephemeral_t: f32,
    pub charm_t: f32,
    pub fury_cd: f32,
    pub fury_t: f32,
    pub stone_cd: f32,
    pub stone_t: f32,
    pub stomp_cd: f32,
    pub lifegiving_cd: f32,
    pub quel_cd: f32,
    pub meteor_cd: f32,
    /// Struck since the last cast pass: reprisal runes fire.
    pub struck: bool,
    /// Shaman: the next cast fires twice.
    pub overload: bool,
    /// Soul Fire is due at the toughest enemy.
    pub soul_fire: bool,
    /// A second wind is due: a boss fell.
    pub bosses_slain: u32,
    /// Flurry: haste after a critical strike, seconds left.
    pub flurry_t: f32,
}
