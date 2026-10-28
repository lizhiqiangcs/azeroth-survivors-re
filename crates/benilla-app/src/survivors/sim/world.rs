//! The engine: one run on an open plain, with the game's own numbers and rules. The run state,
//! the stats, the damage formulas, the level-up offers and the hero's damage taken are the game's
//! own code ([`Run`], [`Owned`], `hud::pick`, `hero::hurt_hero`); what is redone here is only what
//! the game does through the ECS: the spawner, the chase, the casts, the hits and their statuses,
//! the gems. Those follow `enemies.rs`, `abilities.rs` and `procs.rs` closely, without the visuals.

use std::collections::HashMap;

use bevy::math::Vec2;

use super::super::abilities::{roll, Keys};
use super::super::actives::{self, Det, Link};
use super::super::mobility::Move;
use super::super::data::{self, AbilityDef, Effect, EnemyDef, Kind};
use super::super::enemies::Status;
use super::super::hero::{self, HERO_RADIUS};
use super::super::synergy::{self, has, School};
use super::super::{Owned, Rng, Run, Stats};
use super::policy::{Hands, Movement};
use super::Tune;

/// A damage source, for the report: a spell's key, or a proc's name.
pub(super) type Src = &'static str;

pub(super) struct Foe {
    pub def: &'static EnemyDef,
    pub pos: Vec2,
    pub hp: f32,
    pub max_hp: f32,
    speed: f32,
    damage: f32,
    pub radius: f32,
    xp: u32,
    pub boss: bool,
    pub elite: bool,
    attack_cd: f32,
    slow: f32,
    slow_t: f32,
    root_t: f32,
    stun_t: f32,
    fear_t: f32,
    weaken: f32,
    weaken_t: f32,
    dots: Vec<Dot>,
    st: Status,
    dead: bool,
    /// The source of the last damage, for the kill's credit.
    last_src: Src,
}

#[derive(Clone)]
struct Dot {
    dps: f32,
    left: f32,
    school: u8,
    haunt: bool,
    src: Src,
}

/// A hit about to land. Everything lands in the tick it is made: the few tenths of a second a
/// missile flies make no difference to the balance.
#[derive(Clone)]
struct SHit {
    target: usize,
    damage: f32,
    dot: f32,
    dot_dur: f32,
    effect: Effect,
    splash: f32,
    crit: bool,
    schools: u8,
    flags: u16,
    /// The owned ability's slot, for Rampage and Splitting; `u8::MAX` for none.
    slot: u8,
    burn: f32,
    src: Src,
}

impl SHit {
    fn proc(target: usize, damage: f32, school: School, src: Src) -> Self {
        Self {
            target,
            damage,
            dot: 0.0,
            dot_dur: 0.0,
            effect: Effect::None,
            splash: 0.0,
            crit: false,
            schools: school.bit(),
            flags: 0,
            slot: u8::MAX,
            burn: 0.0,
            src,
        }
    }
}

struct Area {
    center: Vec2,
    radius: f32,
    damage: f32,
    dot: f32,
    dot_dur: f32,
    left: f32,
    every: f32,
    next: f32,
    effect: Effect,
    schools: u8,
    flags: u16,
    slot: u8,
    follow: bool,
    src: Src,
}

struct TotemS {
    pos: Vec2,
    damage: f32,
    range: f32,
    radius: f32,
    left: f32,
    every: f32,
    next: f32,
    pulse: bool,
    schools: u8,
    src: Src,
}

enum GemKind {
    Xp(u32),
    Heal(f32),
    Chest(bool),
}

struct Gem {
    pos: Vec2,
    kind: GemKind,
}

/// What one run left behind, beyond the run state itself.
#[derive(Default)]
pub(super) struct Tally {
    pub damage_by: HashMap<Src, f64>,
    /// When each spell was learned, by key.
    pub learned_at: HashMap<Src, f32>,
    pub damage_taken: f64,
    pub healed: f64,
    pub lowest_hp: f32,
    pub elites_killed: u32,
    pub bosses_killed: u32,
    pub chests_missed: u32,
    /// F casts.
    pub moves: u32,
    /// The blows of the last five seconds: when, how much got through, from what (0 a common
    /// foe, 1 an elite, 2 a boss).
    pub recent: std::collections::VecDeque<(f32, f32, u8)>,
}

pub(super) struct Sim<'a> {
    pub run: Run,
    pub rng: Rng,
    tune: &'a Tune,
    movement: Movement,
    hands: Hands,
    map: usize,
    pub hero: Vec2,
    pub foes: Vec<Foe>,
    areas: Vec<Area>,
    totems: Vec<TotemS>,
    gems: Vec<Gem>,
    elites: u32,
    spawn_t: f32,
    mark_t: f32,
    /// A slow wander for the kiter, so it circles the field instead of running off.
    heading: f32,
    wander_t: f32,
    wander_dir: Vec2,
    queue: Vec<SHit>,
    /// The separation grid, kept between steps for its allocations.
    grid: Vec<Vec<u16>>,
    /// The live enemies by distance, until something moves.
    near: std::cell::RefCell<Option<Vec<(f32, usize)>>>,
    pub tally: Tally,
}

const HOME_LEASH: f32 = 40.0;
const MAX_GEMS: usize = 350;
const GRID: usize = 64;
const CELL: f32 = 1.6;

impl<'a> Sim<'a> {
    pub fn new(
        run: Run,
        rng: Rng,
        tune: &'a Tune,
        movement: Movement,
        hands: Hands,
        map: usize,
    ) -> Self {
        let lowest_hp = run.hp;
        let mut sim = Self {
            run,
            rng,
            tune,
            movement,
            hands,
            map,
            hero: Vec2::ZERO,
            foes: Vec::with_capacity(320),
            areas: Vec::new(),
            totems: Vec::new(),
            gems: Vec::new(),
            elites: 0,
            spawn_t: 0.0,
            mark_t: 0.0,
            heading: 0.0,
            wander_t: 0.0,
            wander_dir: Vec2::ZERO,
            queue: Vec::new(),
            grid: Vec::new(),
            near: std::cell::RefCell::new(None),
            tally: Tally::default(),
        };
        sim.tally.lowest_hp = lowest_hp;
        for o in &sim.run.abilities {
            sim.tally.learned_at.insert(o.def.key, 0.0);
        }
        sim
    }

    /// Note any spell learned since the last look.
    pub fn note_learned(&mut self) {
        for o in &self.run.abilities {
            self.tally
                .learned_at
                .entry(o.def.key)
                .or_insert(self.run.elapsed);
        }
    }

    pub fn alive(&self) -> usize {
        self.foes.iter().filter(|f| !f.dead).count()
    }

