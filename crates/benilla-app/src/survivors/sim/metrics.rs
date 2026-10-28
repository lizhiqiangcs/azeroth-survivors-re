//! What a run felt like, in numbers: a second-by-second record of the hero's health, the kills
//! and the damage, boiled down at the end to the measures of fun and frustration the report
//! adds up. And the auto-tuner, which reads the same records.

use std::collections::HashMap;

use super::super::data::{self, ABILITIES};
use super::super::synergy;
use super::world::Sim;
use super::{RunLog, Tune};

/// The per-second record, and its summary once the run is over.
#[derive(Default, Clone)]
pub(crate) struct Meter {
    acc: f32,
    last_kills: u32,
    last_damage: f64,
    /// Health as a fraction of the most, each second.
    pub hp: Vec<f32>,
    pub kills: Vec<u16>,
    pub dps: Vec<f32>,
    /// Enemies within 10 yards.
    pub near: Vec<u16>,
    /// The level and the kills so far, each second.
    pub levels: Vec<u16>,
    pub kill_total: Vec<u32>,
    /// Each hand's time and the damage per second just before it.
    picks: Vec<(f32, f32)>,
    pub s: Summary,
}

/// One run's experience.
#[derive(Default, Clone, Copy)]
pub(crate) struct Summary {
    /// Seconds of rampage: four or more kills in the second, and health above 60%.
    pub power_secs: f32,
    /// The most kills in one second.
    pub peak_kps: u16,
    /// Times health fell under 25% and came back over 50%: the escapes.
    pub close_calls: u32,
    /// Seconds under 30% health.
    pub danger_secs: f32,
    /// Picks after which the damage per second rose by 30% or more: power felt.
    pub spikes: u32,
    pub picks: u32,
    /// The longest wait between two hands.
    pub max_gap: f32,
    /// Health lost in the last five seconds, as a fraction of the most: 0.7 or more is a death
    /// out of nowhere.
    pub death_burst: f32,
    /// Seconds with 40 or more enemies within 10 yards.
    pub swamped_secs: f32,
}

impl Meter {
    pub fn sample(&mut self, sim: &Sim, dt: f32) {
        self.acc += dt;
        if self.acc < 1.0 - 1e-4 {
            return;
        }
        self.acc -= 1.0;
        let run = &sim.run;
        let max = run.stats().max_hp.max(1.0);
        self.hp.push((run.hp / max).clamp(0.0, 1.0));
        self.kills
            .push((run.kills - self.last_kills).min(u32::from(u16::MAX)) as u16);
        self.dps.push((run.damage_done - self.last_damage) as f32);
        self.levels.push(run.level.min(u32::from(u16::MAX)) as u16);
        self.kill_total.push(run.kills);
        self.last_kills = run.kills;
        self.last_damage = run.damage_done;
        let near = sim
            .foes
            .iter()
            .filter(|f| (f.pos - sim.hero).length() <= 10.0)
            .count();
        self.near.push(near.min(usize::from(u16::MAX)) as u16);
    }

    /// The mean damage per second over the last fifteen seconds.
    pub fn dps_now(&self) -> f32 {
        mean(&self.dps[self.dps.len().saturating_sub(15)..])
    }

    pub fn picked(&mut self, t: f32, before: f32) {
        self.picks.push((t, before));
    }

    pub fn end(&mut self, sim: &Sim) {
        let s = &mut self.s;
        s.power_secs = self
            .kills
            .iter()
            .zip(&self.hp)
            .filter(|(k, h)| **k >= 4 && **h > 0.6)
            .count() as f32;
        s.peak_kps = self.kills.iter().copied().max().unwrap_or(0);
        s.danger_secs = self.hp.iter().filter(|h| **h < 0.3).count() as f32;
        s.swamped_secs = self.near.iter().filter(|n| **n >= 40).count() as f32;
        let mut low = false;
        for &h in &self.hp {
            if h < 0.25 {
                low = true;
            } else if low && h > 0.5 {
                low = false;
                s.close_calls += 1;
            }
        }
        s.picks = self.picks.len() as u32;
        let mut last = 0.0;
        for &(t, before) in &self.picks {
            s.max_gap = s.max_gap.max(t - last);
            last = t;
            let i = t as usize;
            if i + 18 < self.dps.len() && before > 1.0 {
                let after = mean(&self.dps[i + 3..i + 18]);
                if after >= before * 1.3 {
                    s.spikes += 1;
                }
            }
        }
        if !sim.run.won {
            let tail = &self.hp[self.hp.len().saturating_sub(6)..];
            let top = tail.iter().copied().fold(0.0, f32::max);
            s.death_burst = top - sim.run.hp.max(0.0) / sim.run.stats().max_hp.max(1.0);
        }
    }
}

