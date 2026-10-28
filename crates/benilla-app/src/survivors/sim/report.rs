//! The batch report: `report.md` for reading, CSVs for a spreadsheet.
//!
//! The experience scores are defined here, each a 0..100 against a target band, so a change to
//! the game reads as "strategy 64 → 71, frustration 22 → 30" rather than as a wall of tables.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use super::super::data;
use super::metrics::{self, mean};
use super::policy::{self, card_family};
use super::{RunLog, Tune};

fn pct(n: usize, d: usize) -> f32 {
    if d == 0 {
        0.0
    } else {
        n as f32 * 100.0 / d as f32
    }
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn win(logs: &[&RunLog]) -> f32 {
    pct(logs.iter().filter(|l| l.won).count(), logs.len())
}

/// How a hand looked to an expert.
#[derive(PartialEq)]
enum HandKind {
    /// Nothing worth having.
    Dead,
    /// One card far ahead of the rest: no real choice.
    Obvious,
    /// Two good cards close together: a real decision.
    Dilemma,
    Plain,
}

fn hand_kind(v: &[f32]) -> HandKind {
    let mut s: Vec<f32> = v.to_vec();
    s.sort_by(|a, b| b.total_cmp(a));
    let top = s.first().copied().unwrap_or(0.0);
    let second = s.get(1).copied().unwrap_or(0.0);
    if top < 40.0 {
        HandKind::Dead
    } else if top - second >= 30.0 {
        HandKind::Obvious
    } else if second >= 45.0 && top - second <= 8.0 {
        HandKind::Dilemma
    } else {
        HandKind::Plain
    }
}

/// The experience of one profile's runs.
struct Exp {
    n: usize,
    win: f32,
    median_time: f32,
    early: f32,
    sudden: f32,
    power_per_min: f32,
    peak_kps: f32,
    close_calls: f32,
    danger_pct: f32,
    spikes: f32,
    max_gap: f32,
    dry: f32,
    dead_hands: f32,
    obvious_hands: f32,
    dilemmas: f32,
    level: f32,
    /// Q, E, F casts per run.
    casts: [f32; 3],
}

fn exp(logs: &[&RunLog]) -> Exp {
    let n = logs.len();
    let deaths: Vec<&&RunLog> = logs.iter().filter(|l| !l.won).collect();
    let mins: f32 = logs.iter().map(|l| l.time / 60.0).sum::<f32>().max(1e-3);
    let hands: Vec<HandKind> = logs
        .iter()
        .flat_map(|l| l.hands.iter().map(|h| hand_kind(&h.value)))
        .collect();
    let hn = hands.len();
    Exp {
        n,
        win: win(logs),
        median_time: median(logs.iter().map(|l| l.time).collect()),
        early: pct(
            logs.iter().filter(|l| !l.won && l.time < 240.0).count(),
            n,
        ),
        sudden: pct(
            deaths.iter().filter(|l| l.meter.s.death_burst >= 0.7).count(),
            deaths.len(),
        ),
        power_per_min: logs.iter().map(|l| l.meter.s.power_secs).sum::<f32>() / mins,
        peak_kps: mean(&logs.iter().map(|l| f32::from(l.meter.s.peak_kps)).collect::<Vec<_>>()),
        close_calls: mean(&logs.iter().map(|l| l.meter.s.close_calls as f32).collect::<Vec<_>>()),
        danger_pct: logs.iter().map(|l| l.meter.s.danger_secs).sum::<f32>() / (mins * 60.0) * 100.0,
        spikes: mean(&logs.iter().map(|l| l.meter.s.spikes as f32).collect::<Vec<_>>()),
        max_gap: median(logs.iter().map(|l| l.meter.s.max_gap).collect()),
        dry: pct(logs.iter().filter(|l| l.meter.s.max_gap > 120.0).count(), n),
        dead_hands: pct(hands.iter().filter(|h| **h == HandKind::Dead).count(), hn),
        obvious_hands: pct(hands.iter().filter(|h| **h == HandKind::Obvious).count(), hn),
        dilemmas: pct(hands.iter().filter(|h| **h == HandKind::Dilemma).count(), hn),
        level: mean(&logs.iter().map(|l| l.level as f32).collect::<Vec<_>>()),
        casts: [0, 1, 2].map(|i| {
            mean(&logs.iter().map(|l| l.casts[i] as f32).collect::<Vec<_>>())
        }),
    }
}

fn band(x: f32, lo: f32, hi: f32) -> f32 {
    (x / lo).min(1.0) * 100.0 * if x > hi { (hi / x).max(0.0) } else { 1.0 }
}

fn verdict(score: f32) -> &'static str {
    if score >= 80.0 {
        "好"
    } else if score >= 55.0 {
        "尚可"
    } else {
        "需改"
    }
}

