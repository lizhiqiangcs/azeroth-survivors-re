//! The AI players: how they move, and how they choose at the level-up cards.
//!
//! A profile is a pick policy and a movement policy. The profiles span the players the game is
//! for, from someone on their first run to someone who has read the talent trees:
//!
//! | profile | picks | moves |
//! |---|---|---|
//! | `novice` | at random, never rerolls | drifts from danger late and loosely, often stands |
//! | `casual` | the shiny card: new spells and upgrades, a little synergy sense | kites in circles |
//! | `expert` | plans the build: spec, combos, capstones, rerolls bad hands | stands while strong, kites when hurt, sweeps up gems |
//! | `expert_stand` | as expert | stands and fights |
//! | `random_picks` | at random | as expert |
//! | `expert_nokeys` | as expert, but barely touches Q/E/F | as expert |
//!
//! The last two isolate the two skills: `expert − random_picks` is what the choices are worth,
//! `expert − expert_stand` what the footwork is worth.

use super::super::data::{self, Kind, MAX_ABILITIES, MAX_ABILITY_LEVEL};
use super::super::hud::{Choice, Offer};
use super::super::synergy::{self, Need, Req, COMBOS};
use super::super::{Rng, Run};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Movement {
    /// Stands still; steps out of a closing ring.
    Stand,
    /// Runs circles, the horde behind.
    Kite,
    /// Goes for the gems, the crowd shoving it off.
    Collect,
    /// Stands and fights while healthy and not surrounded; kites to gems otherwise.
    Adaptive,
    /// A first run: drifts from danger late and loosely, often standing still.
    Wander,
    /// `survivors::auto`, the game's own test driver: to the nearest gem, pushed off by the
    /// crowd. For checking the simulator against the game.
    Auto,
}

/// How a player works the hand-cast spells (Q burst, E guard, F move).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hands {
    /// Never touches them: the game's test driver.
    Off,
    /// Forgets them but in a panic: the guard at death's door, the burst now and then.
    Panic,
    /// Presses the burst whenever there is a crowd, the guard and the move when hurt.
    Eager,
    /// Saves the burst for a boss or a packed field, the guard for a real threat, the move to
    /// break out of a ring (or to charge in, for a warrior).
    Timed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Picks {
    Random,
    Casual,
    Expert,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Profile {
    pub name: &'static str,
    pub picks: Picks,
    pub movement: Movement,
    pub hands: Hands,
}

pub(crate) const PROFILES: &[Profile] = &[
    Profile { name: "novice", picks: Picks::Random, movement: Movement::Wander, hands: Hands::Panic },
    Profile { name: "casual", picks: Picks::Casual, movement: Movement::Kite, hands: Hands::Eager },
    Profile { name: "expert", picks: Picks::Expert, movement: Movement::Adaptive, hands: Hands::Timed },
    Profile { name: "expert_stand", picks: Picks::Expert, movement: Movement::Stand, hands: Hands::Timed },
    Profile { name: "random_picks", picks: Picks::Random, movement: Movement::Adaptive, hands: Hands::Timed },
    Profile { name: "expert_nokeys", picks: Picks::Expert, movement: Movement::Adaptive, hands: Hands::Panic },
    // Not in the default batch: the game's test driver, for calibration (`sim_calibrate`).
    Profile { name: "auto", picks: Picks::Random, movement: Movement::Auto, hands: Hands::Off },
];

pub(crate) fn profile(name: &str) -> Option<Profile> {
    PROFILES.iter().copied().find(|p| p.name == name)
}

/// The vanilla class and race pairs.
pub(crate) fn races(class: u8) -> &'static [u8] {
    match class {
        1 => &[1, 2, 3, 4, 5, 6, 7, 8],
        2 => &[1, 3],
        3 => &[2, 3, 4, 6, 8],
        4 => &[1, 2, 3, 4, 5, 7, 8],
        5 => &[1, 3, 4, 5, 8],
        7 => &[2, 6, 8],
        8 => &[1, 5, 7, 8],
        9 => &[1, 2, 5, 7],
        11 => &[4, 6],
        _ => &[1],
    }
}