pub(crate) fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}

/// A spell of the game, or a proc's name.
pub(crate) fn is_spell(key: &str) -> bool {
    ABILITIES.iter().any(|a| a.key == key)
}

/// Each spell's damage per second while owned, over these runs: total damage over total time
/// owned, and the runs it was owned in.
pub(crate) fn spell_dps(logs: &[&RunLog]) -> HashMap<(u8, &'static str), (f64, f64, u32)> {
    let mut out: HashMap<(u8, &'static str), (f64, f64, u32)> = HashMap::new();
    for l in logs {
        for &(k, at) in &l.learned {
            let owned = f64::from((l.time - at).max(0.0));
            if owned < 20.0 {
                continue;
            }
            let dmg = l
                .damage_by
                .iter()
                .find(|(s, _)| *s == k)
                .map_or(0.0, |(_, d)| *d);
            let e = out.entry((l.setup.class.id, k)).or_insert((0.0, 0.0, 0));
            e.0 += dmg;
            e.1 += owned;
            e.2 += 1;
        }
    }
    out
}

/// One tuning round: spells toward their class's median damage per second (deep and combined
/// spells toward one and a half times it), the horde's health toward the target win rate.
pub(crate) fn tune_round(logs: &[RunLog], tune: &mut Tune, target: f32) -> String {
    let refs: Vec<&RunLog> = logs.iter().collect();
    let dps = spell_dps(&refs);
    let mut by_class: HashMap<u8, Vec<f64>> = HashMap::new();
    for (&(class, key), &(d, t, n)) in &dps {
        let def = data::ability(key);
        if n >= 10 && def.damage + def.dot > 0.0 && !synergy::bonus_spell(key) {
            by_class.entry(class).or_default().push(d / t);
        }
    }
    let medians: HashMap<u8, f64> = by_class
        .into_iter()
        .map(|(c, mut v)| {
            v.sort_by(|a, b| a.total_cmp(b));
            (c, v[v.len() / 2])
        })
        .collect();
    let mut moved = 0;
    for (&(class, key), &(d, t, n)) in &dps {
        let def = data::ability(key);
        if n < 10 || def.damage + def.dot <= 0.0 || d <= 0.0 {
            continue;
        }
        let Some(&median) = medians.get(&class) else {
            continue;
        };
        let goal = if synergy::bonus_spell(key) {
            median * 1.5
        } else {
            median
        };
        let step = ((goal / (d / t)) as f32).sqrt().clamp(0.8, 1.25);
        let a = tune.abilities.entry(key.to_string()).or_insert(super::NO_TUNE);
        // A spell several classes share settles on the mean of their pulls.
        a.damage = (a.damage * step).clamp(0.5, 2.0);
        if (step - 1.0).abs() > 0.03 {
            moved += 1;
        }
    }
    let wins = logs.iter().filter(|l| l.won).count() as f32 / logs.len().max(1) as f32;
    tune.enemy_hp = (tune.enemy_hp * (1.0 + (wins - target) * 0.8).clamp(0.85, 1.2)).clamp(0.4, 3.0);
    format!(
        "win {:.1}% (target {:.0}%), {} spells moved, enemy_hp now {:.3}",
        wins * 100.0,
        target * 100.0,
        moved,
        tune.enemy_hp
    )
}

/// What the tune means for `data.rs`: the new base numbers.
pub(crate) fn suggest(tune: &Tune) -> String {
    let mut out = String::from(
        "# 自动调参建议\n\n把下面的数值写回 `data.rs`，再跑一次 `sim_batch` 验证。\n\n",
    );
    if (tune.enemy_hp - 1.0).abs() > 0.01 {
        out += &format!(
            "- 敌人血量整体 ×{:.2}（改 `data::hp_scale` 的系数，或各 `EnemyDef.hp`）\n",
            tune.enemy_hp
        );
    }
    out += "\n| 技能 | 倍率 | damage | dot |\n|---|---|---|---|\n";
    let mut keys: Vec<_> = tune.abilities.iter().collect();
    keys.sort_by(|a, b| a.1.damage.total_cmp(&b.1.damage));
    for (k, a) in keys {
        if (a.damage - 1.0).abs() < 0.03 || !is_spell(k) {
            continue;
        }
        let d = data::ability(k);
        out += &format!(
            "| {k} | ×{:.2} | {:.1} → {:.1} | {:.1} → {:.1} |\n",
            a.damage,
            d.damage,
            d.damage * a.damage,
            d.dot,
            d.dot * a.damage
        );
    }
    out
}
