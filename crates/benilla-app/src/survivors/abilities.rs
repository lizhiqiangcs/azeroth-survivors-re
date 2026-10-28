//! Auto-cast: every owned ability fires on its own cooldown at the targets its kind picks, with
//! the spell's own visuals; damage lands when its missile would. As it lands, the hit's schools
//! leave their statuses ([`super::synergy`]), and those statuses feed every later hit.

use bevy::prelude::*;

use super::actives;
use super::data::{AbilityDef, Effect, Kind};
use super::enemies::{self, Enemy, Loot, Status};
use super::hero::{self, Hero};
use super::synergy::{self, has, School};
use super::units::{self, flat, Fx, Marks, UnitSpec, COLOR_HEAL, COLOR_MELEE, COLOR_SPELL};
use super::{Rng, Run, RunEntity, Stats};
use crate::net::{FieldChanged, Guid, ObjectStore};

/// No ability: a proc, a relic, thorns.
pub(super) const NO_SRC: u8 = u8::MAX;

/// One blow landing on one enemy.
#[derive(Message, Clone)]
pub(super) struct Hit {
    pub target: Entity,
    pub damage: f32,
    pub dot: f32,
    pub dot_dur: f32,
    pub effect: Effect,
    pub mark: u32,
    pub mark_dur: f32,
    /// Also hits everything this close to the target, for less.
    pub splash: f32,
    pub crit: bool,
    pub color: u32,
    /// Its schools, as a mask ([`School::bit`]).
    pub schools: u8,
    /// [`synergy::F_PROC`] and the rune flags.
    pub flags: u16,
    /// The owned ability it came from, or [`NO_SRC`].
    pub src: u8,
    /// Sets a burn of at least this much per second.
    pub burn: f32,
}

impl Hit {
    pub fn direct(target: Entity, damage: f32, crit: bool) -> Self {
        Self {
            target,
            damage,
            dot: 0.0,
            dot_dur: 0.0,
            effect: Effect::None,
            mark: 0,
            mark_dur: 0.0,
            splash: 0.0,
            crit,
            color: COLOR_MELEE,
            schools: School::Physical.bit(),
            flags: 0,
            src: NO_SRC,
            burn: 0.0,
        }
    }

    /// A proc's hit: a school, no further procs.
    pub fn proc(target: Entity, damage: f32, school: School) -> Self {
        Self {
            color: school.color(),
            schools: school.bit(),
            ..Self::direct(target, damage, false)
        }
    }
}

/// Hits in flight, by the run time they land.
#[derive(Resource, Default)]
pub(super) struct PendingHits(pub Vec<(f32, Hit)>);

/// A ground spell's area, ticking on whatever stands in it.
#[derive(Component)]
pub(super) struct GroundArea {
    damage: f32,
    dot: f32,
    dot_dur: f32,
    radius: f32,
    left: f32,
    every: f32,
    next_tick: f32,
    effect: Effect,
    mark: u32,
    /// The spell whose impact plays on each enemy a tick hits, 0 for none.
    impact: u32,
    schools: u8,
    flags: u16,
    src: u8,
    /// Moves with the hero (Rune of the Tether).
    follow: bool,
}

#[derive(Component)]
pub(super) struct Totem {
    cast: u32,
    damage: f32,
    range: f32,
    radius: f32,
    left: f32,
    tick: f32,
    pulse: bool,
    /// Seconds between attacks: halved by Totemic Mastery.
    every: f32,
    schools: u8,
}

#[derive(Clone, Copy)]
pub(super) struct Foe {
    pub e: Entity,
    pub pos: Vec3,
    pub hp: f32,
    pub dist: f32,
    pub radius: f32,
}

/// The numbers one cast uses, from the ability's level, the hero's stats and the build.
struct Params {
    dmg: f32,
    dot: f32,
    count: usize,
    radius: f32,
    duration: f32,
    cooldown: f32,
    schools: u8,
    flags: u16,
    src: u8,
    /// Crowd control's length, as a multiplier.
    cc: f32,
    /// Every hit of this cast crits.
    crit: bool,
}

fn mark_secs(def: &AbilityDef, duration: f32) -> f32 {
    match def.effect {
        Effect::Slow(_, t) | Effect::Weaken(_, t) => t,
        Effect::Root(t) | Effect::Stun(t) | Effect::Fear(t) => t,
        _ if duration > 0.0 => duration,
        _ => 3.0,
    }
}

/// An effect with its crowd control lengthened.
fn scaled(effect: Effect, cc: f32) -> Effect {
    match effect {
        Effect::Root(t) => Effect::Root(t * cc),
        Effect::Stun(t) => Effect::Stun(t * cc),
        Effect::Fear(t) => Effect::Fear(t * cc),
        e => e,
    }
}

fn hit_for(def: &AbilityDef, p: &Params, target: Entity, damage: f32, crit: bool) -> Hit {
    Hit {
        target,
        damage,
        dot: p.dot,
        dot_dur: p.duration,
        effect: scaled(def.effect, p.cc),
        mark: def.mark,
        mark_dur: mark_secs(def, p.duration) * p.cc,
        splash: if matches!(def.kind, Kind::Bolt | Kind::Strike) {
            p.radius
        } else {
            0.0
        },
        crit,
        color: if def.kind == Kind::Melee {
            COLOR_MELEE
        } else if p.schools == School::Physical.bit() {
            COLOR_SPELL
        } else {
            synergy::primary(p.schools).color()
        },
        schools: p.schools,
        flags: p.flags,
        src: p.src,
        burn: 0.0,
    }
}

fn schedule(
    pending: &mut PendingHits,
    hits: &mut MessageWriter<Hit>,
    now: f32,
    delay: f32,
    hit: Hit,
) {
    if delay <= 0.0 {
        hits.write(hit);
    } else {
        pending.0.push((now + delay, hit));
    }
}

pub(super) fn roll(rng: &mut Rng, stats: &Stats, dmg: f32) -> (f32, bool) {
    if rng.f32() < stats.crit {
        (dmg * stats.crit_mult, true)
    } else {
        (dmg, false)
    }
}

/// A cast's roll: a forced crit, or the usual dice.
fn roll_cast(rng: &mut Rng, stats: &Stats, p: &Params, dmg: f32) -> (f32, bool) {
    if p.crit {
        (dmg * stats.crit_mult, true)
    } else {
        roll(rng, stats, dmg)
    }
}

