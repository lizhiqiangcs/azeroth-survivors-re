//! The specialization's three hand-cast spells, learned with the specialization: a burst on `Q`,
//! a guard on `E`, a move on `F`. Never cast by themselves; when to press them is the player's.
//!
//! Each burst works the spec's own engine rather than only adding damage: Fury's Death Wish fills
//! the rage bar and Enrages at once, Fire's Combustion sets off every Ignite in reach, Frost's
//! Cold Snap freezes the field for Shatter, Affliction's Drain Soul cashes in every DoT, and so on.
//!
//! What a cast does to the run (the buffs, the resources, the cooldowns) is [`Run::activate`], and
//! what it does to each enemy in reach is [`touch`]: the game's system here and the balance
//! simulator both call them, so a change made here plays in both.

use bevy::prelude::*;

use super::abilities::Hit;
use super::data::Effect;
use super::enemies::{Enemy, Status};
use super::hero::{self, Hero};
use super::mobility::{Mobility, Move};
use super::synergy::{self, Req, School};
use super::units::Fx;
use super::{Phase, Run};

pub(super) const BURST_KEY: KeyCode = KeyCode::KeyQ;
pub(super) const GUARD_KEY: KeyCode = KeyCode::KeyE;

/// What a running burst or guard changes, folded into [`Run::stats`] and [`Run::power`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Buff {
    /// Damage multiplier.
    pub might: f32,
    pub haste: f32,
    pub crit: f32,
    /// Damage taken multiplier.
    pub taken: f32,
    pub dodge: f32,
    /// Health per second, as a fraction of the most.
    pub regen: f32,
    /// Fraction of damage dealt healed.
    pub leech: f32,
    /// Damage over time multiplier.
    pub dot: f32,
}

pub(super) const NO_BUFF: Buff = Buff {
    might: 1.0,
    haste: 0.0,
    crit: 0.0,
    taken: 1.0,
    dodge: 0.0,
    regen: 0.0,
    leech: 0.0,
    dot: 1.0,
};

/// A status a burst cashes in on every enemy in reach.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Det {
    /// Ignite: the burn still to come, at once.
    Burn,
    /// Every DoT's damage still to come, at once.
    Dots,
    /// Sunder Armor: damage per stack, and a stun.
    Sunder,
    /// Judgement: holy damage to each judged enemy, and a heal per one.
    Judged,
}

/// What a cast does beyond its buff.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Link {
    /// Rage to full: Enrage at once.
    FillRage,
    /// Five combo points: the next strike is a finisher.
    FillCombo,
    /// Arcane charges to the cap.
    FillArcane,
    /// Five soul shards: Soul Fire is due.
    FillShards,
    /// Holy Power to the cap, held for the buff's length.
    FillHolyPower,
    /// Both eclipses, for the buff's length.
    Eclipse,
    /// Every spell's cooldown times this.
    Reset(f32),
    /// The class surge (Enrage, Rapid Fire) for this long.
    Surge(f32),
    /// Every cast a critical strike while it runs.
    AlwaysCrit,
    /// Every cast echoes while it runs.
    Echo,
    /// The Hunter's Mark falls on every enemy in 30 yards while it runs.
    MarkAll,
    Immune,
    /// A shield of this fraction of the most health.
    Absorb(f32),
    /// Blows taken strike back for this much, scaled by level.
    Thorns(f32),
    /// Heal this fraction of the most health at once.
    Heal(f32),
    /// Around the hero, in this radius: cash in a status at this power.
    Detonate(Det, f32, f32),
    /// Around the hero: freeze solid (bosses: chilled to the brink).
    Freeze(f32),
    /// Around the hero: damage (base, scaled by level) and an effect.
    Nova(f32, f32, Effect),
}

pub(super) struct ActiveDef {
    pub key: &'static str,
    pub name: &'static str,
    /// The icon, and the spell visual on the hero.
    pub spell: u32,
    pub cooldown: f32,
    /// How long the buff runs; 0 for an instant.
    pub secs: f32,
    pub school: School,
    pub buff: Buff,
    pub links: &'static [Link],
    pub desc: &'static str,
}

/// A specialization's three.
pub(super) struct Kit {
    pub class: u8,
    pub tree: u8,
    pub burst: ActiveDef,
    pub guard: ActiveDef,
    pub mobility: Mobility,
}