/// What a player is after in one run.
pub(crate) struct Plan<'a> {
    /// The talent tree it means to take.
    pub tree: u8,
    /// What the simulator has learned each card is worth, by `class:card`, in win-rate points:
    /// `target/sim/values.tsv`, written by every batch from its random pickers.
    pub values: &'a std::collections::HashMap<String, f32>,
}

/// The learned worth of a card for this class, weighted to sit beside the hand-made scores.
fn learned(c: Choice, run: &Run, plan: &Plan) -> f32 {
    let key = format!("{}:{}", run.class.id, card_family(&card_key(c, run)));
    plan.values.get(&key).map_or(0.0, |v| v.clamp(-30.0, 30.0) * 1.2)
}

/// The spells a combination would want at top rank, for this run.
fn combo_parts(run: &Run) -> Vec<&'static str> {
    let mut out = Vec::new();
    for c in COMBOS {
        if run.abilities.iter().any(|o| o.def.key == c.key) {
            continue;
        }
        let owned = |k: &str| run.abilities.iter().any(|o| o.def.key == k);
        let reachable = c.needs.iter().all(|n| match *n {
            Need::Max(k) => owned(k) || run.class.pool.contains(&k),
            Need::MaxSchool(_) => true,
            Need::Relic(k) => run.has_relic(k),
        });
        if !reachable {
            continue;
        }
        for n in c.needs {
            if let Need::Max(k) = *n {
                out.push(k);
            }
        }
    }
    out
}

fn schools_owned(run: &Run) -> u8 {
    run.abilities.iter().fold(0, |m, o| m | o.schools())
}

/// How much an expert wants a card, roughly on a 0..100 scale.
fn expert_score(c: Choice, run: &Run, plan: &Plan<'_>) -> f32 {
    let parts = combo_parts(run);
    let schools = schools_owned(run);
    let hp = run.hp / run.stats().max_hp.max(1.0);
    let slots = run.slots_used();
    match c {
        Choice::Combo(_) => 100.0,
        Choice::Learn(_) => 92.0,
        Choice::Heal => {
            if hp < 0.35 {
                95.0
            } else {
                4.0
            }
        }
        Choice::Talent(t) => match t.req {
            Req::Tree(_, tree, 0) => {
                if tree == plan.tree {
                    99.0
                } else {
                    1.0
                }
            }
            Req::Tree(_, tree, tier) => {
                if tree == plan.tree {
                    62.0 + 4.0 * f32::from(tier)
                } else {
                    24.0
                }
            }
            Req::Duo(..) => 58.0,
            Req::None => {
                if t.cursed {
                    30.0
                } else {
                    40.0
                }
            }
        },
        Choice::Upgrade(k) => {
            let o = &run.abilities[k];
            let mut s = 44.0;
            if parts.contains(&o.def.key) {
                s += 18.0;
            }
            if o.level + 1 == MAX_ABILITY_LEVEL {
                s += 8.0;
            }
            if matches!(o.level + 1, 2 | 5 | 8) {
                // A rune slot opens.
                s += 4.0;
            }
            if !synergy::offensive(o.def.kind) {
                s -= 12.0;
            }
            s
        }
        Choice::NewAbility(d) => {
            let mut s = 50.0 - 6.0 * slots as f32;
            if parts.contains(&d.key) {
                s += 16.0;
            }
            let sch = synergy::school_of(d.key);
            if sch & schools != 0 {
                s += 6.0;
            }
            if !synergy::offensive(d.kind) {
                s += if hp < 0.5 { 6.0 } else { -10.0 };
            }
            if slots >= MAX_ABILITIES - 1 {
                s -= 15.0;
            }
            s
        }
        Choice::Rune(k, r) => {
            let mut s = 46.0;
            if let Some(sch) = r.infuse {
                if sch.bit() & schools == 0 {
                    s -= 4.0;
                }
            }
            if parts.contains(&run.abilities[k].def.key) {
                s += 4.0;
            }
            s
        }
        Choice::Relic(r) => {
            let mut s = 55.0 + 10.0 * r.tier as u8 as f32;
            if r.cursed && hp < 0.5 {
                s -= 25.0;
            }
            s
        }
        Choice::NewPassive(_) => 34.0,
        Choice::UpPassive(_) => 32.0,
        Choice::Forget(_) => 0.0,
    }
}