pub(super) fn nearest(foes: &[Foe], range: f32, n: usize) -> Vec<Foe> {
    foes.iter()
        .filter(|f| f.dist <= range + f.radius)
        .take(n)
        .copied()
        .collect()
}

pub(super) fn toughest(foes: &[Foe], range: f32, n: usize) -> Vec<Foe> {
    let mut v: Vec<Foe> = foes
        .iter()
        .filter(|f| f.dist <= range + f.radius)
        .copied()
        .collect();
    v.sort_by(|a, b| b.hp.total_cmp(&a.hp));
    v.truncate(n);
    v
}

pub(super) fn around(foes: &[Foe], center: Vec3, r: f32) -> Vec<Foe> {
    foes.iter()
        .filter(|f| flat(f.pos - center).length() <= r + f.radius * 0.5)
        .copied()
        .collect()
}

/// The point among the nearest enemies in range with the most others within `r` of it, away from
/// `avoid` (a twin keeps off its sibling).
pub(super) fn densest(foes: &[Foe], range: f32, r: f32, avoid: Option<Vec3>) -> Option<Vec3> {
    let near: Vec<&Foe> = foes
        .iter()
        .filter(|f| f.dist <= range)
        .filter(|f| avoid.is_none_or(|a| flat(f.pos - a).length() > r * 1.5))
        .take(40)
        .collect();
    near.iter()
        .map(|f| {
            (
                f.pos,
                near.iter()
                    .filter(|o| flat(o.pos - f.pos).length() <= r)
                    .count(),
            )
        })
        .max_by_key(|&(_, n)| n)
        .map(|(p, _)| p)
}

fn entities(foes: &[Foe]) -> Vec<Entity> {
    foes.iter().map(|f| f.e).collect()
}

