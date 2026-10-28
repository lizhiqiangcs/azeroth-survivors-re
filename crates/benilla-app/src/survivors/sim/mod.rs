//! The balance simulator: thousands of whole runs an hour, played by AI players, measured and
//! reported. Every number and rule comes from the game's own source: the class pools, the spell
//! numbers, the talents, items, enchants and combinations (`data.rs`, `synergy.rs`), the run's
//! stats and damage formulas (`Run`, `Owned`), the level-up cards and what a pick does
//! (`hud::next_offer`, `hud::pick`), the hero's damage taken (`hero::hurt_hero`). Change the game
//! and the next simulation plays the change. What the game does through the ECS (the spawner, the
//! chase, the casts and their hits) is redone in [`world`], and [`sync`] fails the build's tests
//! when a mechanic named in the game's systems is missing from it.
//!
//! Native only, from the test runner (two jobs, one thread, by the machine's power budget):
//!
//! ```text
//! cargo test -j 2 --release -p benilla-app --lib survivors::sim -- --ignored --nocapture
//! SIM_RUNS=2000 SIM_PROFILES=expert,novice SIM_CLASS=Mage cargo test ... sim_batch
//! SIM_ROUNDS=6 cargo test ... sim_tune
//! ```
//!
//! Reports land in `target/sim/<label>/`: `report.md`, and `runs.csv`, `abilities.csv`,
//! `cards.csv` for a spreadsheet. See `docs/SIMULATION.md`.

mod metrics;
mod policy;
mod report;
mod sync;
mod world;

use std::collections::HashMap;

use bevy::prelude::NextState;

use super::data;
use super::hud::{self, Offer};
use super::synergy::{self, Req};
use super::{Phase, Rng};
use metrics::Meter;
use policy::{Decision, Plan, Profile};

/// The step: a tenth of a second. A missile's flight or a blow's swing is shorter than the gaps
/// that matter to the balance, and the sim stays ~10× faster than at a sixtieth.
const DT: f32 = 0.1;

/// Overrides on the game's numbers, to try a change without rebuilding: a file of `key = value`
/// lines named by `SIM_TUNE`. `enemy_hp`, `enemy_damage` and `xp` scale the horde and the
/// growth; `<spell>` (or `<spell>.damage`) scales a spell's damage, `<spell>.cooldown` its
/// cooldown.
#[derive(Clone)]
pub(super) struct Tune {
    pub enemy_hp: f32,
    pub enemy_damage: f32,
    pub xp: f32,
    pub abilities: HashMap<String, AbilityTune>,
    /// The learned card values the expert reads (`target/sim/values.tsv`).
    pub values: HashMap<String, f32>,
    /// The horde's growth over the run, in place of `data::hp_scale` and `data::damage_scale`:
    /// `k·(1 + a·p + b·p²)` at run fraction `p`; `hp_k`, `hp_p`, `hp_p2`, `damage_k`,
    /// `damage_p`, `damage_p2` in a tune file.
    pub hp_curve: Option<(f32, f32, f32)>,
    pub damage_curve: Option<(f32, f32, f32)>,
}

impl Tune {
    pub fn hp_scale(&self, t: f32) -> f32 {
        let p = data::progress(t);
        self.hp_curve
            .map_or_else(|| data::hp_scale(t), |(k, a, b)| k * (1.0 + a * p + b * p * p))
    }

    pub fn damage_scale(&self, t: f32) -> f32 {
        let p = data::progress(t);
        self.damage_curve
            .map_or_else(|| data::damage_scale(t), |(k, a, b)| k * (1.0 + a * p + b * p * p))
    }
}

#[derive(Clone, Copy)]
pub(super) struct AbilityTune {
    pub damage: f32,
    pub cooldown: f32,
}

const NO_TUNE: AbilityTune = AbilityTune {
    damage: 1.0,
    cooldown: 1.0,
};

impl Default for Tune {
    fn default() -> Self {
        Self {
            enemy_hp: 1.0,
            enemy_damage: 1.0,
            xp: 1.0,
            abilities: HashMap::new(),
            values: HashMap::new(),
            hp_curve: None,
            damage_curve: None,
        }
    }
}

impl Tune {
    pub fn ability(&self, key: &str) -> AbilityTune {
        self.abilities.get(key).copied().unwrap_or(NO_TUNE)
    }