/// The burst's and guard's cooldowns and running time, on [`Run`]; index 0 the burst, 1 the guard.
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct ActState {
    pub cd: [f32; 2],
    pub t: [f32; 2],
    /// Casts of each this run, for the reports.
    pub casts: [u32; 2],
}

/// The level scale of a nova's or a thorn's base number.
fn level_scale(run: &Run) -> f32 {
    1.0 + 0.12 * run.level as f32
}

impl Run {
    /// The specialization taken, if any.
    pub(super) fn spec_tree(&self) -> Option<u8> {
        self.talents.iter().find_map(|t| match t.req {
            Req::Tree(_, tree, 0) => Some(tree),
            _ => None,
        })
    }

    pub(super) fn kit(&self) -> Option<&'static Kit> {
        let tree = self.spec_tree()?;
        KITS.iter()
            .find(|k| k.class == self.class.id && k.tree == tree)
    }

    /// The burst (0) or the guard (1), once specialized.
    pub(super) fn active(&self, slot: usize) -> Option<&'static ActiveDef> {
        self.kit().map(|k| if slot == 0 { &k.burst } else { &k.guard })
    }

    /// The running bursts' and guards' buffs, together.
    pub(super) fn act_buff(&self) -> Buff {
        let mut b = NO_BUFF;
        for slot in 0..2 {
            if self.act.t[slot] <= 0.0 {
                continue;
            }
            let Some(def) = self.active(slot) else { continue };
            let a = def.buff;
            b.might *= a.might;
            b.haste += a.haste;
            b.crit += a.crit;
            b.taken *= a.taken;
            b.dodge += a.dodge;
            b.regen += a.regen;
            b.leech += a.leech;
            b.dot *= a.dot;
        }
        b
    }

    /// Whether a running burst or guard carries this link.
    pub(super) fn act_link(&self, link: Link) -> bool {
        (0..2).any(|slot| {
            self.act.t[slot] > 0.0
                && self
                    .active(slot)
                    .is_some_and(|d| d.links.contains(&link))
        })
    }

    pub(super) fn tick_actives(&mut self, dt: f32) {
        for slot in 0..2 {
            self.act.cd[slot] = (self.act.cd[slot] - dt).max(0.0);
            self.act.t[slot] = (self.act.t[slot] - dt).max(0.0);
        }
    }

    /// Cast the burst (0) or the guard (1) if it is ready: its buff starts and what it does to
    /// the run is done. Returns it, for the caller to apply its reach on the enemies ([`touch`]).
    pub(super) fn activate(&mut self, slot: usize) -> Option<&'static ActiveDef> {
        let def = self.active(slot)?;
        if self.act.cd[slot] > 0.0 || self.hp <= 0.0 {
            return None;
        }
        self.act.cd[slot] = def.cooldown;
        self.act.t[slot] = def.secs;
        self.act.casts[slot] += 1;
        let max = self.stats().max_hp;
        let scale = level_scale(self);
        for &link in def.links {
            match link {
                Link::FillRage if self.class.id == 1 => self.b.res = 100.0,
                Link::FillCombo if self.class.id == 4 => self.b.res = 5.0,
                Link::FillArcane => {
                    self.b.arcane = self.arcane_cap();
                    self.b.arcane_t = 8.0;
                }
                Link::FillShards if self.class.id == 9 => {
                    self.b.res = 5.0;
                    self.b.soul_fire = true;
                }
                Link::FillHolyPower if self.class.id == 2 => {
                    self.b.res = if self.has_talent("vengeance") { 12.0 } else { 8.0 };
                    self.b.res_t = def.secs.max(6.0);
                }
                Link::Eclipse => {
                    self.b.lunar_t = self.b.lunar_t.max(def.secs);
                    self.b.solar_t = self.b.solar_t.max(def.secs);
                }
                Link::Reset(f) => {
                    for o in &mut self.abilities {
                        o.cd *= f;
                    }
                }
                Link::Surge(s) => self.b.surge_t = self.b.surge_t.max(s),
                Link::Immune => self.immune_t = self.immune_t.max(def.secs),
                Link::Absorb(f) => {
                    self.shield = self.shield.max(max * f);
                    self.shield_t = self.shield_t.max(def.secs.max(8.0));
                }
                Link::Thorns(base) => {
                    self.thorns = base * scale;
                    self.thorns_t = def.secs;
                    self.thorns_spell = def.spell;
                }
                Link::Heal(f) => self.hp = (self.hp + max * f).min(max),
                _ => {}
            }
        }
        self.version += 1;
        Some(def)
    }

    /// The burst or guard's cooldown fraction left, for the button's sweep.
    pub(super) fn act_sweep(&self, slot: usize) -> f32 {
        self.active(slot)
            .map_or(0.0, |d| (self.act.cd[slot] / d.cooldown.max(0.01)).clamp(0.0, 1.0))
    }
}