/// One cast this frame: the ability, a damage factor, and whether it is an extra (an echo, an
/// overload, a reprisal) that leaves the cooldown alone.
struct Due {
    i: usize,
    factor: f32,
    extra: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cast_abilities(
    mut commands: Commands,
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut rng: ResMut<Rng>,
    spells: Option<Res<crate::ui_action::Spells>>,
    hero: Query<(Entity, &Transform), With<Hero>>,
    enemies: Query<(Entity, &Transform, &Enemy)>,
    mut fx: Fx,
    mut pending: ResMut<PendingHits>,
    mut hits: MessageWriter<Hit>,
) {
    let dt = time.delta_secs();
    if run.dead_t.is_some() || dt <= 0.0 {
        return;
    }
    let Ok((me, htf)) = hero.single() else { return };
    let run = &mut *run;
    let hpos = htf.translation;
    let stats = run.stats();
    let now = run.elapsed;
    let mut foes: Vec<Foe> = enemies
        .iter()
        .filter(|(_, _, e)| !e.dead)
        .map(|(e, tf, en)| Foe {
            e,
            pos: tf.translation,
            hp: en.hp,
            dist: hero::reach(hpos, tf.translation),
            radius: en.radius,
        })
        .collect();
    foes.sort_by(|a, b| a.dist.total_cmp(&b.dist));
    let speed_of = |spell: u32| {
        spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell))
            .map_or(0.0, |d| d.speed)
    };

    // What fires this frame: the ready, the echoes due, and the reprisals a blow set off.
    let struck = std::mem::take(&mut run.b.struck);
    let mut due = Vec::new();
    for i in 0..run.abilities.len() {
        let o = &mut run.abilities[i];
        o.cd -= dt;
        o.reprisal_cd -= dt;
        if let Some((t, factor)) = o.echo.as_mut() {
            *t -= dt;
            if *t <= 0.0 {
                let factor = *factor;
                o.echo = None;
                due.push(Due {
                    i,
                    factor,
                    extra: true,
                });
            }
        }
        if struck && o.reprisal_cd <= 0.0 && o.has_rune("reactive") {
            o.reprisal_cd = 2.0;
            due.push(Due {
                i,
                factor: 1.0,
                extra: true,
            });
        }
        if o.cd <= 0.0 {
            due.push(Due {
                i,
                factor: 1.0,
                extra: false,
            });
        }
    }
    let has_finisher = run.abilities.iter().any(|o| synergy::finisher(o.def.key));

    for Due { i, factor, extra } in due {
        let o = &run.abilities[i];
        let def = o.def;
        let schools = o.schools();
        let mut mult = o.damage_mult() * stats.might * run.power(schools) * factor;
        let mut flags = o.flags();
        let mut crit = false;
        let mut cc = stats.cc;
        // Rune of Overload: every 4th cast.
        let overload = !extra && o.has_rune("overload") && o.casts % 4 == 3;
        if overload {
            mult *= 3.0;
            crit = true;
        }
        // A running Recklessness, Cold Blood, Inner Focus...
        if run.act_link(actives::Link::AlwaysCrit) {
            crit = true;
        }
        // The class resource, spent.
        let mut finishing = false;
        match run.class.id {
            4 if run.b.res >= 5.0
                && synergy::offensive(def.kind)
                && (synergy::finisher(def.key) || !has_finisher) =>
            {
                finishing = true;
                mult *= 3.0;
                cc *= 2.0;
                flags |= synergy::F_FINISHER;
            }
            8 if run.b.last_school != 0
                && synergy::offensive(def.kind)
                && schools & run.b.last_school == 0 =>
            {
                mult *= if run.has_talent("combustion") {
                    1.45
                } else {
                    1.2
                };
            }
            1 if run.b.surge_t > 0.0 && run.has_talent("unbridled_wrath") => crit = true,
            _ => {}
        }
        let p = Params {
            dmg: def.damage * mult,
            dot: def.dot * mult * run.dot_power(),
            count: (o.count() + u32::from(run.has_relic("accuria") && synergy::counted(def.kind)))
                as usize,
            radius: o.radius(&stats),
            duration: o.duration(),
            cooldown: o.cooldown(&stats),
            schools,
            flags,
            src: i as u8,
            cc,
            crit,
        };
        let speed = speed_of(def.spell);
        let fired = match def.kind {
            Kind::Bolt | Kind::Strike | Kind::Melee => {
                let reach = if def.kind == Kind::Melee {
                    def.range + 0.5
                } else {
                    def.range
                };
                let targets = if matches!(
                    def.key,
                    "pyroblast" | "aimed_shot" | "exorcism" | "starfire"
                ) {
                    toughest(&foes, reach, p.count)
                } else {
                    nearest(&foes, reach, p.count)
                };
                if targets.is_empty() {
                    false
                } else {
                    if def.kind == Kind::Melee {
                        fx.swing(me, targets[0].e, p.dmg);
                        for f in &targets {
                            fx.impact(f.e, def.spell);
                        }
                    } else {
                        fx.cast(me, def.spell, &entities(&targets), None);
                    }
                    for f in &targets {
                        let (dmg, crit) = roll_cast(&mut rng, &stats, &p, p.dmg);
                        let delay = if speed > 0.0 { f.dist / speed } else { 0.0 };
                        schedule(
                            &mut pending,
                            &mut hits,
                            now,
                            delay,
                            hit_for(def, &p, f.e, dmg, crit),
                        );
                    }
                    true
                }
            }
            Kind::Nova | Kind::Cone => {
                let mut targets = around(&foes, hpos, p.radius);
                if def.kind == Kind::Cone {
                    if let Some(first) = targets.first().copied() {
                        let aim = flat(first.pos - hpos).normalize_or_zero();
                        targets.retain(|f| {
                            let d = flat(f.pos - hpos);
                            d.length() < 2.0 || d.normalize_or_zero().dot(aim) >= 0.8
                        });
                    }
                }
                if targets.is_empty() {
                    false
                } else {
                    fx.cast(me, def.spell, &entities(&targets), None);
                    for f in &targets {
                        let (dmg, crit) = roll_cast(&mut rng, &stats, &p, p.dmg);
                        let delay = if speed > 0.0 { f.dist / speed } else { 0.0 };
                        schedule(
                            &mut pending,
                            &mut hits,
                            now,
                            delay,
                            hit_for(def, &p, f.e, dmg, crit),
                        );
                    }
                    true
                }
            }
            Kind::Ground | Kind::GroundSelf => {
                let tether = o.has_rune("tether");
                let center = if def.kind == Kind::GroundSelf || tether {
                    foes.first()
                        .filter(|f| f.dist <= p.radius + 4.0)
                        .map(|_| hpos)
                } else {
                    densest(&foes, def.range, p.radius, None)
                };
                match center {
                    None => false,
                    Some(center) => {
                        let mut centers = vec![center];
                        if o.has_rune("twin") {
                            if let Some(c2) = densest(&foes, def.range, p.radius, Some(center)) {
                                centers.push(c2);
                            }
                        }
                        for c in centers {
                            fx.cast(me, def.spell, &[], Some(c));
                            let area = if def.overhead {
                                commands
                                    .spawn((Transform::from_translation(c), RunEntity))
                                    .id()
                            } else {
                                let serial = run.guid();
                                units::spawn_area_object(
                                    &mut commands,
                                    serial,
                                    def.spell,
                                    p.radius,
                                    c,
                                )
                            };
                            let burst = def.duration <= 0.0;
                            commands.entity(area).insert(GroundArea {
                                damage: p.dmg,
                                dot: p.dot,
                                dot_dur: if burst { 5.0 } else { 0.0 },
                                radius: p.radius,
                                left: if burst { 1.5 } else { p.duration },
                                every: if burst { f32::INFINITY } else { 1.0 },
                                next_tick: if burst { 0.0 } else { 0.4 },
                                effect: scaled(def.effect, p.cc),
                                mark: def.mark,
                                impact: if def.overhead { def.spell } else { 0 },
                                schools: p.schools,
                                flags: p.flags,
                                src: p.src,
                                follow: tether,
                            });
                        }
                        true
                    }
                }
            }
            Kind::Chain => match nearest(&foes, def.range, 1).first().copied() {
                None => false,
                Some(first) => {
                    let forked = o.has_rune("forked");
                    let jumps = p.count + 1 + if forked { 3 } else { 0 };
                    let mut chain = vec![first];
                    while chain.len() < jumps {
                        let last = chain[chain.len() - 1].pos;
                        let next = foes
                            .iter()
                            .filter(|f| !chain.iter().any(|c| c.e == f.e))
                            .filter(|f| flat(f.pos - last).length() <= p.radius)
                            .min_by(|a, b| {
                                flat(a.pos - last)
                                    .length()
                                    .total_cmp(&flat(b.pos - last).length())
                            });
                        match next {
                            Some(f) => chain.push(*f),
                            None => break,
                        }
                    }
                    fx.cast(me, def.spell, &entities(&chain), None);
                    let falloff: f32 = if forked { 1.0 } else { 0.85 };
                    for (k, f) in chain.iter().enumerate() {
                        let (dmg, crit) =
                            roll_cast(&mut rng, &stats, &p, p.dmg * falloff.powi(k as i32));
                        schedule(
                            &mut pending,
                            &mut hits,
                            now,
                            0.08 * k as f32,
                            hit_for(def, &p, f.e, dmg, crit),
                        );
                    }
                    true
                }
            },
            Kind::Dot => {
                let mut pool: Vec<Foe> = foes
                    .iter()
                    .filter(|f| f.dist <= def.range)
                    .take(p.count * 3)
                    .copied()
                    .collect();
                if pool.is_empty() {
                    false
                } else {
                    let mut targets = Vec::with_capacity(p.count);
                    while targets.len() < p.count && !pool.is_empty() {
                        targets.push(pool.swap_remove(rng.index(pool.len())));
                    }
                    fx.cast(me, def.spell, &entities(&targets), None);
                    for f in &targets {
                        let delay = if speed > 0.0 { f.dist / speed } else { 0.0 };
                        schedule(
                            &mut pending,
                            &mut hits,
                            now,
                            delay,
                            hit_for(def, &p, f.e, 0.0, false),
                        );
                    }
                    true
                }
            }
            Kind::Aura => {
                let targets = around(&foes, hpos, p.radius);
                if targets.is_empty() {
                    false
                } else {
                    for f in targets.iter().take(6) {
                        fx.impact(f.e, def.spell);
                    }
                    for f in &targets {
                        let (dmg, crit) = roll_cast(&mut rng, &stats, &p, p.dmg);
                        hits.write(hit_for(def, &p, f.e, dmg, crit));
                    }
                    true
                }
            }
            Kind::Orbit => {
                let targets = nearest(&foes, p.radius, p.count);
                if targets.is_empty() {
                    false
                } else {
                    for f in &targets {
                        fx.impact(f.e, 403);
                        let (dmg, crit) = roll_cast(&mut rng, &stats, &p, p.dmg);
                        hits.write(hit_for(def, &p, f.e, dmg, crit));
                    }
                    true
                }
            }
            Kind::Shield => {
                run.shield = p.dmg;
                run.shield_t = p.duration;
                fx.cast(me, def.spell, &[me], None);
                true
            }
            Kind::Immune => {
                if foes.first().is_some_and(|f| f.dist <= 5.0) {
                    run.immune_t = p.duration;
                    fx.cast(me, def.spell, &[me], None);
                    true
                } else {
                    false
                }
            }
            Kind::Thorns => {
                if foes.first().is_some_and(|f| f.dist <= 8.0) {
                    run.thorns = p.dmg;
                    run.thorns_t = p.duration;
                    run.thorns_spell = def.spell;
                    fx.cast(me, def.spell, &[me], None);
                    true
                } else {
                    false
                }
            }
            Kind::Heal => {
                if run.hp < stats.max_hp * 0.85 {
                    run.heal_rate = p.dmg / p.duration.max(1.0);
                    run.heal_t = p.duration;
                    fx.cast(me, def.spell, &[me], None);
                    fx.number(me, p.dmg, false, COLOR_HEAL);
                    true
                } else {
                    false
                }
            }
            Kind::Totem => {
                if foes
                    .first()
                    .is_some_and(|f| f.dist <= def.range.max(p.radius) + 6.0)
                {
                    fx.cast(me, def.spell, &[], None);
                    let totemic = o.has_rune("totemic");
                    let n = if o.has_rune("twin") { 2 } else { 1 };
                    for _ in 0..n {
                        let a = rng.range(0.0, std::f32::consts::TAU);
                        let pos = hpos + Vec3::new(a.cos(), 0.0, a.sin()) * 2.5;
                        let serial = run.guid();
                        let totem = units::spawn_unit(
                            &mut commands,
                            &UnitSpec {
                                serial,
                                display: def.totem.0,
                                scale: 1.0,
                                level: 60,
                                faction: units::FACTION_FRIENDLY,
                                bytes0: 0,
                                weapons: &[],
                                sheath: 0,
                            },
                            pos,
                            0.0,
                        );
                        let pulse = matches!(def.key, "magma_totem" | "fire_nova_totem");
                        let every = if pulse { 2.0 } else { 1.0 } * if totemic { 0.5 } else { 1.0 };
                        commands.entity(totem).insert(Totem {
                            cast: def.totem.1,
                            damage: p.dmg,
                            range: def.range,
                            radius: p.radius,
                            left: p.duration * if totemic { 1.5 } else { 1.0 },
                            tick: 0.3,
                            pulse,
                            every,
                            schools: p.schools,
                        });
                    }
                    true
                } else {
                    false
                }
            }
        };
        if !fired {
            if !extra {
                run.abilities[i].cd = 0.2;
            }
            continue;
        }

        // Rune of Radiance: the self spells burst with holy light.
        if run.abilities[i].has_rune("radiant") {
            let value = if def.damage > 0.0 { p.dmg } else { 30.0 * mult };
            let targets = around(&foes, hpos, 8.0);
            fx.cast(me, 15237, &entities(&targets), None);
            for f in &targets {
                let (dmg, crit) = roll(&mut rng, &stats, value * 1.5);
                hits.write(Hit {
                    crit,
                    flags: synergy::F_PROC,
                    ..Hit::proc(f.e, dmg, School::Holy)
                });
            }
        }

        // The class resource, gained.
        let zeal_cap = if run.has_talent("vengeance") {
            12.0
        } else {
            8.0
        };
        let rage = if run.has_talent("unbridled_wrath") {
            6.0
        } else {
            3.0
        };
        let b = &mut run.b;
        b.casts += 1;
        match run.class.id {
            4 if synergy::offensive(def.kind) => {
                b.res = if finishing {
                    0.0
                } else {
                    (b.res + 1.0).min(5.0)
                };
            }
            8 if synergy::offensive(def.kind) => b.last_school = schools,
            1 if has(schools, School::Physical) && b.surge_t <= 0.0 => {
                b.res = (b.res + rage).min(100.0);
                b.res_t = 0.0;
            }
            2 if has(schools, School::Holy) => {
                b.res = (b.res + 1.0).min(zeal_cap);
                b.res_t = 6.0;
            }
            11 => {
                if has(schools, School::Nature) && b.lunar_t <= 0.0 {
                    b.nature_n += 1;
                }
                if has(schools, School::Arcane) && b.solar_t <= 0.0 {
                    b.arcane_n += 1;
                }
            }
            _ => {}
        }
        if has(schools, School::Arcane) {
            run.b.arcane = (run.b.arcane + 1).min(run.arcane_cap());
            run.b.arcane_t = 8.0;
        }
        if finishing && run.has_talent("relentless") {
            for (k, other) in run.abilities.iter_mut().enumerate() {
                if k != i {
                    other.cd *= 0.7;
                }
            }
        }

        let echo_all = run.act_link(actives::Link::Echo);
        let o = &mut run.abilities[i];
        if !extra {
            o.casts += 1;
            o.cd = p.cooldown;
            // Clearcasting: ready again at once.
            if run.talents.iter().any(|t| t.key == "clearcasting") && rng.f32() < 0.10 {
                o.cd = 0.2;
            }
            // Echoes, overloads and the Hand of Justice: a second cast, shortly.
            let overload_every = if run.talents.iter().any(|t| t.key == "elemental_mastery") {
                3
            } else {
                5
            };
            if echo_all {
                o.echo = Some((0.25, 0.8));
            } else if o.has_rune("echo") {
                o.echo = Some((0.8, 0.6));
            } else if run.class.id == 7 && run.b.casts % overload_every == 0 {
                o.echo = Some((0.25, 0.8));
            } else if run.relics.iter().any(|r| r.key == "hoj") && rng.f32() < 0.12 {
                o.echo = Some((0.15, 1.0));
            }
        }
    }
}

