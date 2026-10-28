//! The build's clockwork: the timers on the class resource, the racial traits and the relics, and
//! what they set off on their own — Enrage, Eclipse, Soul Fire, Hunter's Mark, War Stomp,
//! Sulfuras's meteor.

use bevy::prelude::*;

use super::abilities::{densest, roll, toughest, Foe, Hit, PendingHits};
use super::enemies::Enemy;
use super::hero::{self, Hero};
use super::synergy::School;
use super::units::{flat, Fx, Marks};
use super::{Rng, Run};

/// The base of the build's own strikes, grown with the hero's level so they keep pace.
fn strike_base(run: &Run, base: f32, per_level: f32) -> f32 {
    (base + per_level * run.level as f32) * run.stats().might
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn tick_build(
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut rng: ResMut<Rng>,
    hero: Query<(Entity, &Transform), With<Hero>>,
    mut enemies: Query<(Entity, &Transform, &mut Enemy, &mut Marks), Without<Hero>>,
    mut fx: Fx,
    mut pending: ResMut<PendingHits>,
    mut mark_t: Local<f32>,
) {
    let dt = time.delta_secs();
    if run.dead_t.is_some() || dt <= 0.0 {
        return;
    }
    let Ok((me, htf)) = hero.single() else { return };
    let hpos = htf.translation;
    let stats = run.stats();
    let now = run.elapsed;
    let run = &mut *run;
    let class = run.class.id;
    let b = &mut run.b;

    // Every timer runs down.
    for t in [
        &mut b.arcane_t,
        &mut b.surge_t,
        &mut b.lunar_t,
        &mut b.solar_t,
        &mut b.shard_cd,
        &mut b.zeal_t,
        &mut b.ephemeral_t,
        &mut b.charm_t,
        &mut b.fury_cd,
        &mut b.fury_t,
        &mut b.stone_cd,
        &mut b.stone_t,
        &mut b.stomp_cd,
        &mut b.lifegiving_cd,
        &mut b.quel_cd,
        &mut b.meteor_cd,
        &mut b.flurry_t,
    ] {
        *t = (*t - dt).max(0.0);
    }
    if b.arcane_t <= 0.0 {
        b.arcane = 0;
    }
    if b.zeal_t <= 0.0 {
        b.zeal = 0;
    }

    let mut banner: Option<&'static str> = None;
    match class {
        // Rage: fades out of the fight, and at the top breaks into Enrage.
        1 => {
            b.res_t += dt;
            if b.res_t > 4.0 {
                b.res = (b.res - 3.0 * dt).max(0.0);
            }
            if b.res >= 100.0 {
                b.res = 0.0;
                b.surge_t = 8.0;
                banner = Some("Enrage!");
            }
        }
        // Zeal fades when the light stops.
        2 => {
            b.res_t -= dt;
            if b.res_t <= 0.0 {
                b.res = 0.0;
            }
        }
        11 => {
            let len = if run.talents.iter().any(|t| t.key == "moonkin") {
                20.0
            } else {
                10.0
            };
            if b.nature_n >= 5 {
                b.nature_n = 0;
                b.lunar_t = len;
                banner = Some("Lunar Eclipse!");
            }
            if b.arcane_n >= 5 {
                b.arcane_n = 0;
                b.solar_t = len;
                banner = Some("Solar Eclipse!");
            }
        }
        _ => {}
    }
    if run.has_relic("ephemeral") && run.b.ephemeral_cd <= 0.0 {
        run.b.ephemeral_cd = 20.0;
        run.b.ephemeral_t = 6.0;
    } else {
        run.b.ephemeral_cd = (run.b.ephemeral_cd - dt).max(0.0);
    }

    let mut foes: Vec<Foe> = enemies
        .iter()
        .filter(|(_, _, e, _)| !e.dead)
        .map(|(e, tf, en, _)| Foe {
            e,
            pos: tf.translation,
            hp: en.hp,
            dist: hero::reach(hpos, tf.translation),
            radius: en.radius,
        })
        .collect();
    foes.sort_by(|a, b| a.dist.total_cmp(&b.dist));
    let fighting = foes.first().is_some_and(|f| f.dist <= 15.0);

    // Orc: Blood Fury, whenever it is ready and there is a fight.
    if run.race == 2 && fighting && run.b.fury_cd <= 0.0 {
        run.b.fury_cd = 45.0;
        run.b.fury_t = 12.0;
        fx.cast(me, 20572, &[me], None);
        banner = Some("Blood Fury!");
    }

    // Tauren: War Stomp when surrounded.
    if run.race == 6 && run.b.stomp_cd <= 0.0 && foes.iter().filter(|f| f.dist <= 6.0).count() >= 5
    {
        run.b.stomp_cd = 20.0;
        fx.cast(me, 20549, &[], None);
        for (_, tf, mut en, _) in &mut enemies {
            if !en.dead && flat(tf.translation - hpos).length() <= 7.0 {
                let t = 2.0 * stats.cc * if en.boss { 0.3 } else { 1.0 };
                en.stun_t = en.stun_t.max(t);
            }
        }
    }

    // Warlock: Soul Fire at the toughest, when the shards are full.
    if run.b.soul_fire {
        if let Some(f) = toughest(&foes, 30.0, 1).first() {
            run.b.soul_fire = false;
            run.b.res = 0.0;
            let mut dmg = strike_base(run, 60.0, 10.0) * run.power(School::Fire.bit());
            if run.has_talent("soul_harvest") {
                dmg *= 2.0;
            }
            let (dmg, crit) = roll(&mut rng, &stats, dmg);
            fx.cast(me, 6353, &[f.e], None);
            pending.0.push((
                now + (f.dist / 20.0).min(1.5),
                Hit {
                    crit,
                    flags: super::synergy::F_PROC,
                    ..Hit::proc(f.e, dmg, School::Fire)
                },
            ));
            banner = Some("Soul Fire!");
        }
    }

    // Sulfuras: a meteor at the densest pack.
    if run.has_relic("sulfuras") && run.b.meteor_cd <= 0.0 {
        if let Some(c) = densest(&foes, 25.0, 6.0, None) {
            run.b.meteor_cd = 8.0;
            let dmg = strike_base(run, 40.0, 4.0) * run.power(School::Fire.bit());
            fx.cast(me, 2120, &[], Some(c));
            for f in foes.iter().filter(|f| flat(f.pos - c).length() <= 6.0) {
                let (d, crit) = roll(&mut rng, &stats, dmg);
                pending.0.push((
                    now + 0.6,
                    Hit {
                        crit,
                        burn: dmg * 0.2,
                        flags: super::synergy::F_PROC,
                        ..Hit::proc(f.e, d, School::Fire)
                    },
                ));
            }
        }
    }

    // Hunter: the mark moves to the toughest in reach twice a second.
    if class == 3 {
        *mark_t -= dt;
        if *mark_t <= 0.0 {
            *mark_t = 0.5;
            let n = if run.act_link(super::actives::Link::MarkAll) {
                usize::MAX
            } else if run.has_talent("trueshot") {
                3
            } else {
                1
            };
            let marked: Vec<Entity> = toughest(&foes, 30.0, n).iter().map(|f| f.e).collect();
            for (e, _, mut en, mut marks) in &mut enemies {
                let on = marked.contains(&e) && !en.dead;
                if on {
                    marks.add(1130, 0.7);
                }
                if en.st.hunted != on {
                    en.st.hunted = on;
                }
            }
        }
    }

    if let Some(msg) = banner {
        if run.banner.is_none() {
            run.banner = Some((msg.into(), 2.0));
        }
    }
}