pub(crate) fn write(dir: &Path, logs: &[RunLog], secs: f64, tune: &Tune) {
    let mut md = String::new();
    let by_profile = |name: &str| -> Vec<&RunLog> {
        logs.iter().filter(|l| l.setup.profile.name == name).collect()
    };
    let profiles: Vec<&str> = {
        let mut seen = Vec::new();
        for l in logs {
            if !seen.contains(&l.setup.profile.name) {
                seen.push(l.setup.profile.name);
            }
        }
        seen
    };
    let _ = writeln!(md, "# 平衡模拟报告\n");
    let _ = writeln!(
        md,
        "{} 局，耗时 {:.0} 秒，**{:.0} 局/小时**（单线程）。调参覆盖：敌人血量 ×{:.2}，伤害 ×{:.2}，经验 ×{:.2}，{} 个技能改动。\n",
        logs.len(),
        secs,
        logs.len() as f64 / secs.max(1e-3) * 3600.0,
        tune.enemy_hp,
        tune.enemy_damage,
        tune.xp,
        tune.abilities.len()
    );

    // --- Experience scores --------------------------------------------------------------------
    let exps: HashMap<&str, Exp> = profiles.iter().map(|p| (*p, exp(&by_profile(p)))).collect();
    let main = exps.get("expert").or_else(|| exps.values().next());
    let _ = writeln!(md, "## 体验评分\n");
    let _ = writeln!(md, "| 维度 | 分数 | 判定 | 依据 |\n|---|---|---|---|");
    if let Some(e) = main {
        let challenge = (100.0 - (e.win - 60.0).abs() * 2.5).max(0.0);
        let _ = writeln!(
            md,
            "| 挑战 | {challenge:.0} | {} | 高手胜率 {:.1}%（目标 45–75%，最好 60%） |",
            verdict(challenge),
            e.win
        );
        let thrill = (band(e.power_per_min, 15.0, 45.0) + band(e.spikes, 6.0, 30.0) + band(e.peak_kps, 25.0, 200.0)) / 3.0;
        let _ = writeln!(
            md,
            "| 爽感 | {thrill:.0} | {} | 每分钟 {:.1} 秒割草（≥15）、每局 {:.1} 次选卡后伤害跃升 ≥30%（≥6）、单秒最多击杀 {:.0}（≥25） |",
            verdict(thrill),
            e.power_per_min,
            e.spikes,
            e.peak_kps
        );
        let tension = (band(e.close_calls, 1.0, 6.0) + band(e.danger_pct, 3.0, 15.0)) / 2.0;
        let _ = writeln!(
            md,
            "| 紧张 | {tension:.0} | {} | 每局 {:.2} 次残血脱险（≥1）、{:.1}% 时间低于 30% 血（3–15%） |",
            verdict(tension),
            e.close_calls,
            e.danger_pct
        );
        let impact = exps
            .get("random_picks")
            .map(|r| e.win - r.win);
        let specs = spec_rows(logs);
        let best = specs.iter().map(|r| r.2).fold(0.0, f32::max);
        let viable = pct(specs.iter().filter(|r| r.2 >= best - 15.0).count(), specs.len());
        let strategy = (impact.map_or(50.0, |i| band(i.max(0.0), 20.0, 100.0))
            + band(e.dilemmas, 25.0, 100.0)
            + viable)
            / 3.0;
        let _ = writeln!(
            md,
            "| 策略 | {strategy:.0} | {} | 选择的价值（高手 − 随机选卡胜率）{}（≥20pp）、两难抉择 {:.1}% 的手牌（≥25%）、{:.0}% 的专精可用（与最强差 15pp 内） |",
            verdict(strategy),
            impact.map_or("未测".into(), |i| format!("{i:+.1}pp")),
            e.dilemmas,
            viable
        );
        if let Some(nk) = exps.get("expert_nokeys") {
            let keys = e.win - nk.win;
            let score = band(keys.max(0.0), 8.0, 35.0);
            let _ = writeln!(
                md,
                "| 操作价值 | {score:.0} | {} | 会用 Q/E/F 的高手 − 几乎不按的高手胜率 {keys:+.1}pp（目标 8–35pp：按得好有回报，不按也不至于没法玩） |",
                verdict(score)
            );
        }
        let footwork = exps.get("expert_stand").map(|s| e.win - s.win);
        if let Some(f) = footwork {
            let _ = writeln!(
                md,
                "| 走位价值 | {:.0} | {} | 高手走位 − 站桩胜率 {f:+.1}pp（目标 10–40pp：走位有用，但站桩流也能活） |",
                band(f.max(0.0), 10.0, 40.0),
                verdict(band(f.max(0.0), 10.0, 40.0))
            );
        }
        if let Some(nv) = exps.get("novice") {
            let access = (band(nv.median_time, 300.0, 900.0) + band(nv.win.max(0.1), 5.0, 35.0)) / 2.0;
            let _ = writeln!(
                md,
                "| 上手 | {access:.0} | {} | 新手中位存活 {:.0} 秒（≥300）、新手胜率 {:.1}%（5–35%） |",
                verdict(access),
                nv.median_time,
                nv.win
            );
        }
        let all: Vec<&RunLog> = logs.iter().collect();
        let a = exp(&all);
        let frustration = (a.early * 1.0 + a.sudden * 0.5 + a.dead_hands * 1.0 + a.dry * 0.5).min(100.0);
        let _ = writeln!(
            md,
            "| 挫败（越低越好） | {frustration:.0} | {} | 4 分钟内阵亡 {:.1}%、阵亡中 {:.1}% 为 5 秒内满血暴毙、{:.1}% 手牌无可选、{:.1}% 局有 >2 分钟等不到升级 |\n",
            if frustration <= 15.0 { "好" } else if frustration <= 30.0 { "尚可" } else { "需改" },
            a.early,
            a.sudden,
            a.dead_hands,
            a.dry
        );
    }

    let _ = writeln!(md, "## 各类玩家\n");
    let _ = writeln!(md, "| 玩家 | 局数 | 胜率 | 中位存活 | 平均等级 | 早死 | 暴毙 | 割草秒/分 | 跃升/局 | 脱险/局 | 危险时间 | 最长等待 | 无用手牌 | 无脑手牌 | 两难手牌 | Q/E/F 每局 |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for p in &profiles {
        let e = &exps[p];
        let _ = writeln!(
            md,
            "| {p} | {} | {:.1}% | {:.0}s | {:.1} | {:.1}% | {:.1}% | {:.1} | {:.1} | {:.2} | {:.1}% | {:.0}s | {:.1}% | {:.1}% | {:.1}% | {:.1}/{:.1}/{:.1} |",
            e.n, e.win, e.median_time, e.level, e.early, e.sudden, e.power_per_min, e.spikes,
            e.close_calls, e.danger_pct, e.max_gap, e.dead_hands, e.obvious_hands, e.dilemmas,
            e.casts[0], e.casts[1], e.casts[2]
        );
    }
    let _ = writeln!(md, "\n玩家：novice 随机选卡+站桩；casual 爱拿新技能+绕圈风筝；expert 按专精/联动规划、会重抽、健康时站桩输出、残血或被围时风筝捡经验；expert_stand 同 expert 但只站桩；random_picks 随机选卡但走位同 expert；expert_nokeys 同 expert 但几乎不按 Q/E/F。\n");

    // --- Deaths over time ------------------------------------------------------------------------
    let _ = writeln!(md, "## 阵亡时间分布（每分钟，按玩家）\n");
    let _ = write!(md, "| 玩家 |");
    for m in 0..15 {
        let _ = write!(md, " {m} |");
    }
    let _ = writeln!(md, "\n|---|{}", "---|".repeat(15));
    for p in &profiles {
        let runs = by_profile(p);
        let _ = write!(md, "| {p} |");
        for m in 0..15 {
            let n = runs
                .iter()
                .filter(|l| !l.won && (l.time / 60.0) as usize == m)
                .count();
            let _ = write!(md, " {:.0}% |", pct(n, runs.len()));
        }
        let _ = writeln!(md);
    }
    let _ = writeln!(md, "\nBoss 每 {:.0} 秒一个；尖峰落在 Boss 分钟说明 Boss 太硬或伤害跳变。\n", data::BOSS_EVERY_SECS);
    let _ = writeln!(md, "### 死因：死前 5 秒的伤害来自哪里\n");
    let _ = writeln!(md, "| 玩家 | 阵亡局 | 小怪 | 精英 | Boss | 5 秒内挨打次数 |\n|---|---|---|---|---|---|");
    for p in &profiles {
        let deaths: Vec<([f32; 3], u32)> = by_profile(p).iter().filter_map(|l| l.death).collect();
        if deaths.is_empty() {
            continue;
        }
        let mut by = [0.0f32; 3];
        for (d, _) in &deaths {
            for k in 0..3 {
                by[k] += d[k];
            }
        }
        let total = by.iter().sum::<f32>().max(1e-3);
        let blows = mean(&deaths.iter().map(|d| d.1 as f32).collect::<Vec<_>>());
        let _ = writeln!(
            md,
            "| {p} | {} | {:.0}% | {:.0}% | {:.0}% | {blows:.0} |",
            deaths.len(),
            by[0] / total * 100.0,
            by[1] / total * 100.0,
            by[2] / total * 100.0
        );
    }
    let _ = writeln!(md);

    // --- Classes and specs -------------------------------------------------------------------------
    let _ = writeln!(md, "## 职业 × 专精（高手）\n");
    let _ = writeln!(md, "| 职业 | 专精 | 局数 | 胜率 | 中位存活 | 平均等级 | 联动技/局 | 深层技/局 | 爆发技 | 爆发直伤占比 |\n|---|---|---|---|---|---|---|---|---|---|");
    for (class, tree, w, n, t, lvl, combos, caps) in spec_rows(logs) {
        let kit = tree.and_then(|t| {
            super::super::actives::KITS
                .iter()
                .find(|k| k.class == class && k.tree == t)
        });
        let share = kit.map_or(0.0, |k| {
            let runs: Vec<&RunLog> = logs
                .iter()
                .filter(|l| {
                    l.setup.profile.name == "expert" && l.setup.class.id == class && l.spec == tree
                })
                .collect();
            let total: f64 = runs.iter().map(|l| l.damage).sum();
            let burst: f64 = runs
                .iter()
                .flat_map(|l| l.damage_by.iter())
                .filter(|(s, _)| *s == k.burst.key)
                .map(|(_, d)| *d)
                .sum();
            burst / total.max(1.0) * 100.0
        });
        let _ = writeln!(
            md,
            "| {} | {} | {n} | {w:.1}% | {t:.0}s | {lvl:.1} | {combos:.2} | {caps:.2} | {} | {share:.1}% |",
            policy::class_name(class),
            tree.map_or("（未专精）", |t| policy::tree_name(class, t)),
            kit.map_or("-", |k| k.burst.name)
        );
    }
    let _ = writeln!(md, "\n爆发直伤占比只计爆发技能自身的直接伤害（引爆、新星）；纯增益型爆发（鲁莽、死亡之愿等）为 0，其价值看“操作价值”一行。");

    // --- Spells --------------------------------------------------------------------------------------
    let experts: Vec<&RunLog> = {
        let e = by_profile("expert");
        if e.is_empty() {
            logs.iter().collect()
        } else {
            e
        }
    };
    let dps = metrics::spell_dps(&experts);
    let mut class_median: HashMap<u8, f64> = HashMap::new();
    {
        let mut v: HashMap<u8, Vec<f64>> = HashMap::new();
        for (&(c, k), &(d, t, n)) in &dps {
            if n >= 5 && d > 0.0 && metrics::is_spell(k) {
                v.entry(c).or_default().push(d / t);
            }
        }
        for (c, mut x) in v {
            x.sort_by(|a, b| a.total_cmp(b));
            class_median.insert(c, x[x.len() / 2]);
        }
    }
    let mut spell_rows: Vec<(u8, &str, u32, f64, f64, f32, f32)> = Vec::new();
    for (&(c, k), &(d, t, n)) in &dps {
        let owned: Vec<&&RunLog> = experts
            .iter()
            .filter(|l| l.setup.class.id == c && l.learned.iter().any(|(s, _)| *s == k))
            .collect();
        let not: Vec<&&RunLog> = experts
            .iter()
            .filter(|l| l.setup.class.id == c && !l.learned.iter().any(|(s, _)| *s == k))
            .collect();
        let w_owned = pct(owned.iter().filter(|l| l.won).count(), owned.len());
        let w_not = pct(not.iter().filter(|l| l.won).count(), not.len());
        let ratio = class_median.get(&c).map_or(0.0, |m| d / t / m.max(1e-6));
        spell_rows.push((c, k, n, d / t, ratio, w_owned, w_owned - w_not));
    }
    spell_rows.sort_by(|a, b| a.0.cmp(&b.0).then(b.4.total_cmp(&a.4)));
    let _ = writeln!(md, "\n## 技能强度（高手局）\n");
    let _ = writeln!(md, "持有期间每秒伤害，与本职业中位数之比。>1.6 过强、<0.5 过弱（防御/治疗技能伤害为 0 属正常）。\n");
    let _ = writeln!(md, "| 职业 | 技能 | 持有局数 | 持有期 DPS | 对职业中位 | 持有时胜率 | 胜率差 | 标记 |\n|---|---|---|---|---|---|---|---|");
    let mut csv = String::from("class,spell,runs,dps_owned,ratio_to_median,win_owned,win_lift\n");
    let mut flags = Vec::new();
    for &(c, k, n, d, r, w, lift) in &spell_rows {
        let def_damage = if metrics::is_spell(k) {
            let d = data::ability(k);
            d.damage + d.dot
        } else {
            1.0
        };
        let flag = if n < 5 {
            "样本少"
        } else if r > 1.6 {
            "过强"
        } else if r < 0.5 && def_damage > 0.0 {
            "过弱"
        } else {
            ""
        };
        if !flag.is_empty() && flag != "样本少" {
            flags.push(format!("{} 的 {k}：{flag}（{r:.2}× 中位）", policy::class_name(c)));
        }
        let _ = writeln!(
            md,
            "| {} | {k} | {n} | {d:.0} | {r:.2} | {w:.1}% | {lift:+.1} | {flag} |",
            policy::class_name(c)
        );
        let _ = writeln!(csv, "{},{k},{n},{d:.1},{r:.3},{w:.1},{lift:.1}", policy::class_name(c));
    }
    let _ = std::fs::write(dir.join("abilities.csv"), csv);

    // --- Damage sources, procs included ---------------------------------------------------------------
    let mut src: HashMap<&str, f64> = HashMap::new();
    let mut total = 0.0;
    for l in &experts {
        for &(k, d) in &l.damage_by {
            *src.entry(k).or_default() += d;
            total += d;
        }
    }
    let mut src: Vec<_> = src.into_iter().collect();
    src.sort_by(|a, b| b.1.total_cmp(&a.1));
    let _ = writeln!(md, "\n## 伤害来源（含触发效果，高手局）\n\n| 来源 | 占比 |\n|---|---|");
    for (k, d) in src.iter().take(30) {
        let _ = writeln!(md, "| {k} | {:.1}% |", d / total.max(1.0) * 100.0);
    }

    // --- Cards ----------------------------------------------------------------------------------------
    // Measured on the random pickers where they were played: a card taken at random is a fair
    // experiment, where an expert's picks are confounded with the rest of its plan. Each card is
    // judged against the same chance passed up, so the runs that lived long enough to be offered
    // it are compared with each other.
    let fair: Vec<&RunLog> = {
        let r = by_profile("random_picks");
        if r.len() >= 100 {
            r
        } else {
            logs.iter().collect()
        }
    };
    let lifts = card_lifts(&fair);
    let mut pick_rate: HashMap<String, (u32, u32)> = HashMap::new();
    for l in by_profile("expert") {
        for h in &l.hands {
            for (j, c) in h.cards.iter().enumerate() {
                let e = pick_rate.entry(card_family(c).to_string()).or_default();
                e.0 += 1;
                if j == h.took {
                    e.1 += 1;
                }
            }
        }
    }
    let mut rows: Vec<(String, usize, usize, f32)> = lifts
        .iter()
        .map(|(k, &(took, passed, lift))| (k.clone(), took, passed, lift))
        .collect();
    rows.sort_by(|a, b| b.3.total_cmp(&a.3));
    let _ = writeln!(md, "\n## 卡牌（随机选卡局：拿了 vs 被提供却没拿的胜率差 = 因果效应；高手选取率）\n");
    let _ = writeln!(md, "选取率近 100% 且提升高 = 无脑必选，削弱或给替代；提升为负 = 陷阱卡。只列两边各 ≥15 局的卡。\n");
    let _ = writeln!(md, "| 卡 | 拿了的局 | 没拿的局 | 胜率提升 | 高手选取率 |\n|---|---|---|---|---|");
    let mut csv = String::from("card,runs_taken,runs_passed,win_lift,expert_pick_rate\n");
    let rate = |k: &str| {
        pick_rate
            .get(k)
            .map_or(0.0, |&(o, p)| pct(p as usize, o as usize))
    };
    for (k, t, p, lift) in &rows {
        let _ = writeln!(csv, "{k},{t},{p},{lift:.2},{:.1}", rate(k));
    }
    let shown: Vec<_> = rows.iter().filter(|r| r.1 >= 15 && r.2 >= 15).collect();
    for (i, (k, t, p, lift)) in shown.iter().enumerate() {
        if i == 25 && shown.len() > 37 {
            let _ = writeln!(md, "| … | | | | |");
        }
        if i < 25 || i + 12 >= shown.len() {
            let _ = writeln!(md, "| {k} | {t} | {p} | {lift:+.1}pp | {:.0}% |", rate(k));
        }
    }
    let _ = std::fs::write(dir.join("cards.csv"), csv);

    // --- Combos and deep spells --------------------------------------------------------------------------
    let _ = writeln!(md, "\n## 联动技与深层技（高手局）\n\n| 技能 | 获得局数 | 获得率（本职业） | 获得时中位时间 | 获得局胜率 |\n|---|---|---|---|---|");
    let mut got: HashMap<&str, Vec<&RunLog>> = HashMap::new();
    for l in &experts {
        for k in l.combos.iter().chain(&l.capstones) {
            got.entry(k).or_default().push(l);
        }
    }
    let mut got: Vec<_> = got.into_iter().collect();
    got.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (k, runs) in &got {
        let class = runs[0].setup.class.id;
        let class_runs = experts.iter().filter(|l| l.setup.class.id == class).count();
        let at = median(
            runs.iter()
                .filter_map(|l| l.learned.iter().find(|(s, _)| s == k).map(|(_, t)| *t))
                .collect(),
        );
        let _ = writeln!(
            md,
            "| {k} | {} | {:.1}% | {at:.0}s | {:.1}% |",
            runs.len(),
            pct(runs.len(), class_runs),
            win(runs)
        );
    }

    // --- Flags -------------------------------------------------------------------------------------------
    let _ = writeln!(md, "\n## 需要关注\n");
    for f in &flags {
        let _ = writeln!(md, "- {f}");
    }
    for (class, tree, w, n, ..) in spec_rows(logs) {
        if n >= 20 && !(30.0..=85.0).contains(&w) {
            let _ = writeln!(
                md,
                "- 专精 {} {}：高手胜率 {w:.1}%，偏离 30–85%",
                policy::class_name(class),
                tree.map_or("（未专精）", |t| policy::tree_name(class, t))
            );
        }
    }
    let _ = std::fs::write(dir.join("report.md"), md);

    let mut csv = String::from("profile,class,race,map,tree,spec,won,time,level,kills,damage,taken,healed,bosses,elites,power_secs,peak_kps,close_calls,danger_secs,spikes,max_gap,death_burst,combos,capstones,talents,items\n");
    for l in logs {
        let s = &l.meter.s;
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{:.1},{},{},{:.0},{:.0},{:.0},{},{},{},{},{},{},{},{:.0},{:.2},{},{},{},{}",
            l.setup.profile.name,
            l.setup.class.name,
            l.setup.race,
            l.setup.map,
            l.setup.tree,
            l.spec.map_or(-1, i32::from),
            u8::from(l.won),
            l.time,
            l.level,
            l.kills,
            l.damage,
            l.tally_taken,
            l.tally_healed,
            l.bosses_killed,
            l.elites_killed,
            s.power_secs,
            s.peak_kps,
            s.close_calls,
            s.danger_secs,
            s.spikes,
            s.max_gap,
            s.death_burst,
            l.combos.join("|"),
            l.capstones.join("|"),
            l.talents.join("|"),
            l.items.join("|")
        );
    }
    let _ = std::fs::write(dir.join("runs.csv"), csv);
}