/// Ground areas tick, and totems fight, until they run out.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(super) fn tick_areas(
    mut commands: Commands,
    time: Res<Time>,
    run: Res<Run>,
    mut rng: ResMut<Rng>,
    spells: Option<Res<crate::ui_action::Spells>>,
    hero: Query<&Transform, (With<Hero>, Without<GroundArea>, Without<Totem>)>,
    mut areas: Query<(Entity, &mut Transform, &mut GroundArea), Without<Hero>>,
    mut totems: Query<(Entity, &Transform, &mut Totem), (Without<GroundArea>, Without<Hero>)>,
    enemies: Query<
        (Entity, &Transform, &Enemy),
        (Without<GroundArea>, Without<Totem>, Without<Hero>),
    >,
    mut fx: Fx,
    mut pending: ResMut<PendingHits>,
    mut hits: MessageWriter<Hit>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let stats = run.stats();
    let now = run.elapsed;
    let hpos = hero.single().ok().map(|t| t.translation);
    let live: Vec<Foe> = enemies
        .iter()
        .filter(|(_, _, e)| !e.dead)
        .map(|(e, tf, en)| Foe {
            e,
            pos: tf.translation,
            hp: en.hp,
            dist: 0.0,
            radius: en.radius,
        })
        .collect();

    for (e, mut tf, mut area) in &mut areas {
        if area.follow {
            if let Some(h) = hpos {
                tf.translation = h;
            }
        }
        area.left -= dt;
        area.next_tick -= dt;
        if area.next_tick <= 0.0 {
            area.next_tick += area.every;
            for (i, f) in around(&live, tf.translation, area.radius)
                .into_iter()
                .enumerate()
            {
                if area.impact != 0 && i < 12 {
                    fx.impact(f.e, area.impact);
                }
                let (dmg, crit) = roll(&mut rng, &stats, area.damage);
                hits.write(Hit {
                    target: f.e,
                    damage: dmg,
                    dot: area.dot,
                    dot_dur: area.dot_dur,
                    effect: area.effect,
                    mark: area.mark,
                    mark_dur: 1.5,
                    splash: 0.0,
                    crit,
                    color: synergy::primary(area.schools).color(),
                    schools: area.schools,
                    flags: area.flags,
                    src: area.src,
                    burn: 0.0,
                });
            }
        }
        if area.left <= 0.0 {
            commands.entity(e).despawn();
        }
    }

    for (e, tf, mut totem) in &mut totems {
        totem.left -= dt;
        if totem.left <= 0.0 {
            commands
                .entity(e)
                .remove::<Totem>()
                .insert(benilla_world::model_fade::DespawnFade::default());
            continue;
        }
        totem.tick -= dt;
        if totem.tick > 0.0 {
            continue;
        }
        let pos = tf.translation;
        let school = synergy::primary(totem.schools);
        if totem.pulse {
            let targets = around(&live, pos, totem.radius);
            totem.tick = totem.every;
            if targets.is_empty() {
                continue;
            }
            fx.cast(e, totem.cast, &[], None);
            for f in &targets {
                let (dmg, crit) = roll(&mut rng, &stats, totem.damage);
                hits.write(Hit {
                    crit,
                    flags: synergy::F_PROC,
                    schools: totem.schools,
                    ..Hit::proc(f.e, dmg, school)
                });
            }
        } else {
            totem.tick = totem.every;
            let target = live
                .iter()
                .map(|f| (flat(f.pos - pos).length(), f))
                .filter(|(d, _)| *d <= totem.range)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((dist, f)) = target else { continue };
            fx.cast(e, totem.cast, &[f.e], None);
            let speed = spells
                .as_deref()
                .and_then(|s| s.catalog.get(totem.cast))
                .map_or(0.0, |d| d.speed);
            let delay = if speed > 0.0 { dist / speed } else { 0.0 };
            let (dmg, crit) = roll(&mut rng, &stats, totem.damage);
            schedule(
                &mut pending,
                &mut hits,
                now,
                delay,
                Hit {
                    crit,
                    flags: synergy::F_PROC,
                    schools: totem.schools,
                    ..Hit::proc(f.e, dmg, school)
                },
            );
        }
    }
}