/// How much a casual player wants a card: the big, new, shiny thing.
fn casual_score(c: Choice, run: &Run) -> f32 {
    match c {
        Choice::Combo(_) | Choice::Learn(_) => 90.0,
        Choice::Heal => {
            if run.hp < run.stats().max_hp * 0.3 {
                80.0
            } else {
                5.0
            }
        }
        Choice::NewAbility(d) if synergy::offensive(d.kind) => 60.0,
        Choice::NewAbility(_) => 35.0,
        Choice::Upgrade(_) => 50.0,
        Choice::Relic(_) => 55.0,
        Choice::Talent(_) => 40.0,
        Choice::Rune(..) => 38.0,
        Choice::NewPassive(_) | Choice::UpPassive(_) => 30.0,
        Choice::Forget(_) => 0.0,
    }
}

/// One decision at the cards: which card, and whether to reroll first.
pub(crate) enum Decision {
    Take(usize),
    Reroll,
}

/// What the profile does with these cards. `rerolled` counts this hand's rerolls.
pub(crate) fn decide(
    p: &Profile,
    run: &Run,
    offer: &Offer,
    plan: &Plan<'_>,
    rng: &mut Rng,
    rerolled: u32,
) -> (Decision, Vec<f32>) {
    let cards = offer.choices();
    let n = cards.len().max(1);
    let scores: Vec<f32> = cards
        .iter()
        .map(|&c| match p.picks {
            Picks::Random => 1.0,
            Picks::Casual => casual_score(c, run) + rng.range(0.0, 25.0),
            Picks::Expert => {
                expert_score(c, run, plan) + learned(c, run, plan) + rng.range(0.0, 6.0)
            }
        })
        .collect();
    let best = (0..scores.len())
        .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
        .unwrap_or(0);
    let decision = match p.picks {
        Picks::Random => Decision::Take(rng.index(n)),
        Picks::Casual => Decision::Take(best),
        Picks::Expert => {
            if scores[best] < 45.0 && run.rerolls > 0 && rerolled < 2 {
                Decision::Reroll
            } else {
                Decision::Take(best)
            }
        }
    };
    (decision, scores)
}

/// A card's short name, for the logs.
pub(crate) fn card_key(c: Choice, run: &Run) -> String {
    match c {
        Choice::NewAbility(d) => format!("new:{}", d.key),
        Choice::Upgrade(k) => format!("up:{}", run.abilities[k].def.key),
        Choice::NewPassive(p) => format!("passive:{}", p.key),
        Choice::UpPassive(k) => format!("passive:{}", run.passives[k].0.key),
        Choice::Rune(k, r) => format!("enchant:{}@{}", r.key, run.abilities[k].def.key),
        Choice::Talent(t) => format!("talent:{}", t.key),
        Choice::Relic(r) => format!("item:{}", r.key),
        Choice::Learn(t) => format!("learn:{}", t.capstone),
        Choice::Combo(c) => format!("combo:{}", c.key),
        Choice::Forget(k) => format!("forget:{}", run.abilities[k].def.key),
        Choice::Heal => "heal".into(),
    }
}

/// A card stripped of what it is attached to: the key a lift is measured by.
pub(crate) fn card_family(key: &str) -> &str {
    match key.split_once('@') {
        Some((k, _)) => k,
        None => key,
    }
}

/// The spells of the class, for the reports.
pub(crate) fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Bolt => "bolt",
        Kind::Strike => "strike",
        Kind::Melee => "melee",
        Kind::Nova => "nova",
        Kind::Cone => "cone",
        Kind::Aura => "aura",
        Kind::Ground => "ground",
        Kind::GroundSelf => "ground_self",
        Kind::Chain => "chain",
        Kind::Dot => "dot",
        Kind::Orbit => "orbit",
        Kind::Shield => "shield",
        Kind::Immune => "immune",
        Kind::Thorns => "thorns",
        Kind::Heal => "heal",
        Kind::Totem => "totem",
    }
}

pub(crate) fn tree_count(class: u8) -> u8 {
    synergy::trees(class).count().max(1) as u8
}

pub(crate) fn tree_name(class: u8, tree: u8) -> &'static str {
    synergy::tree_def(class, tree).map_or("?", |t| t.name)
}

pub(crate) fn class_name(id: u8) -> &'static str {
    data::CLASSES.iter().find(|c| c.id == id).map_or("?", |c| c.name)
}