/// Per class and spec, the expert runs: (class, spec, win%, runs, median time, level, combos, capstones).
#[allow(clippy::type_complexity)]
fn spec_rows(logs: &[RunLog]) -> Vec<(u8, Option<u8>, f32, usize, f32, f32, f32, f32)> {
    let mut groups: HashMap<(u8, Option<u8>), Vec<&RunLog>> = HashMap::new();
    let any_expert = logs.iter().any(|l| l.setup.profile.name == "expert");
    for l in logs {
        if !any_expert || l.setup.profile.name == "expert" {
            groups.entry((l.setup.class.id, l.spec)).or_default().push(l);
        }
    }
    let mut out: Vec<_> = groups
        .into_iter()
        .map(|((c, t), runs)| {
            (
                c,
                t,
                win(&runs),
                runs.len(),
                median(runs.iter().map(|l| l.time).collect()),
                mean(&runs.iter().map(|l| l.level as f32).collect::<Vec<_>>()),
                mean(&runs.iter().map(|l| l.combos.len() as f32).collect::<Vec<_>>()),
                mean(&runs.iter().map(|l| l.capstones.len() as f32).collect::<Vec<_>>()),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

/// For each card (by family), over these runs: the runs that took it when offered, the runs
/// offered it that never took it, and the win-rate difference between them.
fn lifts_by(logs: &[&RunLog], key: impl Fn(&RunLog, &str) -> String) -> HashMap<String, (usize, usize, f32)> {
    let mut seen: HashMap<String, (HashSet<usize>, HashSet<usize>)> = HashMap::new();
    for (i, l) in logs.iter().enumerate() {
        for h in &l.hands {
            for (j, c) in h.cards.iter().enumerate() {
                let e = seen.entry(key(l, card_family(c))).or_default();
                if j == h.took {
                    e.0.insert(i);
                } else {
                    e.1.insert(i);
                }
            }
        }
    }
    seen.into_iter()
        .map(|(k, (took, offered))| {
            let passed: Vec<usize> = offered.difference(&took).copied().collect();
            let w = |set: &mut dyn Iterator<Item = usize>, n: usize| {
                pct(set.filter(|&i| logs[i].won).count(), n)
            };
            let lift = w(&mut took.iter().copied(), took.len()) - w(&mut passed.iter().copied(), passed.len());
            (k, (took.len(), passed.len(), lift))
        })
        .collect()
}

pub(crate) fn card_lifts(logs: &[&RunLog]) -> HashMap<String, (usize, usize, f32)> {
    lifts_by(logs, |_, c| c.to_string())
}

/// What each card is worth to each class, in win-rate points, where both sides have enough runs:
/// the expert's learned values.
pub(crate) fn card_values(logs: &[RunLog]) -> HashMap<String, f32> {
    let refs: Vec<&RunLog> = logs.iter().collect();
    lifts_by(&refs, |l, c| format!("{}:{c}", l.setup.class.id))
        .into_iter()
        .filter(|(_, (t, p, _))| *t >= 12 && *p >= 12)
        .map(|(k, (t, p, lift))| {
            // Shrunk toward zero on few runs.
            let n = (t.min(p)) as f32;
            (k, lift * n / (n + 20.0))
        })
        .collect()
}