    pub fn parse(text: &str) -> Self {
        let mut t = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, Ok(v)) = (k.trim(), v.trim().parse::<f32>()) else {
                continue;
            };
            match k {
                "enemy_hp" => t.enemy_hp = v,
                "enemy_damage" => t.enemy_damage = v,
                "xp" => t.xp = v,
                "hp_k" => t.hp_curve.get_or_insert((1.35, 1.1, 0.9)).0 = v,
                "hp_p" => t.hp_curve.get_or_insert((1.35, 1.1, 0.9)).1 = v,
                "hp_p2" => t.hp_curve.get_or_insert((1.35, 1.1, 0.9)).2 = v,
                "damage_k" => t.damage_curve.get_or_insert((2.0, 1.0, 0.3)).0 = v,
                "damage_p" => t.damage_curve.get_or_insert((2.0, 1.0, 0.3)).1 = v,
                "damage_p2" => t.damage_curve.get_or_insert((2.0, 1.0, 0.3)).2 = v,
                _ => {
                    let (key, field) = k.split_once('.').unwrap_or((k, "damage"));
                    let a = t.abilities.entry(key.to_string()).or_insert(NO_TUNE);
                    match field {
                        "cooldown" => a.cooldown = v,
                        _ => a.damage = v,
                    }
                }
            }
        }
        t
    }

    pub fn write(&self) -> String {
        let mut out = format!(
            "enemy_hp = {:.3}\nenemy_damage = {:.3}\nxp = {:.3}\n",
            self.enemy_hp, self.enemy_damage, self.xp
        );
        let mut keys: Vec<_> = self.abilities.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        for (k, a) in keys {
            if (a.damage - 1.0).abs() > 1e-3 {
                out += &format!("{k} = {:.3}\n", a.damage);
            }
            if (a.cooldown - 1.0).abs() > 1e-3 {
                out += &format!("{k}.cooldown = {:.3}\n", a.cooldown);
            }
        }
        out
    }

    fn from_env() -> Self {
        let mut t = std::env::var("SIM_TUNE")
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| Self::parse(&s))
            .unwrap_or_default();
        if std::env::var("SIM_LEARNED").as_deref() != Ok("0") {
            t.values = read_values(&out_dir("").join("values.tsv"));
        }
        t
    }
}

/// One run to play.
#[derive(Clone, Copy)]
pub(super) struct Setup {
    pub class: &'static data::ClassDef,
    pub race: u8,
    pub map: usize,
    pub profile: Profile,
    pub seed: u64,
    /// The talent tree the player means to take.
    pub tree: u8,
}

/// One card hand: what was offered, how an expert would value each card, what was taken.
pub(super) struct Hand {
    pub t: f32,
    pub cards: Vec<String>,
    pub value: Vec<f32>,
    pub took: usize,
    pub rerolls: u32,
}