    /// One step of `dt` seconds. Returns false once the hero has fallen or the bell has rung.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.run.hp <= 0.0 {
            return false;
        }
        if self.run.elapsed >= data::VICTORY_SECS {
            self.run.won = true;
            return false;
        }
        let stats = self.run.stats();
        self.moved();
        self.vitals(dt, &stats);
        self.procs(dt, &stats);
        self.hands(&stats);
        self.spawn(dt);
        self.move_hero(dt, &stats);
        self.move_foes(dt);
        self.moved();
        self.attacks(dt);
        let stats = self.run.stats();
        let keys = Keys::of(&self.run);
        self.cast(dt, &stats);
        self.tick_areas(dt, &stats);
        self.resolve(&keys, &stats);
        self.over_time(dt, &keys, &stats);
        self.collect(&stats);
        self.foes.retain(|f| !f.dead);
        self.tally.lowest_hp = self.tally.lowest_hp.min(self.run.hp);
        self.run.hp > 0.0
    }

    // --- The hero ------------------------------------------------------------------------------

    fn vitals(&mut self, dt: f32, stats: &Stats) {
        let run = &mut self.run;
        run.elapsed += dt;
        run.tick_actives(dt);
        let m = &mut run.mobile;
        m.cd = (m.cd - dt).max(0.0);
        m.haste_t = (m.haste_t - dt).max(0.0);
        m.vanish_t = (m.vanish_t - dt).max(0.0);
        for t in [&mut run.shield_t, &mut run.immune_t, &mut run.thorns_t] {
            *t = (*t - dt).max(0.0);
        }
        let mut heal = stats.regen * dt;
        if run.heal_t > 0.0 {
            heal += run.heal_rate * dt.min(run.heal_t);
            run.heal_t = (run.heal_t - dt).max(0.0);
        }
        let before = run.hp;
        run.hp = (run.hp + heal).min(stats.max_hp);
        self.tally.healed += f64::from(run.hp - before);
    }

    /// `procs::tick_build`, without the visuals.
    fn procs(&mut self, dt: f32, stats: &Stats) {
        let class = self.run.class.id;
        let b = &mut self.run.b;
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
        match class {
            1 => {
                b.res_t += dt;
                if b.res_t > 4.0 {
                    b.res = (b.res - 3.0 * dt).max(0.0);
                }
                if b.res >= 100.0 {
                    b.res = 0.0;
                    b.surge_t = 8.0;
                }
            }
            2 => {
                b.res_t -= dt;
                if b.res_t <= 0.0 {
                    b.res = 0.0;
                }
            }
            11 => {
                let len = if self.run.talents.iter().any(|t| t.key == "moonkin") {
                    20.0
                } else {
                    10.0
                };
                if b.nature_n >= 5 {
                    b.nature_n = 0;
                    b.lunar_t = len;
                }
                if b.arcane_n >= 5 {
                    b.arcane_n = 0;
                    b.solar_t = len;
                }
            }
            _ => {}
        }
        if self.run.has_relic("ephemeral") && self.run.b.ephemeral_cd <= 0.0 {
            self.run.b.ephemeral_cd = 20.0;
            self.run.b.ephemeral_t = 6.0;
        } else {
            self.run.b.ephemeral_cd = (self.run.b.ephemeral_cd - dt).max(0.0);
        }
        let near = self.sorted();
        let fighting = near.first().is_some_and(|&(d, _)| d <= 15.0);
        if self.run.race == 2 && fighting && self.run.b.fury_cd <= 0.0 {
            self.run.b.fury_cd = 45.0;
            self.run.b.fury_t = 12.0;
        }
        if self.run.race == 6
            && self.run.b.stomp_cd <= 0.0
            && near.iter().filter(|(d, _)| *d <= 6.0).count() >= 5
        {
            self.run.b.stomp_cd = 20.0;
            for f in &mut self.foes {
                if !f.dead && (f.pos - self.hero).length() <= 7.0 {
                    let t = 2.0 * stats.cc * if f.boss { 0.3 } else { 1.0 };
                    f.stun_t = f.stun_t.max(t);
                }
            }
        }
        if self.run.b.soul_fire {
            if let Some(i) = self.toughest(&near, 30.0, 1).first().copied() {
                self.run.b.soul_fire = false;
                self.run.b.res = 0.0;
                let mut dmg = (60.0 + 10.0 * self.run.level as f32)
                    * stats.might
                    * self.run.power(School::Fire.bit());
                if self.run.has_talent("soul_harvest") {
                    dmg *= 2.0;
                }
                let (dmg, crit) = roll(&mut self.rng, stats, dmg);
                self.queue.push(SHit {
                    crit,
                    flags: synergy::F_PROC,
                    ..SHit::proc(i, dmg, School::Fire, "Soul Fire (shards)")
                });
            }
        }
        if self.run.has_relic("sulfuras") && self.run.b.meteor_cd <= 0.0 {
            if let Some(c) = self.densest(&near, 25.0, 6.0, None) {
                self.run.b.meteor_cd = 8.0;
                let dmg = (40.0 + 4.0 * self.run.level as f32)
                    * stats.might
                    * self.run.power(School::Fire.bit());
                for i in self.around(c, 6.0) {
                    let (d, crit) = roll(&mut self.rng, stats, dmg);
                    self.queue.push(SHit {
                        crit,
                        burn: dmg * 0.2,
                        flags: synergy::F_PROC,
                        ..SHit::proc(i, d, School::Fire, "Sulfuras meteor")
                    });
                }
            }
        }
        if class == 3 {
            self.mark_t -= dt;
            if self.mark_t <= 0.0 {
                self.mark_t = 0.5;
                let n = if self.run.act_link(Link::MarkAll) {
                usize::MAX
            } else if self.run.has_talent("trueshot") {
                3
            } else {
                1
            };
                let marked = self.toughest(&near, 30.0, n);
                for (i, f) in self.foes.iter_mut().enumerate() {
                    f.st.hunted = marked.contains(&i) && !f.dead;
                }
            }
        }
    }

    /// Where the hero goes: the movement policy.
    fn move_hero(&mut self, dt: f32, stats: &Stats) {
        let speed = stats.speed * self.run.haste_mult();
        let mut push = Vec2::ZERO;
        let mut crowd = 0;
        for f in &self.foes {
            if f.dead {
                continue;
            }
            let d = self.hero - f.pos;
            let len = d.length();
            let reach = 7.0 + f.radius;
            if len < reach && len > 0.01 {
                push += d / len * (reach - len) / reach * 3.0;
                crowd += 1;
            }
        }
        let home = -self.hero;
        let movement = match self.movement {
            // Healthy and not hemmed in: plant the feet and fight; otherwise run for the gems.
            Movement::Adaptive => {
                let hp = self.run.hp / stats.max_hp.max(1.0);
                if hp > 0.7 && crowd < 4 {
                    Movement::Stand
                } else {
                    Movement::Collect
                }
            }
            m => m,
        };
        let dir = match movement {
            Movement::Adaptive => Vec2::ZERO,
            // Standing still: only the shove of the horde moves a stander, and only when it is
            // about to be swamped, as a player steps out of a ring.
            Movement::Stand => {
                if crowd >= 14 {
                    push.normalize_or_zero() * 0.5
                } else {
                    Vec2::ZERO
                }
            }
            // Kiting: away from the crowd, sweeping round the field so the horde trails behind.
            Movement::Kite => {
                self.heading += dt * 0.25;
                let circle = Vec2::new(self.heading.cos(), self.heading.sin());
                let mut want = circle + push;
                if home.length() > HOME_LEASH * 0.6 {
                    want += home.normalize() * 1.5;
                }
                want.normalize_or_zero()
            }
            // Collecting: kiting round the field, turning aside for a gem (or a chest) that does
            // not lie through the crowd.
            Movement::Collect => {
                self.heading += dt * 0.25;
                let circle = Vec2::new(self.heading.cos(), self.heading.sin());
                let away = push.normalize_or_zero();
                let gem = self
                    .gems
                    .iter()
                    .map(|g| (g.pos - self.hero, matches!(g.kind, GemKind::Chest(_))))
                    .filter(|(d, chest)| d.length() < if *chest { 40.0 } else { 18.0 })
                    .map(|(d, chest)| (d.normalize_or_zero(), d.length() * if chest { 0.3 } else { 1.0 }))
                    .filter(|(d, _)| crowd == 0 || d.dot(away) > -0.3)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(d, _)| d);
                let mut want = gem.map_or(circle, |g| g * 1.5 + circle * 0.3) + push;
                if home.length() > HOME_LEASH * 0.6 {
                    want += home.normalize() * 1.5;
                }
                want.normalize_or_zero()
            }
            // `auto::plan`, line for line.
            Movement::Auto => {
                let mut want = self
                    .gems
                    .iter()
                    .map(|g| g.pos - self.hero)
                    .filter(|d| d.length() < 30.0)
                    .min_by(|a, b| a.length().total_cmp(&b.length()))
                    .map_or(Vec2::ZERO, |d| d.normalize_or_zero());
                if want == Vec2::ZERO {
                    let a = self.run.elapsed * 0.35;
                    want = Vec2::new(a.cos(), a.sin()) * 0.5;
                    if home.length() > HOME_LEASH * 0.5 {
                        want += home.normalize();
                    }
                }
                (want + push).normalize_or_zero()
            }
            // A first run: running from what is near, late and loosely, and often just standing
            // to watch the spells go.
            Movement::Wander => {
                self.wander_t -= dt;
                if self.wander_t <= 0.0 {
                    self.wander_t = self.rng.range(1.0, 3.0);
                    self.wander_dir = if self.rng.f32() < 0.35 {
                        Vec2::ZERO
                    } else {
                        let a = self.rng.range(0.0, std::f32::consts::TAU);
                        Vec2::new(a.cos(), a.sin())
                    };
                }
                let mut want = self.wander_dir + push * 0.6;
                if home.length() > HOME_LEASH * 0.6 {
                    want += home.normalize();
                }
                want.normalize_or_zero()
            }
        };
        self.hero += dir * speed * dt;
    }

    // --- The horde -------------------------------------------------------------------------------

    fn spawn_one(&mut self, def: &'static EnemyDef, pos: Vec2, boss: bool, elite: bool) {
        let run = &self.run;
        let scale = self.tune.hp_scale(run.elapsed)
            * run.enemy_hp_mult()
            * self.tune.enemy_hp
            * if elite { 7.0 } else { 1.0 };
        let size = if elite { 1.45 } else { 1.0 };
        let hp = def.hp * scale;
        self.foes.push(Foe {
            def,
            pos,
            hp,
            max_hp: hp,
            speed: def.speed * if elite { 1.1 } else { 1.0 },
            damage: def.damage
                * self.tune.damage_scale(run.elapsed)
                * run.enemy_dmg_mult()
                * self.tune.enemy_damage
                * if elite { 1.5 } else { 1.0 },
            radius: (if def.big { 1.3 } else { 0.6 }) * def.scale * size,
            xp: def.xp * if elite { 8 } else { 1 },
            boss,
            elite,
            attack_cd: 0.5,
            slow: 1.0,
            slow_t: 0.0,
            root_t: 0.0,
            stun_t: 0.0,
            fear_t: 0.0,
            weaken: 1.0,
            weaken_t: 0.0,
            dots: Vec::new(),
            st: Status::default(),
            dead: false,
            last_src: "",
        });
    }

    fn ring_point(&mut self, r: f32) -> Vec2 {
        let a = self.rng.range(0.0, std::f32::consts::TAU);
        self.hero + Vec2::new(a.cos(), a.sin()) * r
    }

    /// `enemies::spawn_enemies`.
    fn spawn(&mut self, dt: f32) {
        let elapsed = self.run.elapsed;
        let roster = data::roster(self.map);
        let pool = data::wave_pool(elapsed, roster.enemies.len());
        let due_bosses = ((elapsed / data::BOSS_EVERY_SECS) as u32).min(roster.bosses.len() as u32);
        while self.run.bosses < due_bosses {
            let def = &roster.bosses[self.run.bosses as usize];
            let pos = self.ring_point(30.0);
            self.spawn_one(def, pos, true, false);
            self.run.bosses += 1;
        }
        let due_rings = ((elapsed + data::BOSS_EVERY_SECS / 2.0) / data::BOSS_EVERY_SECS) as u32;
        if self.run.rings < due_rings {
            self.run.rings = due_rings;
            let def = &roster.enemies[pool[0]];
            let n = 28 + (data::progress(elapsed) * 60.0) as usize;
            let gap = self.rng.range(0.0, std::f32::consts::TAU);
            for i in 0..n {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                if data::in_ring_gap(a, gap) {
                    continue;
                }
                let pos = self.hero + Vec2::new(a.cos(), a.sin()) * 26.0;
                self.spawn_one(def, pos, false, false);
            }
        }
        let due_elites = (elapsed / 55.0) as u32;
        if self.elites < due_elites {
            self.elites = due_elites;
            let def = &roster.enemies[pool[2].min(roster.enemies.len() - 1)];
            let pos = self.ring_point(28.0);
            self.spawn_one(def, pos, false, true);
        }
        self.spawn_t -= dt;
        if self.spawn_t > 0.0 {
            return;
        }
        self.spawn_t = 0.12;
        let alive = self.alive();
        let target = data::target_alive(elapsed);
        if alive >= target {
            return;
        }
        let n = (((target - alive) as f32) / 6.0).ceil().min(4.0) as usize;
        for _ in 0..n {
            let def = &roster.enemies[pool[self.rng.index(pool.len())]];
            let r = self.rng.range(34.0, 42.0);
            let pos = self.ring_point(r);
            self.spawn_one(def, pos, false, false);
        }
    }

    /// The chase, with the crowd kept apart on a coarse grid round the hero.
    fn move_foes(&mut self, dt: f32) {
        let hero = self.hero;
        let lost = self.run.mobile.vanish_t > 0.0;
        for f in &mut self.foes {
            if f.dead {
                continue;
            }
            // Left far behind: brought back round, as `enemies::move_enemies` does.
            let off = f.pos - hero;
            if off.length() > 75.0 {
                f.pos = hero + off.normalize_or_zero() * 38.0;
            }
            for t in [
                &mut f.slow_t,
                &mut f.root_t,
                &mut f.stun_t,
                &mut f.fear_t,
                &mut f.weaken_t,
                &mut f.attack_cd,
            ] {
                *t = (*t - dt).max(-1.0);
            }
            if f.slow_t <= 0.0 {
                f.slow = 1.0;
            }
            if f.weaken_t <= 0.0 {
                f.weaken = 1.0;
            }
            f.st.tick(dt);
            let frozen = f.st.frozen_t > 0.0;
            if f.stun_t > 0.0 || f.root_t > 0.0 || frozen || (lost && !f.boss) {
                continue;
            }
            let chill = 1.0 - 0.06 * f32::from(f.st.chill);
            let to = hero - f.pos;
            let dist = to.length();
            let speed = f.speed * f.slow * chill;
            if f.fear_t > 0.0 {
                f.pos -= to.normalize_or_zero() * speed * dt;
            } else if dist > HERO_RADIUS + f.radius + 0.3 {
                f.pos += to / dist * (speed * dt).min(dist - HERO_RADIUS - f.radius - 0.3);
            }
        }
        // The game pushes the crowd apart every frame; at a tenth of a second, twice a step
        // resolves it as firmly.
        self.separate();
        self.separate();
    }

    /// Separation: each pair in neighbouring cells pushed apart by half their overlap, and
    /// nobody inside the hero.
    fn separate(&mut self) {
        let hero = self.hero;
        let mut grid = std::mem::take(&mut self.grid);
        if grid.is_empty() {
            grid = vec![Vec::new(); GRID * GRID];
        }
        for cell in &mut grid {
            cell.clear();
        }
        let half = GRID as f32 * CELL / 2.0;
        let cell_of = |p: Vec2| -> Option<(usize, usize)> {
            let x = ((p.x - hero.x + half) / CELL).floor();
            let y = ((p.y - hero.y + half) / CELL).floor();
            (x >= 0.0 && y >= 0.0 && (x as usize) < GRID && (y as usize) < GRID)
                .then_some((x as usize, y as usize))
        };
        for (i, f) in self.foes.iter().enumerate() {
            if !f.dead {
                if let Some((x, y)) = cell_of(f.pos) {
                    grid[y * GRID + x].push(i as u16);
                }
            }
        }
        let mut shove = vec![Vec2::ZERO; self.foes.len()];
        for (i, f) in self.foes.iter().enumerate() {
            let Some((x, y)) = (!f.dead).then(|| cell_of(f.pos)).flatten() else {
                continue;
            };
            for gy in y.saturating_sub(1)..=(y + 1).min(GRID - 1) {
                for gx in x.saturating_sub(1)..=(x + 1).min(GRID - 1) {
                    for &j in &grid[gy * GRID + gx] {
                        let j = j as usize;
                        if j <= i {
                            continue;
                        }
                        let o = &self.foes[j];
                        let d = f.pos - o.pos;
                        let len = d.length();
                        let min = f.radius + o.radius;
                        if len < min && len > 1e-4 {
                            let p = d / len * (min - len) * 0.5;
                            shove[i] += p;
                            shove[j] -= p;
                        }
                    }
                }
            }
        }
        for (f, s) in self.foes.iter_mut().zip(shove) {
            f.pos += s;
            let d = f.pos - hero;
            let min = HERO_RADIUS + f.radius;
            if d.length() < min && d.length() > 1e-4 {
                f.pos = hero + d.normalize() * min;
            }
        }
        self.grid = grid;
    }

    /// The horde's blows, `enemies::move_enemies`'s contact half.
    fn attacks(&mut self, _dt: f32) {
        let lost = self.run.mobile.vanish_t > 0.0;
        // As many blows land as bodies fit round the hero: the ring's circumference over a body.
        let mut ring = 0.0;
        for i in 0..self.foes.len() {
            let f = &self.foes[i];
            if (lost && !f.boss)
                || f.dead
                || f.stun_t > 0.0
                || f.st.frozen_t > 0.0
                || f.fear_t > 0.0
                || f.attack_cd > 0.0
            {
                continue;
            }
            if (f.pos - self.hero).length() > HERO_RADIUS + f.radius + 0.35 {
                continue;
            }
            // Each body takes its width of the ring, at the ring's radius.
            ring += 2.0 * f.radius / (HERO_RADIUS + f.radius + 0.35);
            if ring > std::f32::consts::TAU * 1.15 {
                continue;
            }
            let (raw, boss) = (f.damage * f.weaken, f.boss);
            self.foes[i].attack_cd = if boss { 1.0 } else { 1.3 };
            let stats = self.run.stats();
            let warded = self.run.has_relic("quelserrar") && self.run.b.quel_cd <= 0.0;
            if warded {
                self.run.b.quel_cd = 6.0;
            }
            if !warded && self.rng.f32() >= stats.dodge {
                let taken = hero::hurt_hero(&mut self.run, raw);
                self.tally.damage_taken += f64::from(taken);
                let now = self.run.elapsed;
                let what = if boss { 2 } else { u8::from(self.foes[i].elite) };
                self.tally.recent.push_back((now, taken, what));
                while self.tally.recent.front().is_some_and(|r| r.0 < now - 5.0) {
                    self.tally.recent.pop_front();
                }
            }
            if self.run.thorns_t > 0.0 {
                let dmg = self.run.thorns * stats.might;
                self.queue.push(SHit {
                    schools: School::Physical.bit(),
                    ..SHit::proc(i, dmg, School::Physical, "Thorns")
                });
            }
            if self.run.has_relic("gift_arthas") && self.foes[i].dots.len() < 6 {
                self.foes[i].dots.push(Dot {
                    dps: 20.0 * stats.might,
                    left: 5.0,
                    school: School::Shadow.bit(),
                    haunt: false,
                    src: "Gift of Arthas",
                });
            }
            if self.run.hp <= 0.0 {
                return;
            }
        }
    }

    // --- The hand-cast spells: Q burst, E guard, F move ---------------------------------------

    /// When the player presses Q, E and F.
    fn hands(&mut self, stats: &Stats) {
        let hp = self.run.hp / stats.max_hp.max(1.0);
        let Some(kit) = self.run.kit() else {
            // Before the specialization only the class move can be had.
            if self.run.mobility().is_some() && !matches!(self.hands, Hands::Panic | Hands::Off) {
                let near = self.sorted();
                if hp < 0.35 && near.iter().take_while(|(d, _)| *d <= 6.0).count() >= 6 {
                    self.cast_move();
                }
            }
            return;
        };
        let near = self.sorted();
        let within = |r: f32| near.iter().take_while(|(d, _)| *d <= r).count();
        let boss_near = near.iter().any(|&(d, i)| d <= 25.0 && self.foes[i].boss);
        let ready = |slot: usize| self.run.act.cd[slot] <= 0.0;
        let move_ready = self.run.mobile.cd <= 0.0;
        let (burst, guard, mv) = match self.hands {
            Hands::Off => (false, false, false),
            Hands::Panic => (
                ready(0) && within(8.0) >= 10 && self.rng.f32() < 0.01,
                ready(1) && hp < 0.2,
                false,
            ),
            Hands::Eager => (
                ready(0) && within(10.0) >= 6,
                ready(1) && hp < 0.35,
                move_ready && hp < 0.3 && within(6.0) >= 5,
            ),
            Hands::Timed => {
                // A burst that cashes in a status waits until enough of the field carries it.
                let det = kit.burst.links.iter().find_map(|l| match *l {
                    Link::Detonate(d, r, _) => Some((d, r)),
                    _ => None,
                });
                let burst = ready(0)
                    && match det {
                        Some((d, r)) => {
                            let primed = near
                                .iter()
                                .take_while(|(dist, _)| *dist <= r)
                                .filter(|&&(_, i)| {
                                    let f = &self.foes[i];
                                    match d {
                                        Det::Burn => f.st.burn_t > 0.0,
                                        Det::Dots => !f.dots.is_empty(),
                                        Det::Sunder => f.st.sunder >= 3,
                                        Det::Judged => f.st.judged_t > 0.0,
                                    }
                                })
                                .count();
                            primed >= 10 || (boss_near && primed >= 3)
                        }
                        None => boss_near || within(12.0) >= 18,
                    };
                let guard = ready(1)
                    && ((hp < 0.5 && within(5.0) >= 4) || hp < 0.3 || (boss_near && hp < 0.6));
                let mv = move_ready
                    && match kit.mobility.kind {
                        Move::Charge(yd) => {
                            hp > 0.5 && near.first().is_some_and(|&(d, _)| d > 8.0 && d <= yd)
                        }
                        _ => (hp < 0.45 && within(6.0) >= 6) || within(4.0) >= 14,
                    };
                (burst, guard, mv)
            }
        };
        if burst {
            self.cast_active(0);
        }
        if guard {
            self.cast_active(1);
        }
        if mv {
            self.cast_move();
        }
    }

    /// `actives::cast`: the run's half is the game's own [`Run::activate`], each enemy's the
    /// game's own [`actives::touch`].
    fn cast_active(&mut self, slot: usize) {
        let Some(def) = self.run.activate(slot) else { return };
        let r = actives::reach(def);
        if r <= 0.0 {
            return;
        }
        let mut heal = 0.0;
        for i in 0..self.foes.len() {
            let f = &mut self.foes[i];
            if f.dead {
                continue;
            }
            let dist = (f.pos - self.hero).length();
            if dist > r {
                continue;
            }
            let dots_left: f32 = f.dots.iter().map(|d| d.dps * d.left.max(0.0)).sum();
            let t = actives::touch(def, &self.run, dist, &mut f.st, dots_left, f.boss);
            if t.clear_dots {
                f.dots.clear();
            }
            heal += t.heal;
            if t.damage > 0.0 || t.effect.is_some() {
                self.queue.push(SHit {
                    effect: t.effect.unwrap_or(Effect::None),
                    flags: synergy::F_PROC,
                    ..SHit::proc(i, t.damage, def.school, def.key)
                });
            }
        }
        if heal > 0.0 {
            let max = self.run.stats().max_hp;
            self.heal(heal * max);
        }
    }

    /// `mobility::cast_mobility`, aimed away from the crowd (or at a charge's target).
    fn cast_move(&mut self) {
        let Some(def) = self.run.mobility() else { return };
        if self.run.mobile.cd > 0.0 {
            return;
        }
        let near = self.sorted();
        match def.kind {
            Move::Blink(yd) => {
                let mut c = Vec2::ZERO;
                let mut n = 0.0;
                for &(d, i) in &near {
                    if d > 10.0 {
                        break;
                    }
                    c += self.foes[i].pos - self.hero;
                    n += 1.0;
                }
                let mut away = if n > 0.0 { -c / n } else { -self.hero };
                if self.hero.length() > HOME_LEASH * 0.6 {
                    away += -self.hero.normalize_or_zero() * away.length();
                }
                self.hero += away.normalize_or_zero() * yd;
            }
            Move::Charge(yd) => match near.first() {
                Some(&(d, i)) if d <= yd => {
                    let to = self.foes[i].pos;
                    self.foes[i].stun_t = self.foes[i].stun_t.max(def.secs);
                    self.hero = to - (to - self.hero).normalize_or_zero() * 1.5;
                }
                _ => return,
            },
            Move::Haste(x) => {
                self.run.mobile.haste = x;
                self.run.mobile.haste_t = def.secs;
            }
            Move::Vanish(x) => {
                self.run.mobile.haste = x;
                self.run.mobile.haste_t = def.secs;
                self.run.mobile.vanish_t = def.secs;
            }
            Move::Scatter(r) => {
                for f in &mut self.foes {
                    if !f.dead && !f.boss && (f.pos - self.hero).length() <= r {
                        f.fear_t = f.fear_t.max(def.secs);
                    }
                }
            }
            Move::Ward(r, effect) => {
                self.run.mobile.haste = 1.3;
                self.run.mobile.haste_t = def.secs;
                for f in &mut self.foes {
                    if !f.dead && (f.pos - self.hero).length() <= r {
                        let boss = if f.boss { 0.3 } else { 1.0 };
                        match effect {
                            Effect::Slow(m, t) if !f.boss => {
                                f.slow = f.slow.min(m);
                                f.slow_t = f.slow_t.max(t);
                            }
                            Effect::Root(t) => f.root_t = f.root_t.max(t * boss),
                            Effect::Stun(t) => f.stun_t = f.stun_t.max(t * boss),
                            Effect::Fear(t) if !f.boss => f.fear_t = f.fear_t.max(t),
                            _ => {}
                        }
                    }
                }
            }
        }
        self.run.mobile.cd = def.cooldown * if self.run.race == 7 { 0.75 } else { 1.0 };
        self.tally.moves += 1;
        self.moved();
    }

    // --- Targeting -------------------------------------------------------------------------------

    /// The live enemies by distance from the hero.
    fn sorted(&self) -> Vec<(f32, usize)> {
        if let Some(v) = self.near.borrow().as_ref() {
            return v.clone();
        }
        let v = self.sort_near();
        *self.near.borrow_mut() = Some(v.clone());
        v
    }

    /// Positions moved: the next [`Self::sorted`] sorts afresh.
    fn moved(&self) {
        *self.near.borrow_mut() = None;
    }

    fn sort_near(&self) -> Vec<(f32, usize)> {
        let mut v: Vec<(f32, usize)> = self
            .foes
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.dead)
            .map(|(i, f)| ((f.pos - self.hero).length(), i))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }

    fn nearest(&self, near: &[(f32, usize)], reach: f32, n: usize) -> Vec<usize> {
        near.iter()
            .filter(|(d, i)| *d <= reach + self.foes[*i].radius && !self.foes[*i].dead)
            .take(n)
            .map(|&(_, i)| i)
            .collect()
    }

    fn toughest(&self, near: &[(f32, usize)], reach: f32, n: usize) -> Vec<usize> {
        let mut v: Vec<usize> = near
            .iter()
            .filter(|(d, i)| *d <= reach && !self.foes[*i].dead)
            .map(|&(_, i)| i)
            .collect();
        v.sort_by(|a, b| self.foes[*b].hp.total_cmp(&self.foes[*a].hp));
        v.truncate(n);
        v
    }

    fn around(&self, c: Vec2, r: f32) -> Vec<usize> {
        self.foes
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.dead && (f.pos - c).length() <= r + f.radius * 0.5)
            .map(|(i, _)| i)
            .collect()
    }

    fn densest(&self, near: &[(f32, usize)], range: f32, r: f32, avoid: Option<Vec2>) -> Option<Vec2> {
        let cand: Vec<Vec2> = near
            .iter()
            .filter(|(d, _)| *d <= range)
            .map(|&(_, i)| self.foes[i].pos)
            .filter(|p| avoid.is_none_or(|a| (*p - a).length() > r * 1.5))
            .take(40)
            .collect();
        cand.iter()
            .map(|p| (*p, cand.iter().filter(|o| (**o - *p).length() <= r).count()))
            .max_by_key(|&(_, n)| n)
            .map(|(p, _)| p)
    }

    fn neighbours(&self, pos: Vec2, r: f32, n: usize, not: usize) -> Vec<usize> {
        let mut v: Vec<(f32, usize)> = self
            .foes
            .iter()
            .enumerate()
            .filter(|(i, f)| *i != not && !f.dead)
            .map(|(i, f)| ((f.pos - pos).length(), i))
            .filter(|(d, _)| *d <= r)
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().take(n).map(|(_, i)| i).collect()
    }

    // --- Casting -----------------------------------------------------------------------------------

    /// `abilities::cast_abilities`, without the visuals.
    fn cast(&mut self, dt: f32, stats: &Stats) {
        let near = self.sorted();
        let struck = std::mem::take(&mut self.run.b.struck);
        let mut due: Vec<(usize, f32, bool)> = Vec::new();
        for i in 0..self.run.abilities.len() {
            let o = &mut self.run.abilities[i];
            o.cd -= dt;
            o.reprisal_cd -= dt;
            if let Some((t, factor)) = o.echo.as_mut() {
                *t -= dt;
                if *t <= 0.0 {
                    let factor = *factor;
                    o.echo = None;
                    due.push((i, factor, true));
                }
            }
            if struck && o.reprisal_cd <= 0.0 && o.has_rune("reactive") {
                o.reprisal_cd = 2.0;
                due.push((i, 1.0, true));
            }
            if o.cd <= 0.0 {
                due.push((i, 1.0, false));
            }
        }
        let has_finisher = self.run.abilities.iter().any(|o| synergy::finisher(o.def.key));

        for (i, factor, extra) in due {
            let o = &self.run.abilities[i];
            let def: &'static AbilityDef = o.def;
            let schools = o.schools();
            let tune = self.tune.ability(def.key);
            let mut mult =
                o.damage_mult() * stats.might * self.run.power(schools) * factor * tune.damage;
            let mut flags = o.flags();
            let mut crit = false;
            let mut cc = stats.cc;
            let overload = !extra && o.has_rune("overload") && o.casts % 4 == 3;
            if overload {
                mult *= 3.0;
                crit = true;
            }
            if self.run.act_link(Link::AlwaysCrit) {
                crit = true;
            }
            let mut finishing = false;
            match self.run.class.id {
                4 if self.run.b.res >= 5.0
                    && synergy::offensive(def.kind)
                    && (synergy::finisher(def.key) || !has_finisher) =>
                {
                    finishing = true;
                    mult *= 3.0;
                    cc *= 2.0;
                    flags |= synergy::F_FINISHER;
                }
                8 if self.run.b.last_school != 0
                    && synergy::offensive(def.kind)
                    && schools & self.run.b.last_school == 0 =>
                {
                    mult *= if self.run.has_talent("combustion") {
                        1.45
                    } else {
                        1.2
                    };
                }
                1 if self.run.b.surge_t > 0.0 && self.run.has_talent("unbridled_wrath") => {
                    crit = true
                }
                _ => {}
            }
            let dmg = def.damage * mult;
            let dot = def.dot * mult * self.run.dot_power();
            let count = (o.count()
                + u32::from(self.run.has_relic("accuria") && synergy::counted(def.kind)))
                as usize;
            let radius = o.radius(stats);
            let duration = o.duration();
            let cooldown = o.cooldown(stats) * tune.cooldown;
            let effect = scaled(def.effect, cc);
            let src = def.key;
            let slot = i as u8;
            let tether = o.has_rune("tether");
            let twin = o.has_rune("twin");
            let forked = o.has_rune("forked");
            let totemic = o.has_rune("totemic");
            let mk = |rng: &mut Rng, target: usize, d: f32| {
                let (d, c) = if crit {
                    (d * stats.crit_mult, true)
                } else {
                    roll(rng, stats, d)
                };
                SHit {
                    target,
                    damage: d,
                    dot,
                    dot_dur: duration,
                    effect,
                    splash: if matches!(def.kind, Kind::Bolt | Kind::Strike) {
                        radius
                    } else {
                        0.0
                    },
                    crit: c,
                    schools,
                    flags,
                    slot,
                    burn: 0.0,
                    src,
                }
            };
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
                        self.toughest(&near, reach, count)
                    } else {
                        self.nearest(&near, reach, count)
                    };
                    for &t in &targets {
                        let h = mk(&mut self.rng, t, dmg);
                        self.queue.push(h);
                    }
                    !targets.is_empty()
                }
                Kind::Nova | Kind::Cone | Kind::Aura => {
                    let mut targets = self.around(self.hero, radius);
                    if def.kind == Kind::Cone {
                        if let Some(&first) = near.first().map(|(_, i)| i) {
                            let aim = (self.foes[first].pos - self.hero).normalize_or_zero();
                            let hero = self.hero;
                            targets.retain(|&i| {
                                let d = self.foes[i].pos - hero;
                                d.length() < 2.0 || d.normalize_or_zero().dot(aim) >= 0.8
                            });
                        }
                    }
                    for &t in &targets {
                        let h = mk(&mut self.rng, t, dmg);
                        self.queue.push(h);
                    }
                    !targets.is_empty()
                }
                Kind::Ground | Kind::GroundSelf => {
                    let center = if def.kind == Kind::GroundSelf || tether {
                        near.first()
                            .filter(|(d, _)| *d <= radius + 4.0)
                            .map(|_| self.hero)
                    } else {
                        self.densest(&near, def.range, radius, None)
                    };
                    match center {
                        None => false,
                        Some(c) => {
                            let mut centers = vec![c];
                            if twin {
                                if let Some(c2) = self.densest(&near, def.range, radius, Some(c)) {
                                    centers.push(c2);
                                }
                            }
                            let burst = def.duration <= 0.0;
                            for c in centers {
                                self.areas.push(Area {
                                    center: c,
                                    radius,
                                    damage: dmg,
                                    dot,
                                    dot_dur: if burst { 5.0 } else { 0.0 },
                                    left: if burst { 1.5 } else { duration },
                                    every: if burst { f32::INFINITY } else { 1.0 },
                                    next: if burst { 0.0 } else { 0.4 },
                                    effect,
                                    schools,
                                    flags,
                                    slot,
                                    follow: tether,
                                    src,
                                });
                            }
                            true
                        }
                    }
                }
                Kind::Chain => match self.nearest(&near, def.range, 1).first().copied() {
                    None => false,
                    Some(first) => {
                        let jumps = count + 1 + if forked { 3 } else { 0 };
                        let mut chain = vec![first];
                        while chain.len() < jumps {
                            let last = self.foes[chain[chain.len() - 1]].pos;
                            let next = self
                                .foes
                                .iter()
                                .enumerate()
                                .filter(|(j, f)| !f.dead && !chain.contains(j))
                                .map(|(j, f)| ((f.pos - last).length(), j))
                                .filter(|(d, _)| *d <= radius)
                                .min_by(|a, b| a.0.total_cmp(&b.0));
                            match next {
                                Some((_, j)) => chain.push(j),
                                None => break,
                            }
                        }
                        let falloff: f32 = if forked { 1.0 } else { 0.85 };
                        for (k, &t) in chain.iter().enumerate() {
                            let h = mk(&mut self.rng, t, dmg * falloff.powi(k as i32));
                            self.queue.push(h);
                        }
                        true
                    }
                },
                Kind::Dot => {
                    let mut pool: Vec<usize> = near
                        .iter()
                        .filter(|(d, _)| *d <= def.range)
                        .take(count * 3)
                        .map(|&(_, i)| i)
                        .collect();
                    if pool.is_empty() {
                        false
                    } else {
                        let mut n = 0;
                        while n < count && !pool.is_empty() {
                            let t = pool.swap_remove(self.rng.index(pool.len()));
                            let h = SHit {
                                damage: 0.0,
                                crit: false,
                                ..mk(&mut self.rng, t, 0.0)
                            };
                            self.queue.push(h);
                            n += 1;
                        }
                        true
                    }
                }
                Kind::Orbit => {
                    let targets = self.nearest(&near, radius, count);
                    for &t in &targets {
                        let h = mk(&mut self.rng, t, dmg);
                        self.queue.push(h);
                    }
                    !targets.is_empty()
                }
                Kind::Shield => {
                    self.run.shield = dmg;
                    self.run.shield_t = duration;
                    true
                }
                Kind::Immune => {
                    if near.first().is_some_and(|(d, _)| *d <= 5.0) {
                        self.run.immune_t = duration;
                        true
                    } else {
                        false
                    }
                }
                Kind::Thorns => {
                    if near.first().is_some_and(|(d, _)| *d <= 8.0) {
                        self.run.thorns = dmg;
                        self.run.thorns_t = duration;
                        true
                    } else {
                        false
                    }
                }
                Kind::Heal => {
                    if self.run.hp < stats.max_hp * 0.85 {
                        self.run.heal_rate = dmg / duration.max(1.0);
                        self.run.heal_t = duration;
                        true
                    } else {
                        false
                    }
                }
                Kind::Totem => {
                    if near
                        .first()
                        .is_some_and(|(d, _)| *d <= def.range.max(radius) + 6.0)
                    {
                        let n = if twin { 2 } else { 1 };
                        let pulse = matches!(def.key, "magma_totem" | "fire_nova_totem");
                        let every = if pulse { 2.0 } else { 1.0 } * if totemic { 0.5 } else { 1.0 };
                        for _ in 0..n {
                            let a = self.rng.range(0.0, std::f32::consts::TAU);
                            self.totems.push(TotemS {
                                pos: self.hero + Vec2::new(a.cos(), a.sin()) * 2.5,
                                damage: dmg,
                                range: def.range,
                                radius,
                                left: duration * if totemic { 1.5 } else { 1.0 },
                                every,
                                next: 0.3,
                                pulse,
                                schools,
                                src,
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
                    self.run.abilities[i].cd = 0.2;
                }
                continue;
            }
            if self.run.abilities[i].has_rune("radiant") {
                let value = if def.damage > 0.0 { dmg } else { 30.0 * mult };
                for t in self.around(self.hero, 8.0) {
                    let (d, c) = roll(&mut self.rng, stats, value * 1.5);
                    self.queue.push(SHit {
                        crit: c,
                        flags: synergy::F_PROC,
                        ..SHit::proc(t, d, School::Holy, "Sanctity Aura")
                    });
                }
            }
            self.gain_resource(i, schools, def, finishing);
            let echo_all = self.run.act_link(Link::Echo);
            let o = &mut self.run.abilities[i];
            if !extra {
                o.casts += 1;
                o.cd = cooldown;
                let talents = &self.run.talents;
                if talents.iter().any(|t| t.key == "clearcasting") && self.rng.f32() < 0.10 {
                    o.cd = 0.2;
                }
                let every = if talents.iter().any(|t| t.key == "elemental_mastery") {
                    3
                } else {
                    5
                };
                if echo_all {
                    o.echo = Some((0.25, 0.8));
                } else if o.has_rune("echo") {
                    o.echo = Some((0.8, 0.6));
                } else if self.run.class.id == 7 && self.run.b.casts % every == 0 {
                    o.echo = Some((0.25, 0.8));
                } else if self.run.relics.iter().any(|r| r.key == "hoj") && self.rng.f32() < 0.12
                {
                    o.echo = Some((0.15, 1.0));
                }
            }
        }
    }

    fn gain_resource(&mut self, i: usize, schools: u8, def: &AbilityDef, finishing: bool) {
        let run = &mut self.run;
        let zeal_cap = if run.has_talent("vengeance") { 12.0 } else { 8.0 };
        let rage = if run.has_talent("unbridled_wrath") { 6.0 } else { 3.0 };
        let b = &mut run.b;
        b.casts += 1;
        match run.class.id {
            4 if synergy::offensive(def.kind) => {
                b.res = if finishing { 0.0 } else { (b.res + 1.0).min(5.0) };
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
    }

    fn tick_areas(&mut self, dt: f32, stats: &Stats) {
        let hero = self.hero;
        let mut areas = std::mem::take(&mut self.areas);
        for a in &mut areas {
            if a.follow {
                a.center = hero;
            }
            a.left -= dt;
            a.next -= dt;
            if a.next <= 0.0 {
                a.next += a.every;
                for t in self.around(a.center, a.radius) {
                    let (d, crit) = roll(&mut self.rng, stats, a.damage);
                    self.queue.push(SHit {
                        target: t,
                        damage: d,
                        dot: a.dot,
                        dot_dur: a.dot_dur,
                        effect: a.effect,
                        splash: 0.0,
                        crit,
                        schools: a.schools,
                        flags: a.flags,
                        slot: a.slot,
                        burn: 0.0,
                        src: a.src,
                    });
                }
            }
        }
        areas.retain(|a| a.left > 0.0);
        self.areas = areas;

        let mut totems = std::mem::take(&mut self.totems);
        for t in &mut totems {
            t.left -= dt;
            t.next -= dt;
            if t.left <= 0.0 || t.next > 0.0 {
                continue;
            }
            t.next = t.every;
            let school = synergy::primary(t.schools);
            if t.pulse {
                for i in self.around(t.pos, t.radius) {
                    let (d, crit) = roll(&mut self.rng, stats, t.damage);
                    self.queue.push(SHit {
                        crit,
                        flags: synergy::F_PROC,
                        schools: t.schools,
                        ..SHit::proc(i, d, school, t.src)
                    });
                }
            } else if let Some(i) = self.neighbours(t.pos, t.range, 1, usize::MAX).first() {
                let (d, crit) = roll(&mut self.rng, stats, t.damage);
                self.queue.push(SHit {
                    crit,
                    flags: synergy::F_PROC,
                    schools: t.schools,
                    ..SHit::proc(*i, d, school, t.src)
                });
            }
        }
        totems.retain(|t| t.left > 0.0);
        self.totems = totems;
    }

    // --- Landing hits: `abilities::apply_hits` ---------------------------------------------------

    fn credit(&mut self, src: Src, dmg: f32) {
        *self.tally.damage_by.entry(src).or_insert(0.0) += f64::from(dmg);
        self.run.damage_done += f64::from(dmg);
    }

    fn heal(&mut self, amount: f32) {
        let before = self.run.hp;
        self.run.leech(amount);
        self.tally.healed += f64::from(self.run.hp - before);
    }

    fn resolve(&mut self, k: &Keys, stats: &Stats) {
        let mut thunder_left = 3u32;
        // Procs make more hits; a generous cap keeps a runaway chain finite.
        let mut budget = 20_000;
        while let Some(h) = self.queue.pop() {
            budget -= 1;
            if budget == 0 {
                self.queue.clear();
                break;
            }
            if h.splash > 0.0 {
                if let Some(c) = self.foes.get(h.target).filter(|f| !f.dead).map(|f| f.pos) {
                    for n in self.neighbours(c, h.splash, 64, h.target) {
                        self.queue.push(SHit {
                            target: n,
                            damage: h.damage * 0.7,
                            splash: 0.0,
                            flags: h.flags & !(synergy::F_PROC | synergy::F_SPLIT),
                            ..h.clone()
                        });
                    }
                }
            }
            self.land(SHit { splash: 0.0, ..h }, k, stats, &mut thunder_left);
        }
    }

    fn land(&mut self, h: SHit, k: &Keys, stats: &Stats, thunder_left: &mut u32) {
        if self.run.has_talent("flurry") && h.crit {
            self.run.b.flurry_t = 3.0;
        }
        let Some(en) = self.foes.get(h.target) else { return };
        if en.dead {
            return;
        }
        let s = h.schools;
        let frozen = en.st.frozen_t > 0.0;
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
        let pos = en.pos;
        let boss = en.boss;
        if dmg > 0.0 {
            let en = &mut self.foes[h.target];
            en.hp -= dmg;
            en.last_src = h.src;
            self.credit(h.src, dmg);
            if let Effect::Drain(frac) = h.effect {
                self.heal(dmg * frac);
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
                self.heal(leech);
            }
        }
        let en = &mut self.foes[h.target];
        if h.dot > 0.0 && h.dot_dur > 0.0 {
            if en.dots.len() >= 6 {
                en.dots.remove(0);
            }
            en.dots.push(Dot {
                dps: h.dot,
                left: h.dot_dur,
                school: s,
                haunt: h.flags & synergy::F_HAUNT != 0,
                src: h.src,
            });
            if h.flags & synergy::F_CONTAGION != 0 {
                en.st.contagion = true;
            }
        }
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

        let landed = dmg > 0.0 || h.dot > 0.0 || h.burn > 0.0;
        let reps: u8 = if h.flags & synergy::F_DOUBLE_STATUS != 0 { 2 } else { 1 };
        let procs = h.flags & synergy::F_PROC != 0;
        if landed {
            if k.frostfire && frozen && has(s, School::Fire) && dmg > 0.0 {
                self.foes[h.target].st.frozen_t = 0.0;
                for n in self.neighbours(pos, 5.0, 12, h.target) {
                    self.queue
                        .push(SHit::proc(n, dmg * 0.6, School::Fire, "Frostfire: Thermal Shock"));
                }
            }
            if k.frostblade && frozen && has(s, School::Physical) && dmg > 0.0 && procs {
                for n in self.neighbours(pos, 5.0, 12, h.target) {
                    self.queue
                        .push(SHit::proc(n, dmg * 0.5, School::Frost, "Frostmourne cleave"));
                }
            }
            let en = &mut self.foes[h.target];
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
                    }
                }
            }
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
            if has(s, School::Holy) {
                en.st.judged_t = 5.0;
            }
            if has(s, School::Physical) && dmg > 0.0 {
                en.st.sunder = (en.st.sunder + reps).min(k.sunder_cap);
                en.st.sunder_t = 6.0;
                if k.deep_wounds && h.crit && en.dots.len() < 6 {
                    en.dots.push(Dot {
                        dps: dmg * 0.5 / 6.0,
                        left: 6.0,
                        school: School::Physical.bit(),
                        haunt: false,
                        src: "Deep Wounds",
                    });
                }
                if k.spellblade && procs {
                    self.run.b.arcane = (self.run.b.arcane + 1).min(self.run.arcane_cap());
                    self.run.b.arcane_t = 8.0;
                }
            }
            let en = &mut self.foes[h.target];
            if has(s, School::Nature) {
                let extra = u8::from(k.rending_storm && en.st.sunder > 0);
                en.st.stat = en.st.stat.saturating_add(reps + extra);
                en.st.stat_t = 5.0;
                let at = if k.conduction { 2 } else { 3 };
                if procs && dmg > 0.0 && (en.st.stat >= at || (k.frozen_circuit && frozen)) {
                    en.st.stat = 0;
                    let charges = if k.arcane_storm { self.run.b.arcane } else { 0 };
                    let n = if k.conduction { 3 } else { 2 } + usize::from(charges);
                    let pct =
                        if k.conduction { 0.5 } else { 0.35 } * (1.0 + 0.1 * f32::from(charges));
                    for t in self.neighbours(pos, 8.0, n, h.target) {
                        let mut hit = SHit::proc(t, dmg * pct, School::Nature, "Shock: Chain Lightning");
                        if k.frozen_circuit {
                            hit.schools |= School::Frost.bit();
                            hit.flags |= synergy::F_DOUBLE_STATUS;
                        }
                        if k.rending_storm {
                            hit.schools |= School::Physical.bit();
                            hit.flags |= synergy::F_DOUBLE_STATUS;
                        }
                        self.queue.push(hit);
                    }
                }
            }
            if k.thunderfury && procs && dmg > 0.0 && *thunder_left > 0 && self.rng.f32() < 0.15 {
                *thunder_left -= 1;
                let bolt = (dmg * 0.6).max(20.0 * stats.might);
                for t in self.neighbours(pos, 10.0, 5, h.target) {
                    self.queue.push(SHit {
                        effect: Effect::Slow(0.7, 2.0),
                        ..SHit::proc(t, bolt, School::Nature, "Thunderfury")
                    });
                }
            }
        }
        if self.foes[h.target].hp <= 0.0 {
            self.die(h.target, h.slot, h.flags, h.schools, h.damage, k, stats);
        }
    }

    /// The damage over time and the burn, as a steady stream rather than once-a-second ticks.
    fn over_time(&mut self, dt: f32, k: &Keys, stats: &Stats) {
        let ignite = if k.ignite { 2.0 } else { 1.0 };
        for i in 0..self.foes.len() {
            if self.foes[i].dead {
                continue;
            }
            let mut dealt: Vec<(Src, f32)> = Vec::new();
            let mut haunted = 0.0;
            let en = &mut self.foes[i];
            for d in &mut en.dots {
                let step = dt.min(d.left);
                d.left -= dt;
                dealt.push((d.src, d.dps * step));
                if d.haunt {
                    haunted += d.dps * step * 0.2;
                }
            }
            en.dots.retain(|d| d.left > 0.0);
            if en.st.burn_t > 0.0 {
                let step = dt.min(en.st.burn_t);
                en.st.burn_t -= dt;
                dealt.push(("Ignite", en.st.burn * ignite * step));
                if en.st.burn_t <= 0.0 {
                    en.st.burn = 0.0;
                }
            }
            if haunted > 0.0 {
                self.heal(haunted);
            }
            let mut total = 0.0;
            for (src, d) in dealt {
                if d > 0.0 {
                    self.credit(src, d);
                    total += d;
                }
            }
            if total > 0.0 {
                let en = &mut self.foes[i];
                en.hp -= total;
                if en.hp <= 0.0 {
                    self.die(i, u8::MAX, 0, 0, 0.0, k, stats);
                }
            }
        }
    }

    // --- Death and its legacy ------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn die(&mut self, i: usize, slot: u8, flags: u16, schools: u8, damage: f32, k: &Keys, stats: &Stats) {
        let en = &mut self.foes[i];
        if en.dead {
            return;
        }
        en.dead = true;
        en.hp = 0.0;
        let pos = en.pos;
        let dots = std::mem::take(&mut en.dots);
        let st = en.st.clone();
        let (boss, elite, xp) = (en.boss, en.elite, en.xp);
        self.run.kills += 1;
        if boss {
            self.tally.bosses_killed += 1;
        }
        if elite {
            self.tally.elites_killed += 1;
        }
        // Loot: `enemies::drop_loot`.
        if boss || elite {
            self.gems.push(Gem {
                pos: pos - Vec2::X * 0.8,
                kind: GemKind::Chest(boss),
            });
        }
        if boss || self.rng.f32() < 0.015 {
            self.gems.push(Gem {
                pos: pos + Vec2::X * 0.8,
                kind: GemKind::Heal(if boss { 1000.0 } else { 30.0 }),
            });
        }
        let xp_gems = self.gems.iter().filter(|g| matches!(g.kind, GemKind::Xp(_))).count();
        if xp_gems >= MAX_GEMS && !boss {
            if let Some(g) = self.gems.iter_mut().find(|g| matches!(g.kind, GemKind::Xp(_))) {
                if let GemKind::Xp(x) = &mut g.kind {
                    *x += xp;
                }
            }
        } else {
            self.gems.push(Gem {
                pos,
                kind: GemKind::Xp(xp),
            });
        }

        let afflicted = !dots.is_empty() || st.burn_t > 0.0;
        if let Some(o) = self.run.abilities.get_mut(usize::from(slot)) {
            if o.has_rune("rampage") {
                o.rampage = (o.rampage + 0.01).min(0.8);
            }
            if flags & synergy::F_SPLIT != 0 && damage > 0.0 {
                for t in self.neighbours(pos, 10.0, 2, i) {
                    self.queue.push(SHit {
                        schools,
                        ..SHit::proc(t, damage * 0.5, School::Physical, "Multi-Shot (enchant)")
                    });
                }
            }
        }
        let spread = if st.contagion { 2 } else { usize::from(k.plague) };
        if spread > 0 && !dots.is_empty() {
            for t in self.neighbours(pos, 8.0, spread, i) {
                for d in &dots {
                    self.queue.push(SHit {
                        dot: d.dps,
                        dot_dur: d.left.max(3.0),
                        schools: d.school,
                        flags: if st.contagion { synergy::F_CONTAGION } else { 0 },
                        ..SHit::proc(t, 0.0, School::Shadow, d.src)
                    });
                }
            }
        }
        if k.ignite && st.burn > 0.0 {
            for t in self.neighbours(pos, 8.0, 2, i) {
                self.queue.push(SHit {
                    burn: st.burn,
                    ..SHit::proc(t, 0.0, School::Fire, "Ignite")
                });
            }
        }
        if k.twilight && afflicted {
            for t in self.neighbours(pos, 8.0, 12, i) {
                self.queue.push(SHit::proc(t, 1.0, School::Holy, "Penance"));
            }
        }
        if st.judged_t > 0.0 {
            self.heal(if k.zeal { 3.0 } else { 1.0 });
            if k.zeal {
                self.run.b.zeal = (self.run.b.zeal + 1).min(5);
                self.run.b.zeal_t = 6.0;
            }
        }
        if self.run.race == 5 {
            self.heal(0.5);
        }
        if k.ashbringer {
            self.heal(1.0);
        }
        if st.hunted && self.run.class.id == 3 {
            self.run.b.surge_t = 5.0;
        }
        let b = &mut self.run.b;
        if self.run.class.id == 9 && afflicted && b.shard_cd <= 0.0 && b.res < 5.0 {
            b.res += 1.0;
            b.shard_cd = if self.run.talents.iter().any(|t| t.key == "soul_harvest") {
                0.75
            } else {
                1.5
            };
            if b.res >= 5.0 {
                b.soul_fire = true;
            }
        }
        if boss {
            let run = &mut self.run;
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

    // --- Pickups -------------------------------------------------------------------------------------

    /// `enemies::collect_gems`: what lies within the pickup range comes in.
    fn collect(&mut self, stats: &Stats) {
        let hero = self.hero;
        let mut i = 0;
        while i < self.gems.len() {
            if (self.gems[i].pos - hero).length() <= stats.magnet + 0.9 {
                let g = self.gems.swap_remove(i);
                match g.kind {
                    GemKind::Xp(xp) => {
                        self.run.xp += xp as f32 * data::XP_RATE * stats.growth * self.tune.xp
                    }
                    GemKind::Heal(hp) => {
                        let before = self.run.hp;
                        self.run.hp = (self.run.hp + hp).min(stats.max_hp);
                        self.tally.healed += f64::from(self.run.hp - before);
                    }
                    GemKind::Chest(boss) => self.run.chests.push(boss),
                }
            } else {
                i += 1;
            }
        }
        while self.run.xp >= data::xp_to_next(self.run.level) {
            self.run.xp -= data::xp_to_next(self.run.level);
            self.run.level += 1;
            self.run.pending_levels += 1;
        }
    }

    /// Chests still on the ground at the end.
    pub fn finish(&mut self) {
        self.tally.chests_missed = self
            .gems
            .iter()
            .filter(|g| matches!(g.kind, GemKind::Chest(_)))
            .count() as u32;
    }
}

/// Crowd control lengthened by the build, as `abilities::scaled`.
fn scaled(e: Effect, cc: f32) -> Effect {
    match e {
        Effect::Root(t) => Effect::Root(t * cc),
        Effect::Stun(t) => Effect::Stun(t * cc),
        Effect::Fear(t) => Effect::Fear(t * cc),
        other => other,
    }
}

/// A fresh run as `hud::start_run` makes one.
pub(super) fn start(class: &'static data::ClassDef, race: u8) -> Run {
    let mut run = Run {
        class,
        race,
        ..Run::default()
    };
    run.abilities
        .push(Owned::new(data::ability(class.start), 0.5));
    if race == 1 {
        run.rerolls += 1;
        run.banishes += 1;
    }
    run.hp = run.stats().max_hp;
    run
}