/// The talents and relics a landing hit consults, read once per frame.
pub(super) struct Keys {
    pub shatter: bool,
    pub ignite: bool,
    pub weaving: bool,
    pub conduction: bool,
    pub zeal: bool,
    pub deep_wounds: bool,
    pub frostfire: bool,
    pub shadowflame: bool,
    pub frozen_circuit: bool,
    pub twilight: bool,
    pub arcane_storm: bool,
    pub searing_light: bool,
    pub frostblade: bool,
    pub spellblade: bool,
    pub rending_storm: bool,
    pub plague: bool,
    pub vampiric: bool,
    pub trueshot: bool,
    pub thunderfury: bool,
    pub ashbringer: bool,
    pub weave_cap: usize,
    pub sunder_cap: u8,
    pub atonement: f32,
    /// A running burst or guard's lifesteal.
    pub act_leech: f32,
}

impl Keys {
    pub(super) fn of(run: &Run) -> Self {
        let t = |k: &str| run.has_talent(k);
        let r = |k: &str| run.has_relic(k);
        let priest = run.class.id == 5;
        Self {
            shatter: t("shatter"),
            ignite: t("ignite"),
            weaving: t("weaving"),
            conduction: t("conduction"),
            zeal: t("zeal"),
            deep_wounds: t("deep_wounds"),
            frostfire: t("frostfire"),
            shadowflame: t("shadowflame"),
            frozen_circuit: t("frozen_circuit"),
            twilight: t("twilight"),
            arcane_storm: t("arcane_storm"),
            searing_light: t("searing_light"),
            frostblade: t("frostblade"),
            spellblade: t("spellblade"),
            rending_storm: t("rending_storm"),
            plague: t("plague"),
            vampiric: t("vampiric"),
            trueshot: t("trueshot"),
            thunderfury: r("thunderfury"),
            ashbringer: r("ashbringer"),
            weave_cap: if t("weaving") { 6 } else { 5 }
                + usize::from(priest)
                + usize::from(t("spirit_tap")),
            sunder_cap: 5
                + if t("deep_wounds") { 3 } else { 0 }
                + if r("dalrend_r") && r("dalrend_l") {
                    3
                } else {
                    0
                },
            atonement: if priest {
                if t("spirit_tap") {
                    0.10
                } else {
                    0.05
                }
            } else {
                0.0
            },
            act_leech: run.act_buff().leech,
        }
    }
}