/// What one run left: the run's outcome and everything the reports measure.
pub(super) struct RunLog {
    pub setup: Setup,
    pub won: bool,
    pub time: f32,
    pub level: u32,
    pub kills: u32,
    pub damage: f64,
    /// The tree taken at the specialization, if any.
    pub spec: Option<u8>,
    pub hands: Vec<Hand>,
    pub learned: Vec<(&'static str, f32)>,
    pub damage_by: Vec<(&'static str, f64)>,
    pub combos: Vec<&'static str>,
    pub capstones: Vec<&'static str>,
    pub talents: Vec<&'static str>,
    pub items: Vec<&'static str>,
    pub meter: Meter,
    pub tally_taken: f64,
    pub tally_healed: f64,
    pub bosses_killed: u32,
    pub elites_killed: u32,
    /// Q, E and F casts.
    pub casts: [u32; 3],
    /// For a loss, the damage of the last five seconds by source: common foes, elites, bosses;
    /// and how many blows it took.
    pub death: Option<([f32; 3], u32)>,
}

/// Plays one run start to finish.
pub(super) fn play(setup: Setup, tune: &Tune) -> RunLog {
    let run = world::start(setup.class, setup.race);
    let rng = Rng(setup.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut sim = world::Sim::new(
        run,
        rng,
        tune,
        setup.profile.movement,
        setup.profile.hands,
        setup.map,
    );
    let plan = Plan {
        tree: setup.tree,
        values: &tune.values,
    };
    let mut offer = Offer::default();
    let mut next = NextState::<Phase>::default();
    let mut hands = Vec::new();
    let mut meter = Meter::default();
    loop {
        let alive = sim.step(DT);
        meter.sample(&sim, DT);
        if !alive {
            break;
        }
        if sim.run.pending_levels > 0 || !sim.run.chests.is_empty() {
            let before = meter.dps_now();
            cards(&mut sim, &mut offer, &mut next, &setup.profile, &plan, &mut hands);
            meter.picked(sim.run.elapsed, before);
            sim.note_learned();
        }
    }
    sim.finish();
    meter.end(&sim);
    let run = &sim.run;
    let spec = run.talents.iter().find_map(|t| match t.req {
        Req::Tree(_, tree, 0) => Some(tree),
        _ => None,
    });
    let mut learned: Vec<_> = sim.tally.learned_at.iter().map(|(k, t)| (*k, *t)).collect();
    learned.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut damage_by: Vec<_> = sim.tally.damage_by.iter().map(|(k, d)| (*k, *d)).collect();
    damage_by.sort_by(|a, b| b.1.total_cmp(&a.1));
    let combos = run
        .abilities
        .iter()
        .map(|o| o.def.key)
        .filter(|k| synergy::COMBOS.iter().any(|c| c.key == *k))
        .collect();
    let capstones = run
        .abilities
        .iter()
        .map(|o| o.def.key)
        .filter(|k| synergy::TREES.iter().any(|t| t.capstone == *k))
        .collect();
    RunLog {
        setup,
        won: run.won,
        time: run.elapsed.min(data::VICTORY_SECS),
        level: run.level,
        kills: run.kills,
        damage: run.damage_done,
        spec,
        hands,
        learned,
        damage_by,
        combos,
        capstones,
        talents: run.talents.iter().map(|t| t.key).collect(),
        items: run.relics.iter().map(|r| r.key).collect(),
        meter,
        tally_taken: sim.tally.damage_taken,
        tally_healed: sim.tally.healed,
        bosses_killed: sim.tally.bosses_killed,
        elites_killed: sim.tally.elites_killed,
        casts: [run.act.casts[0], run.act.casts[1], sim.tally.moves],
        death: (!run.won).then(|| {
            let mut by = [0.0; 3];
            for &(_, d, w) in &sim.tally.recent {
                by[usize::from(w)] += d;
            }
            (by, sim.tally.recent.len() as u32)
        }),
    }
}

/// The level-ups and chests waiting, decided one hand at a time as the game deals them.
fn cards(
    sim: &mut world::Sim,
    offer: &mut Offer,
    next: &mut NextState<Phase>,
    profile: &Profile,
    plan: &Plan<'_>,
    hands: &mut Vec<Hand>,
) {
    hud::next_offer(&sim.run, &mut sim.rng, offer);
    let mut rerolls = 0;
    for _ in 0..200 {
        if offer.len() == 0 {
            // Nothing left to deal: the levels are spent unseen, as the game would skip them.
            sim.run.pending_levels = 0;
            sim.run.chests.clear();
            return;
        }
        // Every hand is valued as an expert sees it, whoever plays it: the measure of the hand.
        let expert = Profile {
            picks: policy::Picks::Expert,
            ..*profile
        };
        let (_, value) = policy::decide(&expert, &sim.run, offer, plan, &mut Rng(7), 9);
        let (decision, _) = policy::decide(profile, &sim.run, offer, plan, &mut sim.rng, rerolls);
        match decision {
            Decision::Reroll => {
                if hud::reroll(&mut sim.run, offer, &mut sim.rng) {
                    rerolls += 1;
                } else {
                    rerolls = 99;
                }
            }
            Decision::Take(i) => {
                hands.push(Hand {
                    t: sim.run.elapsed,
                    cards: offer
                        .choices()
                        .iter()
                        .map(|&c| policy::card_key(c, &sim.run))
                        .collect(),
                    value,
                    took: i,
                    rerolls,
                });
                rerolls = 0;
                hud::pick(i, &mut sim.run, offer, &mut sim.rng, next);
                if sim.run.pending_levels == 0 && sim.run.chests.is_empty() {
                    return;
                }
                if offer.len() == 0 {
                    hud::next_offer(&sim.run, &mut sim.rng, offer);
                }
            }
        }
    }
    sim.run.pending_levels = 0;
    sim.run.chests.clear();
}

/// The runs a batch plays: every class, its races, maps and trees in turn, for each profile.
pub(super) fn setups(n: usize, profiles: &[Profile], class: Option<&str>, seed: u64) -> Vec<Setup> {
    let classes: Vec<&'static data::ClassDef> = data::CLASSES
        .iter()
        .filter(|c| class.is_none_or(|n| c.name.eq_ignore_ascii_case(n)))
        .collect();
    let maps = std::env::var("SIM_MAP")
        .ok()
        .and_then(|m| m.parse::<usize>().ok())
        .map_or_else(|| (0..data::MAPS.len()).collect(), |m| vec![m]);
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let profile = profiles[i % profiles.len()];
        let class = classes[(i / profiles.len()) % classes.len()];
        let races = policy::races(class.id);
        out.push(Setup {
            class,
            race: races[rng.index(races.len())],
            map: maps[rng.index(maps.len())],
            profile,
            seed: rng.next_u64(),
            tree: rng.index(policy::tree_count(class.id) as usize) as u8,
        });
    }
    out
}

/// Plays them all, on `SIM_THREADS` threads (one by default; never more than two).
pub(super) fn play_all(setups: &[Setup], tune: &Tune) -> Vec<RunLog> {
    let threads = std::env::var("SIM_THREADS")
        .ok()
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 2);
    let started = std::time::Instant::now();
    let total = setups.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let progress = |done: usize| {
        if done % 250 == 0 || done == total {
            let secs = started.elapsed().as_secs_f32();
            eprintln!(
                "  {done}/{total} runs, {secs:.0}s, {:.0} runs/hour",
                done as f32 / secs.max(1e-3) * 3600.0
            );
        }
    };
    let chunk = total.div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = setups
            .chunks(chunk.max(1))
            .map(|part| {
                let done = &done;
                s.spawn(move || {
                    part.iter()
                        .map(|&setup| {
                            let log = play(setup, tune);
                            progress(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1);
                            log
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a sim thread panicked"))
            .collect()
    })
}

/// The learned card values: `class:card<TAB>points` lines.
fn read_values(path: &std::path::Path) -> HashMap<String, f32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('\t')?;
            Some((k.to_string(), v.trim().parse().ok()?))
        })
        .collect()
}

/// Learning the cards' worth: random pickers only, the fair experiment, merged into
/// `values.tsv` for the expert to read. Rerun after a change to the cards.
#[test]
#[ignore = "the balance simulator; run on purpose"]
fn sim_learn() {
    let n = env_usize("SIM_RUNS", 3000);
    let class = std::env::var("SIM_CLASS").ok();
    let mut tune = Tune::from_env();
    tune.values.clear();
    let random = [policy::profile("random_picks").unwrap()];
    let logs = play_all(&setups(n, &random, class.as_deref(), 77), &tune);
    let path = out_dir("").join("values.tsv");
    let mut values = read_values(&path);
    values.extend(report::card_values(&logs));
    let mut lines: Vec<String> = values.iter().map(|(k, v)| format!("{k}\t{v:.2}")).collect();
    lines.sort();
    std::fs::write(&path, lines.join("\n")).unwrap();
    eprintln!("sim: {} card values in {}", values.len(), path.display());
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_profiles() -> Vec<Profile> {
    let names = std::env::var("SIM_PROFILES")
        .unwrap_or_else(|_| "expert,casual,novice,random_picks,expert_stand,expert_nokeys".into());
    names
        .split(',')
        .filter_map(|n| policy::profile(n.trim()))
        .collect()
}

pub(super) fn out_dir(label: &str) -> std::path::PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR").map_or_else(
        |_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        std::path::PathBuf::from,
    );
    let dir = target.join("sim").join(label);
    std::fs::create_dir_all(&dir).expect("make the report directory");
    dir
}

/// A batch: play, measure, report.
#[test]
#[ignore = "the balance simulator; run on purpose"]
fn sim_batch() {
    let n = env_usize("SIM_RUNS", 1000);
    let profiles = env_profiles();
    let class = std::env::var("SIM_CLASS").ok();
    let seed = env_usize("SIM_SEED", 1) as u64;
    let label = std::env::var("SIM_LABEL").unwrap_or_else(|_| "latest".into());
    let tune = Tune::from_env();
    eprintln!("sim: {n} runs, profiles {:?}", profiles.iter().map(|p| p.name).collect::<Vec<_>>());
    let started = std::time::Instant::now();
    let logs = play_all(&setups(n, &profiles, class.as_deref(), seed), &tune);
    let secs = started.elapsed().as_secs_f64();
    let dir = out_dir(&label);
    report::write(&dir, &logs, secs, &tune);
    eprintln!("sim: report in {}", dir.display());
}

/// Auto-tuning: rounds of expert runs, each pulling every spell's damage toward its class's
/// median and the horde's health toward the target win rate. Writes the tune file it ends on and
/// the base numbers it suggests for `data.rs`.
#[test]
#[ignore = "the balance simulator; run on purpose"]
fn sim_tune() {
    let rounds = env_usize("SIM_ROUNDS", 5);
    let n = env_usize("SIM_RUNS", 900);
    let class = std::env::var("SIM_CLASS").ok();
    let target: f32 = std::env::var("SIM_TARGET_WIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.6);
    let mut tune = Tune::from_env();
    let expert = [policy::profile("expert").unwrap()];
    let dir = out_dir("tune");
    let mut log = String::new();
    for round in 0..rounds {
        let logs = play_all(
            &setups(n, &expert, class.as_deref(), 1000 + round as u64),
            &tune,
        );
        let line = metrics::tune_round(&logs, &mut tune, target);
        eprintln!("tune round {round}: {line}");
        log += &format!("round {round}: {line}\n");
    }
    std::fs::write(dir.join("tune.txt"), tune.write()).unwrap();
    std::fs::write(dir.join("suggested.md"), metrics::suggest(&tune)).unwrap();
    std::fs::write(dir.join("rounds.txt"), log).unwrap();
    eprintln!("sim: tune in {}", dir.display());
}

/// A quick check that every class plays a whole run without panicking.
#[test]
fn sim_smoke() {
    let profiles = [policy::profile("expert").unwrap(), policy::profile("novice").unwrap()];
    let tune = Tune::default();
    for s in setups(data::CLASSES.len() * 2, &profiles, None, 42) {
        let log = play(s, &tune);
        assert!(log.time > 0.0);
        assert!(log.level >= 1);
    }
}

/// One run, second by second: `SIM_CLASS`, `SIM_PROFILES` (the first), `SIM_SEED`. For seeing
/// why a run went the way it did.
#[test]
#[ignore = "the balance simulator; run on purpose"]
fn sim_trace() {
    let profiles = env_profiles();
    let class = std::env::var("SIM_CLASS").ok();
    let seed = env_usize("SIM_SEED", 1) as u64;
    let setup = setups(1, &profiles[..1], class.as_deref(), seed)[0];
    let log = play(setup, &Tune::default());
    eprintln!(
        "{} {} race {} map {} tree {}: won {} at {:.0}s, level {}, kills {}",
        setup.profile.name, setup.class.name, setup.race, setup.map, setup.tree, log.won,
        log.time, log.level, log.kills
    );
    let m = &log.meter;
    for (i, ((hp, k), d)) in m.hp.iter().zip(&m.kills).zip(&m.dps).enumerate() {
        if i % 5 == 0 || *hp < 0.3 {
            eprintln!("  {i:>4}s hp {:>3.0}% kills {k:>3} dps {d:>8.0}", hp * 100.0);
        }
    }
    for h in &log.hands {
        eprintln!("  {:>4.0}s took {} of {:?}", h.t, h.cards[h.took], h.cards);
    }
    eprintln!("  damage: {:?}", &log.damage_by[..log.damage_by.len().min(8)]);
}

/// Checking the simulator against the game: the game's own test driver (`?survivors_auto=<class>`
/// in the browser, `WOW_SURVIVORS_AUTO` natively) logs its run every ten seconds; this plays the
/// same driver `SIM_RUNS` times and prints the same numbers at each minute, quartiles, to set
/// beside the game's log.
#[test]
#[ignore = "the balance simulator; run on purpose"]
fn sim_calibrate() {
    let n = env_usize("SIM_RUNS", 200);
    let class = std::env::var("SIM_CLASS").unwrap_or_else(|_| "Mage".into());
    let auto = [policy::profile("auto").unwrap()];
    let mut all = setups(n, &auto, Some(&class), 5);
    if let Ok(m) = std::env::var("SIM_MAP") {
        for s in &mut all {
            s.map = m.parse().unwrap_or(0);
        }
    }
    let logs = play_all(&all, &Tune::from_env());
    let won = logs.iter().filter(|l| l.won).count() as f32 * 100.0 / n as f32;
    eprintln!("sim auto {class}: {n} runs, won {won:.0}%");
    eprintln!("     t  alive  level q1/med/q3   kills q1/med/q3      hp% med");
    for m in 1..=15usize {
        let t = m * 60 - 1;
        let at: Vec<&RunLog> = logs.iter().filter(|l| l.meter.levels.len() > t).collect();
        if at.is_empty() {
            break;
        }
        let q = |mut v: Vec<f32>| {
            v.sort_by(|a, b| a.total_cmp(b));
            (v[v.len() / 4], v[v.len() / 2], v[v.len() * 3 / 4])
        };
        let lv = q(at.iter().map(|l| f32::from(l.meter.levels[t])).collect());
        let kl = q(at.iter().map(|l| l.meter.kill_total[t] as f32).collect());
        let hp = q(at.iter().map(|l| l.meter.hp[t] * 100.0).collect());
        eprintln!(
            "  {:>4}s  {:>4.0}%  {:>3.0}/{:>3.0}/{:>3.0}       {:>5.0}/{:>5.0}/{:>5.0}      {:>3.0}",
            t + 1,
            at.len() as f32 * 100.0 / n as f32,
            lv.0,
            lv.1,
            lv.2,
            kl.0,
            kl.1,
            kl.2,
            hp.1
        );
    }
}

/// The game's `?survivors_auto=<class>:late` scenario: the whole pool at its top level from two
/// thirds of the way in. The same build every time, so the spells' own machinery can be set beside
/// the game's log second by second.
#[test]
#[ignore = "the balance simulator; run on purpose"]
fn sim_calibrate_late() {
    let n = env_usize("SIM_RUNS", 50);
    let name = std::env::var("SIM_CLASS").unwrap_or_else(|_| "Mage".into());
    let class = data::CLASSES.iter().find(|c| c.name.eq_ignore_ascii_case(&name)).unwrap();
    let tune = Tune::default();
    let profile = policy::profile("auto").unwrap();
    let values = HashMap::new();
    let plan = Plan { tree: 0, values: &values };
    let marks = [5.0f32, 9.0, 12.0, 21.0, 31.0];
    let mut at: Vec<Vec<(f64, u32, u32)>> = vec![Vec::new(); marks.len()];
    for seed in 0..n as u64 {
        let mut run = world::start(class, policy::races(class.id)[0]);
        for key in run.class.pool {
            if !run.abilities.iter().any(|o| o.def.key == *key) {
                run.abilities.push(super::Owned::new(data::ability(key), 0.5));
            }
        }
        for o in &mut run.abilities {
            o.level = data::MAX_ABILITY_LEVEL;
        }
        run.elapsed = data::VICTORY_SECS * 2.0 / 3.0;
        run.bosses = 3;
        run.rings = 4;
        let start = run.elapsed;
        let mut sim = world::Sim::new(run, Rng(seed * 7 + 1), &tune, profile.movement, profile.hands, 0);
        let (mut offer, mut next, mut hands) = (Offer::default(), NextState::<Phase>::default(), Vec::new());
        let mut k = 0;
        while k < marks.len() && sim.step(DT) {
            if sim.run.pending_levels > 0 || !sim.run.chests.is_empty() {
                cards(&mut sim, &mut offer, &mut next, &profile, &plan, &mut hands);
            }
            if sim.run.elapsed - start >= marks[k] {
                at[k].push((sim.run.damage_done, sim.run.kills, sim.run.level));
                k += 1;
            }
        }
    }
    eprintln!("sim {name}:late, {n} runs (medians)");
    for (m, v) in marks.iter().zip(&mut at) {
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mid = v[v.len() / 2];
        let mut kills: Vec<u32> = v.iter().map(|x| x.1).collect();
        kills.sort();
        let mut lv: Vec<u32> = v.iter().map(|x| x.2).collect();
        lv.sort();
        eprintln!(
            "  +{m:>3.0}s damage {:>8.0}  kills {:>4}  level {:>3}",
            mid.0,
            kills[kills.len() / 2],
            lv[lv.len() / 2]
        );
    }
}