/// What a cast's reach does to one enemy.
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct Touch {
    pub damage: f32,
    pub effect: Option<Effect>,
    /// Heal the hero this fraction of the most health.
    pub heal: f32,
    /// The enemy's DoTs were cashed in: the caller clears them.
    pub clear_dots: bool,
}

/// The reach of a cast's links on one enemy within `dist` of the hero: its status `st`, the
/// damage its DoTs still have to do `dots_left`. Freezes are made on `st` here; the damage and
/// the effect are for the caller to land as a hit.
pub(super) fn touch(
    def: &ActiveDef,
    run: &Run,
    dist: f32,
    st: &mut Status,
    dots_left: f32,
    boss: bool,
) -> Touch {
    let scale = level_scale(run) * run.stats().might * run.power(def.school.bit());
    let mut out = Touch::default();
    for &link in def.links {
        match link {
            Link::Detonate(kind, r, power) if dist <= r => match kind {
                Det::Burn if st.burn_t > 0.0 => {
                    out.damage += st.burn * st.burn_t.max(1.0) * power;
                    st.burn = 0.0;
                    st.burn_t = 0.0;
                }
                Det::Dots if dots_left > 0.0 => {
                    out.damage += dots_left * power;
                    out.clear_dots = true;
                }
                Det::Sunder if st.sunder > 0 => {
                    out.damage += power * f32::from(st.sunder) * scale;
                    out.effect = Some(Effect::Stun(2.0));
                }
                Det::Judged if st.judged_t > 0.0 => {
                    out.damage += power * scale;
                    out.heal += 0.01;
                }
                _ => {}
            },
            Link::Freeze(r) if dist <= r => {
                if boss {
                    st.chill = st.chill.max(3);
                    st.chill_t = 4.0;
                } else {
                    st.chill = 0;
                    st.frozen_t = st.frozen_t.max(2.5 * run.stats().cc);
                }
            }
            Link::Nova(r, base, effect) if dist <= r => {
                out.damage += base * scale;
                if effect != Effect::None {
                    out.effect = Some(effect);
                }
            }
            _ => {}
        }
    }
    out
}

/// The farthest reach of a cast's links: who [`touch`] is asked about.
pub(super) fn reach(def: &ActiveDef) -> f32 {
    def.links
        .iter()
        .map(|l| match *l {
            Link::Detonate(_, r, _) | Link::Freeze(r) | Link::Nova(r, ..) => r,
            _ => 0.0,
        })
        .fold(0.0, f32::max)
}

/// `Q` and `E`: cast the burst or the guard; ticks their timers.
#[allow(clippy::too_many_arguments)]
pub(super) fn cast_actives(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    phase: Res<State<Phase>>,
    mut run: ResMut<Run>,
    hero: Query<(Entity, &Transform), With<Hero>>,
    mut enemies: Query<(Entity, &Transform, &mut Enemy), Without<Hero>>,
    mut fx: Fx,
    mut hits: MessageWriter<Hit>,
) {
    run.tick_actives(time.delta_secs());
    if *phase.get() != Phase::Playing || run.dead_t.is_some() {
        return;
    }
    for (slot, key) in [BURST_KEY, GUARD_KEY].into_iter().enumerate() {
        if keys.just_pressed(key) {
            cast(slot, &mut run, &hero, &mut enemies, &mut fx, &mut hits);
        }
    }
}