/// An enemy that died this frame, and what it carried into death.
struct Death {
    pos: Vec3,
    dots: Vec<enemies::Dot>,
    st: Status,
    /// The killing blow's ability, flags, schools and damage.
    src: u8,
    flags: u16,
    schools: u8,
    damage: f32,
    boss: bool,
}

/// The `n` live enemies nearest `pos` within `r`, but not `not`.
fn neighbours(live: &[(Entity, Vec3)], pos: Vec3, r: f32, n: usize, not: Entity) -> Vec<Entity> {
    let mut v: Vec<(f32, Entity)> = live
        .iter()
        .filter(|(e, _)| *e != not)
        .map(|(e, p)| (flat(*p - pos).length(), *e))
        .filter(|(d, _)| *d <= r)
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().take(n).map(|(_, e)| e).collect()
}

/// Land every hit due: damage and its number, the statuses its schools leave and the procs they
/// set off, the over-time part, crowd control, the marks, and death with its loot and its legacy.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn apply_hits(
    mut commands: Commands,
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut rng: ResMut<Rng>,
    mut pending: ResMut<PendingHits>,
    mut incoming: MessageReader<Hit>,
    mut enemies: Query<(
        Entity,
        &Guid,
        &Transform,
        &mut Enemy,
        &mut ObjectStore,
        &mut Marks,
    )>,
    mut fx: Fx,
    mut loot: MessageWriter<Loot>,
    mut fields: MessageWriter<FieldChanged>,
) {
    let dt = time.delta_secs();
    let now = run.elapsed;
    let mut due: Vec<Hit> = incoming.read().cloned().collect();
    pending.0.retain(|(t, h)| {
        if *t <= now {
            due.push(h.clone());
            false
        } else {
            true
        }
    });
    let live: Vec<(Entity, Vec3)> = enemies
        .iter()
        .filter(|(.., en, _, _)| !en.dead)
        .map(|(e, _, tf, ..)| (e, tf.translation))
        .collect();
    let stats = run.stats();
    let k = Keys::of(&run);
    // New hits the procs set off, landing a beat later so the cascade reads.
    let mut later: Vec<(f32, Hit)> = Vec::new();
    let mut deaths: Vec<Death> = Vec::new();
    let mut thunder_left = 3u32;

    // Splash reaches the target's neighbours at seventy percent.
    let mut all = Vec::with_capacity(due.len());
    // Flurry: a critical strike quickens the next casts.
    if run.has_talent("flurry") && due.iter().any(|h| h.crit) {
        run.b.flurry_t = 3.0;
    }
    for h in due {
        if h.splash > 0.0 {
            if let Ok((_, _, center, ..)) = enemies.get(h.target) {
                let c = center.translation;
                for &(e, p) in &live {
                    if e != h.target && flat(p - c).length() <= h.splash {
                        all.push(Hit {
                            target: e,
                            damage: h.damage * 0.7,
                            splash: 0.0,
                            mark: 0,
                            flags: h.flags & !(synergy::F_PROC | synergy::F_SPLIT),
                            ..h.clone()
                        });
                    }
                }
            }
        }
        all.push(Hit { splash: 0.0, ..h });
    }

    for h in all {
        let Ok((e, guid, tf, mut en, mut store, mut marks)) = enemies.get_mut(h.target) else {
            continue;
        };
        if en.dead {
            continue;
        }
        let en = &mut *en;
        let pos = tf.translation;
        let s = h.schools;
        let frozen = en.st.frozen_t > 0.0;

        // What the target's statuses make of the blow.
        let mut m = 1.0;
        if frozen {
            m += if k.shatter { 1.0 } else { 0.4 };
        }
        m += 0.03 * f32::from(en.st.sunder);
        if has(s, School::Shadow) {
            let n = en
                .st
                .afflictions(en.dots.len(), k.shadowflame, k.twilight)
                .min(k.weave_cap);
            m += n as f32 * if k.weaving { 0.15 } else { 0.08 };
        }
        if has(s, School::Physical) && en.st.judged_t > 0.0 {
            m += if k.zeal { 0.3 } else { 0.15 };
        }
        if en.st.hunted {
            m += if k.trueshot { 0.6 } else { 0.35 };
        }
        if k.frostfire && has(s, School::Fire) && (en.st.chill > 0 || frozen) {
            m += 0.5;
        }
        if k.searing_light && has(s, School::Holy) && en.st.burn_t > 0.0 {
            m += 0.3;
        }
        if h.flags & synergy::F_EXECUTIONER != 0 && en.hp / en.max_hp < 0.3 {
            m *= 2.0;
        }
        let mut dmg = h.damage * m;
        if let Effect::Execute(threshold) = h.effect {
            if !en.boss && en.hp / en.max_hp <= threshold {
                dmg = dmg.max(en.hp);
            }
        }
        if dmg > 0.0 {
            en.hp -= dmg;
            run.damage_done += f64::from(dmg);
            fx.number(e, dmg, h.crit, h.color);
            if let Effect::Drain(frac) = h.effect {
                let max = stats.max_hp;
                run.hp = (run.hp + dmg * frac).min(max);
            }
            let mut leech = 0.0;
            if k.vampiric {
                leech += dmg * 0.02;
            }
            if has(s, School::Holy) {
                leech += dmg * k.atonement;
            }
            if h.flags & synergy::F_BLOODTHIRST != 0 {
                leech += stats.max_hp * 0.01;
            }
            leech += dmg * k.act_leech;
            if leech > 0.0 {
                run.leech(leech);
            }
        }
        if h.dot > 0.0 && h.dot_dur > 0.0 {
            if en.dots.len() >= 6 {
                en.dots.remove(0);
            }
            en.dots.push(enemies::Dot {
                dps: h.dot,
                left: h.dot_dur,
                tick: 1.0,
                school: s,
                haunt: h.flags & synergy::F_HAUNT != 0,
            });
            if h.flags & synergy::F_CONTAGION != 0 {
                en.st.contagion = true;
            }
        }
        let boss = en.boss;
        match h.effect {
            Effect::Slow(m, t) => {
                let m = if boss { m.max(0.75) } else { m };
                en.slow = if en.slow_t > 0.0 { en.slow.min(m) } else { m };
                en.slow_t = en.slow_t.max(t);
            }
            Effect::Root(t) => en.root_t = en.root_t.max(if boss { t * 0.3 } else { t }),
            Effect::Stun(t) => en.stun_t = en.stun_t.max(if boss { t * 0.3 } else { t }),
            Effect::Fear(t) if !boss => en.fear_t = en.fear_t.max(t),
            Effect::Weaken(m, t) => {
                en.weaken = m;
                en.weaken_t = t;
            }
            _ => {}
        }
        if h.mark != 0 {
            marks.add(h.mark, h.mark_dur);
        }

        // --- The statuses this blow's schools leave. ---
        let landed = dmg > 0.0 || h.dot > 0.0 || h.burn > 0.0;
        let reps: u8 = if h.flags & synergy::F_DOUBLE_STATUS != 0 {
            2
        } else {
            1
        };
        let procs = h.flags & synergy::F_PROC != 0;
        if landed {
            // Fire on the frozen: Thermal Shock, before the frost below can refreeze it.
            if k.frostfire && frozen && has(s, School::Fire) && dmg > 0.0 {
                en.st.frozen_t = 0.0;
                fx.impact(e, 11113);
                for n in neighbours(&live, pos, 5.0, 12, e) {
                    later.push((0.05, Hit::proc(n, dmg * 0.6, School::Fire)));
                }
            }
            if k.frostblade && frozen && has(s, School::Physical) && dmg > 0.0 && procs {
                for n in neighbours(&live, pos, 5.0, 12, e) {
                    later.push((0.05, Hit::proc(n, dmg * 0.5, School::Frost)));
                }
            }
            // Frost: chill, then freeze.
            if has(s, School::Frost) || (k.frostblade && has(s, School::Physical)) {
                let at = if k.shatter { 4 } else { 5 };
                en.st.chill = en.st.chill.saturating_add(reps);
                en.st.chill_t = 4.0;
                if en.st.chill >= at {
                    if boss {
                        en.st.chill = at - 1;
                    } else {
                        en.st.chill = 0;
                        en.st.frozen_t = 1.5 * stats.cc;
                        marks.add(122, en.st.frozen_t);
                    }
                }
            }
            // Fire: the burn grows with each fire blow, to a ceiling set by the blow.
            if has(s, School::Fire) && dmg > 0.0 {
                let add = dmg * 0.08 * f32::from(reps) * if h.crit { 2.0 } else { 1.0 };
                en.st.burn = (en.st.burn + add).min(en.st.burn.max(add * 2.5));
                en.st.burn_t = 4.0;
            }
            if h.burn > 0.0 {
                en.st.burn = en.st.burn.max(h.burn);
                en.st.burn_t = 4.0;
            }
            if k.searing_light && en.st.judged_t > 0.0 && dmg > 0.0 {
                en.st.burn = en.st.burn.max(dmg * 0.05);
                en.st.burn_t = 4.0;
            }
            if k.shadowflame && has(s, School::Shadow) && en.st.burn > 0.0 {
                en.st.burn_t = 4.0;
            }
            if en.st.burn_t > 0.0 && en.st.burn_tick <= 0.0 {
                en.st.burn_tick = 1.0;
            }
            // Holy: judged.
            if has(s, School::Holy) {
                en.st.judged_t = 5.0;
            }
            // Physical: sunder; and the deep wounds of a crit.
            if has(s, School::Physical) && dmg > 0.0 {
                en.st.sunder = (en.st.sunder + reps).min(k.sunder_cap);
                en.st.sunder_t = 6.0;
                if k.deep_wounds && h.crit && en.dots.len() < 6 {
                    en.dots.push(enemies::Dot {
                        dps: dmg * 0.5 / 6.0,
                        left: 6.0,
                        tick: 1.0,
                        school: School::Physical.bit(),
                        haunt: false,
                    });
                }
                if k.spellblade && procs {
                    run.b.arcane = (run.b.arcane + 1).min(run.arcane_cap());
                    run.b.arcane_t = 8.0;
                }
            }
            // Nature: static, then the discharge.
            if has(s, School::Nature) {
                let extra = u8::from(k.rending_storm && en.st.sunder > 0);
                en.st.stat = en.st.stat.saturating_add(reps + extra);
                en.st.stat_t = 5.0;
                let at = if k.conduction { 2 } else { 3 };
                if procs && dmg > 0.0 && (en.st.stat >= at || (k.frozen_circuit && frozen)) {
                    en.st.stat = 0;
                    let charges = if k.arcane_storm { run.b.arcane } else { 0 };
                    let n = if k.conduction { 3 } else { 2 } + usize::from(charges);
                    let pct =
                        if k.conduction { 0.5 } else { 0.35 } * (1.0 + 0.1 * f32::from(charges));
                    let targets = neighbours(&live, pos, 8.0, n, e);
                    if !targets.is_empty() {
                        fx.cast(e, 421, &targets, None);
                    }
                    for (j, t) in targets.into_iter().enumerate() {
                        let mut hit = Hit::proc(t, dmg * pct, School::Nature);
                        if k.frozen_circuit {
                            hit.schools |= School::Frost.bit();
                            hit.flags |= synergy::F_DOUBLE_STATUS;
                        }
                        if k.rending_storm {
                            hit.schools |= School::Physical.bit();
                            hit.flags |= synergy::F_DOUBLE_STATUS;
                        }
                        later.push((0.06 + 0.04 * j as f32, hit));
                    }
                }
            }
            // Thunderfury.
            if k.thunderfury && procs && dmg > 0.0 && thunder_left > 0 && rng.f32() < 0.15 {
                thunder_left -= 1;
                let targets = neighbours(&live, pos, 10.0, 5, e);
                if !targets.is_empty() {
                    fx.cast(e, 421, &targets, None);
                }
                let bolt = (dmg * 0.6).max(20.0 * stats.might);
                for t in targets {
                    later.push((
                        0.08,
                        Hit {
                            effect: Effect::Slow(0.7, 2.0),
                            ..Hit::proc(t, bolt, School::Nature)
                        },
                    ));
                }
            }
        }

        if en.hp <= 0.0 {
            deaths.push(Death {
                pos,
                dots: en.dots.clone(),
                st: en.st.clone(),
                src: h.src,
                flags: h.flags,
                schools: s,
                damage: h.damage,
                boss,
            });
            kill(&mut commands, &mut run, &mut loot, e, pos, en);
            units::kill_unit(&mut store);
            marks.clear();
        }
        marks.flush(e, guid.0, &mut store, &mut fields);
    }

    // The over-time ticks, once a second each: the damage over time, and the burn.
    let ignite = if k.ignite { 2.0 } else { 1.0 };
    for (e, guid, tf, mut en, mut store, mut marks) in &mut enemies {
        if en.dead {
            continue;
        }
        let en = &mut *en;
        let mut dealt = 0.0;
        let mut haunted = 0.0;
        for d in &mut en.dots {
            d.left -= dt;
            d.tick -= dt;
            if d.tick <= 0.0 {
                d.tick += 1.0;
                dealt += d.dps;
                if d.haunt {
                    haunted += d.dps * 0.2;
                }
            }
        }
        en.dots.retain(|d| d.left > 0.0);
        let mut burned = 0.0;
        if en.st.burn_t > 0.0 {
            en.st.burn_t -= dt;
            en.st.burn_tick -= dt;
            if en.st.burn_tick <= 0.0 {
                en.st.burn_tick += 1.0;
                burned = en.st.burn * ignite;
            }
            if en.st.burn_t <= 0.0 {
                en.st.burn = 0.0;
            }
        }
        if haunted > 0.0 {
            run.leech(haunted);
        }
        if burned > 0.0 {
            fx.number(e, burned, false, School::Fire.color());
        }
        if dealt > 0.0 {
            fx.number(e, dealt, false, COLOR_SPELL);
        }
        let total = dealt + burned;
        if total > 0.0 {
            en.hp -= total;
            run.damage_done += f64::from(total);
            if en.hp <= 0.0 {
                let pos = tf.translation;
                deaths.push(Death {
                    pos,
                    dots: en.dots.clone(),
                    st: en.st.clone(),
                    src: NO_SRC,
                    flags: 0,
                    schools: 0,
                    damage: 0.0,
                    boss: en.boss,
                });
                kill(&mut commands, &mut run, &mut loot, e, pos, en);
                units::kill_unit(&mut store);
                marks.clear();
                marks.flush(e, guid.0, &mut store, &mut fields);
            }
        }
    }

    // What the dead leave behind.
    for d in deaths {
        let afflicted = !d.dots.is_empty() || d.st.burn_t > 0.0;
        let me = Entity::PLACEHOLDER;
        // Rampage: the spell that killed grows.
        if let Some(o) = run.abilities.get_mut(usize::from(d.src)) {
            if o.has_rune("rampage") {
                o.rampage = (o.rampage + 0.01).min(0.8);
            }
            // Rune of Splitting.
            if d.flags & synergy::F_SPLIT != 0 && d.damage > 0.0 {
                let spell = o.def.spell;
                let targets = neighbours(&live, d.pos, 10.0, 2, me);
                for t in targets {
                    fx.impact(t, spell);
                    later.push((
                        0.1,
                        Hit {
                            color: synergy::primary(d.schools).color(),
                            schools: d.schools,
                            ..Hit::direct(t, d.damage * 0.5, false)
                        },
                    ));
                }
            }
        }
        // Spreading afflictions: contagion to 2, the plague to 1.
        let spread = if d.st.contagion {
            2
        } else {
            usize::from(k.plague)
        };
        if spread > 0 && !d.dots.is_empty() {
            for t in neighbours(&live, d.pos, 8.0, spread, me) {
                for dot in &d.dots {
                    later.push((
                        0.15,
                        Hit {
                            dot: dot.dps,
                            dot_dur: dot.left.max(3.0),
                            schools: dot.school,
                            flags: if d.st.contagion {
                                synergy::F_CONTAGION
                            } else {
                                0
                            },
                            ..Hit::direct(t, 0.0, false)
                        },
                    ));
                }
                fx.impact(t, 172);
            }
        }
        // Master of Flame: the burn leaps.
        if k.ignite && d.st.burn > 0.0 {
            for t in neighbours(&live, d.pos, 8.0, 2, me) {
                later.push((
                    0.12,
                    Hit {
                        burn: d.st.burn,
                        schools: School::Fire.bit(),
                        ..Hit::direct(t, 0.0, false)
                    },
                ));
            }
        }
        // Twilight: an afflicted death judges all around.
        if k.twilight && afflicted {
            for t in neighbours(&live, d.pos, 8.0, 12, me) {
                later.push((0.1, Hit::proc(t, 1.0, School::Holy)));
            }
        }
        // Holy: the judged heal in death.
        if d.st.judged_t > 0.0 {
            run.kill_heal(if k.zeal { 3.0 } else { 1.0 });
            if k.zeal {
                run.b.zeal = (run.b.zeal + 1).min(5);
                run.b.zeal_t = 6.0;
            }
        }
        if run.race == 5 {
            run.kill_heal(0.5);
        }
        if k.ashbringer {
            run.kill_heal(1.0);
        }
        // Hunter's Mark: the quarry's fall brings Rapid Fire.
        if d.st.hunted && run.class.id == 3 {
            run.b.surge_t = 5.0;
        }
        // Soul shards from the afflicted.
        if run.class.id == 9 && afflicted && run.b.shard_cd <= 0.0 && run.b.res < 5.0 {
            run.b.res += 1.0;
            run.b.shard_cd = if run.has_talent("soul_harvest") {
                0.75
            } else {
                1.5
            };
            if run.b.res >= 5.0 {
                run.b.soul_fire = true;
            }
        }
        if d.boss {
            run.b.bosses_slain += 1;
            run.rerolls += 1;
            run.banishes += 1;
            run.forgets += 1;
            if run.has_talent("second_wind") {
                run.hp = stats.max_hp;
                run.rerolls += 1;
                run.banishes += 1;
                run.forgets += 1;
            }
        }
    }

    for (delay, h) in later {
        pending.0.push((now + delay, h));
    }
}

fn kill(
    commands: &mut Commands,
    run: &mut Run,
    loot: &mut MessageWriter<Loot>,
    e: Entity,
    pos: Vec3,
    en: &mut Enemy,
) {
    en.dead = true;
    en.hp = 0.0;
    en.dots.clear();
    enemies::mark_dying(commands, e);
    loot.write(Loot {
        pos,
        xp: en.xp,
        boss: en.boss,
        elite: en.elite,
    });
    run.kills += 1;
    if en.boss {
        run.banner = Some((format!("{} is slain!", en.def.name), 3.0));
    } else if en.elite {
        run.banner = Some((format!("The elite {} is slain!", en.def.name), 2.5));
    }
}