fn cast(
    slot: usize,
    run: &mut Run,
    hero: &Query<(Entity, &Transform), With<Hero>>,
    enemies: &mut Query<(Entity, &Transform, &mut Enemy), Without<Hero>>,
    fx: &mut Fx,
    hits: &mut MessageWriter<Hit>,
) {
    let Some(def) = run.activate(slot) else { return };
    let Ok((me, tf)) = hero.single() else { return };
    let pos = tf.translation;
    fx.cast(me, def.spell, &[me], None);
    let r = reach(def);
    if r <= 0.0 {
        return;
    }
    let mut heal = 0.0;
    for (e, t, mut en) in enemies.iter_mut() {
        if en.dead {
            continue;
        }
        let dist = hero::reach(pos, t.translation);
        if dist > r {
            continue;
        }
        let dots_left: f32 = en.dots.iter().map(|d| d.dps * d.left.max(0.0)).sum();
        let boss = en.boss;
        let t = touch(def, run, dist, &mut en.st, dots_left, boss);
        if t.clear_dots {
            en.dots.clear();
        }
        heal += t.heal;
        if t.damage > 0.0 || t.effect.is_some() {
            hits.write(Hit {
                effect: t.effect.unwrap_or(Effect::None),
                flags: synergy::F_PROC,
                ..Hit::proc(e, t.damage, def.school)
            });
        }
    }
    if heal > 0.0 {
        let max = run.stats().max_hp;
        run.leech(heal * max);
    }
}

// --- The kits -----------------------------------------------------------------------------------

const fn b() -> Buff {
    NO_BUFF
}

macro_rules! active {
    ($key:literal, $name:literal, $spell:literal, $cd:expr, $secs:expr, $school:ident,
     { $($f:ident: $v:expr),* $(,)? }, [$($link:expr),* $(,)?], $desc:literal) => {
        ActiveDef {
            key: $key,
            name: $name,
            spell: $spell,
            cooldown: $cd,
            secs: $secs,
            school: School::$school,
            buff: Buff { $($f: $v,)* ..b() },
            links: &[$($link),*],
            desc: $desc,
        }
    };
}

const fn mv(spell: u32, name: &'static str, kind: Move, cooldown: f32, secs: f32, desc: &'static str) -> Mobility {
    Mobility {
        spell,
        name,
        kind,
        cooldown,
        secs,
        desc,
    }
}

use Det::*;
use Link::*;

pub(super) static KITS: &[Kit] = &[
    // --- Warrior: rage, Enrage, Sunder Armor ---------------------------------------------------
    Kit {
        class: 1,
        tree: 0,
        burst: active!("recklessness", "Recklessness", 1719, 50.0, 10.0, Physical,
            { might: 1.15 }, [AlwaysCrit],
            "For 10 sec every strike is a critical strike: Deep Wounds bleeds and Flurry quickens on every blow. 15% more damage."),
        guard: active!("retaliation", "Retaliation", 20230, 35.0, 10.0, Physical,
            { taken: 0.75 }, [Thorns(18.0)],
            "For 10 sec every blow against you is struck back, and you take 25% less damage."),
        mobility: mv(100, "Charge", Move::Charge(25.0), 8.0, 1.5,
            "Charge the enemy nearest the cursor within 25 yards, stunning it."),
    },
    Kit {
        class: 1,
        tree: 1,
        burst: active!("death_wish", "Death Wish", 12328, 45.0, 15.0, Physical,
            { might: 1.25, taken: 1.15 }, [FillRage],
            "Rage fills at once: you Enrage on the spot. For 15 sec you deal 25% more damage, and take 15% more."),
        guard: active!("berserker_rage", "Berserker Rage", 18499, 30.0, 8.0, Physical,
            { regen: 0.04, taken: 0.85 }, [],
            "For 8 sec you heal 4% of your health every second and take 15% less damage."),
        mobility: mv(20252, "Intercept", Move::Charge(25.0), 10.0, 3.0,
            "Intercept the enemy nearest the cursor within 25 yards, stunning it for 3 sec."),
    },
    Kit {
        class: 1,
        tree: 2,
        burst: active!("concussion_blow", "Concussion Blow", 12809, 25.0, 0.0, Physical,
            {}, [Detonate(Sunder, 10.0, 6.0)],
            "Every enemy within 10 yards takes damage for each stack of Sunder Armor on it, and is stunned for 2 sec."),
        guard: active!("shield_wall", "Shield Wall", 871, 50.0, 10.0, Physical,
            { taken: 0.3 }, [],
            "For 10 sec you take 70% less damage."),
        mobility: mv(12323, "Piercing Howl", Move::Ward(12.0, Effect::Slow(0.4, 5.0)), 15.0, 4.0,
            "Every enemy within 12 yards is slowed by 60% for 5 sec; you run 30% faster for 4 sec."),
    },
    // --- Paladin: Holy Power, Judgement ------------------------------------------------------------
    Kit {
        class: 2,
        tree: 0,
        burst: active!("divine_favor", "Divine Favor", 20216, 40.0, 8.0, Holy,
            {}, [AlwaysCrit, Detonate(Judged, 20.0, 30.0)],
            "Every judged enemy within 20 yards is struck by holy light, each one healing you 1%. For 8 sec every cast is a critical strike."),
        guard: active!("blessing_of_protection", "Blessing of Protection", 1022, 50.0, 5.0, Holy,
            {}, [Immune],
            "Nothing can harm you for 5 sec."),
        mobility: mv(1044, "Blessing of Freedom", Move::Haste(1.6), 15.0, 5.0,
            "Nothing can hold you: run 60% faster for 5 sec."),
    },
    Kit {
        class: 2,
        tree: 1,
        burst: active!("righteous_fury", "Righteous Fury", 25780, 45.0, 15.0, Holy,
            { taken: 0.8 }, [FillHolyPower, Nova(10.0, 25.0, Effect::Stun(1.5))],
            "Holy Power fills at once and holds for 15 sec. A burst of holy fire stuns everything within 10 yards; you take 20% less damage."),
        guard: active!("blessing_of_sanctuary", "Blessing of Sanctuary", 20911, 35.0, 12.0, Holy,
            { taken: 0.6 }, [Thorns(12.0)],
            "For 12 sec you take 40% less damage, and every blow against you is answered with holy damage."),
        mobility: mv(13819, "Summon Warhorse", Move::Haste(1.7), 16.0, 5.0,
            "Mount up: run 70% faster for 5 sec."),
    },
    Kit {
        class: 2,
        tree: 2,
        burst: active!("seal_of_the_crusader", "Seal of the Crusader", 21082, 45.0, 15.0, Holy,
            { might: 1.3 }, [FillHolyPower],
            "Holy Power fills at once and holds for 15 sec: its haste runs at full. 30% more damage for 15 sec."),
        guard: active!("divine_protection", "Divine Protection", 498, 40.0, 8.0, Holy,
            { taken: 0.5 }, [],
            "For 8 sec you take 50% less damage."),
        mobility: mv(23214, "Summon Charger", Move::Haste(1.9), 18.0, 4.0,
            "Your epic mount: run 90% faster for 4 sec."),
    },
    // --- Hunter: Hunter's Mark, Rapid Fire -----------------------------------------------------------
    Kit {
        class: 3,
        tree: 0,
        burst: active!("intimidation", "Intimidation", 19577, 40.0, 12.0, Physical,
            { might: 1.25, haste: 0.2 }, [Nova(12.0, 20.0, Effect::Stun(2.0))],
            "Your pet's roar stuns everything within 12 yards for 2 sec. For 12 sec you deal 25% more damage, 20% faster."),
        guard: active!("aspect_of_the_monkey", "Aspect of the Monkey", 13163, 30.0, 12.0, Physical,
            { dodge: 0.3 }, [],
            "For 12 sec you dodge 30% more blows."),
        mobility: mv(5118, "Aspect of the Cheetah", Move::Haste(1.7), 14.0, 5.0,
            "Run 70% faster for 5 sec."),
    },
    Kit {
        class: 3,
        tree: 1,
        burst: active!("rapid_fire", "Rapid Fire", 3045, 45.0, 15.0, Physical,
            {}, [Surge(15.0), MarkAll],
            "Rapid Fire for 15 sec (40% faster shots), and your Hunter's Mark falls on every enemy within 30 yards while it lasts."),
        guard: active!("feign_death", "Feign Death", 5384, 35.0, 3.0, Physical,
            {}, [Immune],
            "Play dead: nothing harms you for 3 sec."),
        mobility: mv(781, "Disengage", Move::Blink(15.0), 8.0, 0.0,
            "Leap 15 yards toward the cursor."),
    },
    Kit {
        class: 3,
        tree: 2,
        burst: active!("readiness", "Readiness", 23989, 50.0, 0.0, Nature,
            {}, [Reset(0.0), Detonate(Dots, 25.0, 1.0)],
            "Every spell is ready again at once, and every sting within 25 yards deals all its remaining damage at once."),
        guard: active!("deterrence", "Deterrence", 19263, 40.0, 8.0, Physical,
            { dodge: 0.5, taken: 0.7 }, [],
            "For 8 sec you dodge 50% more blows and take 30% less from the rest."),
        mobility: mv(19503, "Scatter Shot", Move::Ward(8.0, Effect::Stun(2.5)), 14.0, 3.0,
            "Everything within 8 yards is stunned for 2.5 sec; you run 30% faster for 3 sec."),
    },
    // --- Rogue: combo points, finishers --------------------------------------------------------------
    Kit {
        class: 4,
        tree: 0,
        burst: active!("cold_blood", "Cold Blood", 14177, 40.0, 8.0, Physical,
            {}, [FillCombo, AlwaysCrit],
            "Five combo points at once: your next strike is a finisher. For 8 sec every strike is a critical strike."),
        guard: active!("feint", "Feint", 1966, 25.0, 6.0, Physical,
            { taken: 0.5 }, [],
            "For 6 sec you take 50% less damage."),
        mobility: mv(2983, "Sprint", Move::Haste(1.8), 15.0, 4.0, "Run 80% faster for 4 sec."),
    },
    Kit {
        class: 4,
        tree: 1,
        burst: active!("preparation", "Preparation", 14185, 50.0, 10.0, Physical,
            { haste: 0.35 }, [Reset(0.0), FillCombo],
            "Every spell is ready again, five combo points at once, and for 10 sec you strike 35% faster."),
        guard: active!("evasion", "Evasion", 5277, 40.0, 10.0, Physical,
            { dodge: 0.5 }, [],
            "For 10 sec you dodge 50% more blows."),
        mobility: mv(1725, "Distract", Move::Ward(12.0, Effect::Fear(3.0)), 14.0, 3.0,
            "Everything within 12 yards turns away for 3 sec; you run 30% faster."),
    },
    Kit {
        class: 4,
        tree: 2,
        burst: active!("premeditation", "Premeditation", 14183, 35.0, 10.0, Physical,
            { might: 1.35 }, [FillCombo],
            "Five combo points at once, and for 10 sec you deal 35% more damage."),
        guard: active!("vanish", "Vanish", 1856, 45.0, 3.0, Physical,
            {}, [Immune],
            "Vanish: nothing harms you for 3 sec."),
        mobility: mv(2983, "Sprint", Move::Vanish(1.7), 15.0, 4.0,
            "Run 70% faster for 4 sec, and the horde loses sight of you."),
    },
    // --- Priest ----------------------------------------------------------------------------------------
    Kit {
        class: 5,
        tree: 0,
        burst: active!("power_infusion", "Power Infusion", 10060, 45.0, 15.0, Holy,
            { might: 1.2, haste: 0.4 }, [],
            "For 15 sec you cast 40% faster and deal 20% more damage."),
        guard: active!("inner_fire", "Inner Fire", 588, 30.0, 15.0, Holy,
            { taken: 0.6 }, [Absorb(0.2)],
            "For 15 sec you take 40% less damage, and a shield absorbs 20% of your health."),
        mobility: mv(1706, "Levitate", Move::Haste(1.5), 15.0, 6.0, "Float 50% faster for 6 sec."),
    },
    Kit {
        class: 5,
        tree: 1,
        burst: active!("inner_focus", "Inner Focus", 14751, 40.0, 8.0, Holy,
            { might: 1.15 }, [AlwaysCrit],
            "For 8 sec every spell is a critical strike: holy crits burn with Holy Fire and heal you through Atonement."),
        guard: active!("desperate_prayer", "Desperate Prayer", 13908, 40.0, 0.0, Holy,
            {}, [Heal(0.4)],
            "Heal 40% of your health at once."),
        mobility: mv(586, "Fade", Move::Vanish(1.4), 18.0, 4.0,
            "Fade from the horde's sight: enemies lose you and you run 40% faster for 4 sec."),
    },
    Kit {
        class: 5,
        tree: 2,
        burst: active!("shadowform", "Shadowform", 15473, 45.0, 15.0, Shadow,
            { might: 1.2, dot: 1.6 }, [],
            "For 15 sec your damage over time does 60% more, and all your damage 20% more: every affliction stacks Shadow Weaving faster."),
        guard: active!("vampiric_embrace", "Vampiric Embrace", 15286, 30.0, 10.0, Shadow,
            { leech: 0.12 }, [],
            "For 10 sec you heal for 12% of all the damage you deal."),
        mobility: mv(453, "Mind Soothe", Move::Ward(10.0, Effect::Fear(3.0)), 15.0, 3.0,
            "Everything within 10 yards loses interest for 3 sec; you run 30% faster."),
    },
    // --- Shaman: Overload ------------------------------------------------------------------------------
    Kit {
        class: 7,
        tree: 0,
        burst: active!("elemental_mastery_q", "Elemental Mastery", 16166, 45.0, 10.0, Nature,
            { might: 1.1 }, [Echo],
            "For 10 sec every spell Overloads: it casts twice."),
        guard: active!("stoneskin_totem", "Stoneskin Totem", 8071, 35.0, 12.0, Nature,
            { taken: 0.6 }, [],
            "For 12 sec you take 40% less damage."),
        mobility: mv(2645, "Ghost Wolf", Move::Haste(1.6), 15.0, 6.0,
            "Take the form of a ghost wolf, running 60% faster for 6 sec."),
    },
    Kit {
        class: 7,
        tree: 1,
        burst: active!("windfury_totem", "Windfury Totem", 8512, 40.0, 12.0, Nature,
            { might: 1.15, haste: 0.5 }, [],
            "For 12 sec you strike 50% faster and deal 15% more damage."),
        guard: active!("grounding_totem", "Grounding Totem", 8177, 30.0, 6.0, Nature,
            { taken: 0.5 }, [Thorns(10.0)],
            "For 6 sec you take 50% less damage and blows against you are struck back with lightning."),
        mobility: mv(2645, "Ghost Wolf", Move::Haste(1.8), 14.0, 5.0,
            "Take the form of a ghost wolf, running 80% faster for 5 sec."),
    },
    Kit {
        class: 7,
        tree: 2,
        burst: active!("mana_tide_totem", "Mana Tide Totem", 16190, 45.0, 12.0, Nature,
            { haste: 0.3 }, [Reset(0.3)],
            "Your spells are nearly ready again, and for 12 sec you cast 30% faster."),
        guard: active!("healing_stream_totem", "Healing Stream Totem", 5394, 30.0, 10.0, Nature,
            { regen: 0.04 }, [],
            "For 10 sec you heal 4% of your health every second."),
        mobility: mv(2484, "Earthbind Totem", Move::Ward(12.0, Effect::Root(3.0)), 15.0, 4.0,
            "Everything within 12 yards is held fast for 3 sec; you run 30% faster."),
    },
    // --- Mage: Arcane charges, Ignite, Winter's Chill ---------------------------------------------------
    Kit {
        class: 8,
        tree: 0,
        burst: active!("evocation", "Evocation", 12051, 45.0, 12.0, Arcane,
            { might: 1.1 }, [FillArcane, Reset(0.0)],
            "Arcane charges fill at once and every spell is ready again; 10% more damage for 12 sec."),
        guard: active!("mana_shield", "Mana Shield", 1463, 30.0, 12.0, Arcane,
            {}, [Absorb(0.4)],
            "A shield absorbs damage up to 40% of your health."),
        mobility: mv(1953, "Blink", Move::Blink(22.0), 8.0, 0.0,
            "Teleport 22 yards toward the cursor."),
    },
    Kit {
        class: 8,
        tree: 1,
        burst: active!("combustion", "Combustion", 11129, 40.0, 10.0, Fire,
            { crit: 0.5 }, [Detonate(Burn, 25.0, 4.0)],
            "Every Ignite within 25 yards bursts: four times its remaining burn, all at once. 50% more critical strike chance for 10 sec."),
        guard: active!("fire_ward", "Fire Ward", 543, 30.0, 10.0, Fire,
            { taken: 0.65 }, [Thorns(14.0)],
            "For 10 sec you take 35% less damage, and blows against you burn the attacker."),
        mobility: mv(1953, "Blink", Move::Blink(18.0), 10.0, 0.0,
            "Teleport 18 yards toward the cursor."),
    },
    Kit {
        class: 8,
        tree: 2,
        burst: active!("cold_snap", "Cold Snap", 12472, 45.0, 0.0, Frost,
            {}, [Freeze(15.0), Reset(0.0)],
            "Everything within 15 yards freezes solid, ready to Shatter, and every spell is ready again."),
        guard: active!("ice_block", "Ice Block", 11958, 50.0, 5.0, Frost,
            { regen: 0.05 }, [Immune],
            "Encased in ice for 5 sec: nothing harms you, and you heal 5% a second."),
        mobility: mv(1953, "Blink", Move::Blink(18.0), 10.0, 0.0,
            "Teleport 18 yards toward the cursor."),
    },
    // --- Warlock: soul shards -------------------------------------------------------------------------
    Kit {
        class: 9,
        tree: 0,
        burst: active!("drain_soul", "Drain Soul", 1120, 35.0, 0.0, Shadow,
            {}, [Detonate(Dots, 25.0, 1.0)],
            "Every DoT within 25 yards deals all its remaining damage at once: the afflicted dead feed your soul shards."),
        guard: active!("shadow_ward", "Shadow Ward", 6229, 30.0, 12.0, Shadow,
            {}, [Absorb(0.35)],
            "A shield absorbs damage up to 35% of your health."),
        mobility: mv(18223, "Curse of Exhaustion", Move::Ward(15.0, Effect::Slow(0.5, 5.0)), 14.0, 4.0,
            "Everything within 15 yards is slowed by half for 5 sec; you run 30% faster."),
    },
    Kit {
        class: 9,
        tree: 1,
        burst: active!("demonic_sacrifice", "Demonic Sacrifice", 18788, 45.0, 12.0, Shadow,
            { might: 1.25 }, [FillShards],
            "Five soul shards at once: Soul Fire hurls itself at the toughest enemy. 25% more damage for 12 sec."),
        guard: active!("soul_link", "Soul Link", 19028, 35.0, 15.0, Shadow,
            { taken: 0.7, regen: 0.015 }, [],
            "Your demon shares the pain: for 15 sec you take 30% less damage and heal 1.5% a second."),
        mobility: mv(5784, "Summon Felsteed", Move::Haste(1.7), 16.0, 5.0,
            "Mount your felsteed: run 70% faster for 5 sec."),
    },
    Kit {
        class: 9,
        tree: 2,
        burst: active!("shadowburn", "Shadowburn", 17877, 40.0, 8.0, Fire,
            {}, [FillShards, AlwaysCrit, Detonate(Burn, 20.0, 2.0)],
            "Five soul shards at once (Soul Fire), every burn within 20 yards flares out for twice its remaining damage, and for 8 sec every spell crits."),
        guard: active!("demon_armor", "Demon Armor", 706, 35.0, 15.0, Shadow,
            { taken: 0.75, regen: 0.02 }, [],
            "For 15 sec you take 25% less damage and heal 2% a second."),
        mobility: mv(5782, "Fear", Move::Ward(8.0, Effect::Fear(4.0)), 14.0, 3.0,
            "Everything within 8 yards flees in fear for 4 sec; you run 30% faster."),
    },
    // --- Druid: eclipses -------------------------------------------------------------------------------
    Kit {
        class: 11,
        tree: 0,
        burst: active!("natures_grace", "Nature's Grace", 16880, 45.0, 12.0, Nature,
            { might: 1.1 }, [Eclipse],
            "Both eclipses at once for 12 sec: Arcane and Nature both empowered."),
        guard: active!("barkskin", "Barkskin", 22812, 35.0, 12.0, Nature,
            { taken: 0.5 }, [],
            "For 12 sec you take 50% less damage."),
        mobility: mv(783, "Travel Form", Move::Haste(1.7), 15.0, 6.0,
            "Run 70% faster for 6 sec."),
    },
    Kit {
        class: 11,
        tree: 1,
        burst: active!("tigers_fury", "Tiger's Fury", 5217, 35.0, 10.0, Physical,
            { might: 1.3, haste: 0.3 }, [Reset(0.5)],
            "Your spells are halfway ready again, and for 10 sec you deal 30% more damage, 30% faster."),
        guard: active!("frenzied_regeneration", "Frenzied Regeneration", 22842, 35.0, 8.0, Nature,
            { regen: 0.05 }, [],
            "For 8 sec you heal 5% of your health every second."),
        mobility: mv(1850, "Dash", Move::Haste(1.8), 15.0, 4.0, "Run 80% faster for 4 sec."),
    },
    Kit {
        class: 11,
        tree: 2,
        burst: active!("innervate", "Innervate", 29166, 45.0, 12.0, Nature,
            { haste: 0.35 }, [Reset(0.0)],
            "Every spell is ready again, and for 12 sec you cast 35% faster."),
        guard: active!("regrowth", "Regrowth", 8936, 30.0, 10.0, Nature,
            { regen: 0.02 }, [Heal(0.3)],
            "Heal 30% of your health at once, and 2% a second for 10 sec."),
        mobility: mv(16689, "Nature's Grasp", Move::Ward(10.0, Effect::Root(3.0)), 15.0, 4.0,
            "Everything within 10 yards is rooted for 3 sec; you run 30% faster."),
    },
];
