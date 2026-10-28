//! Warcraft Survivors: a top-down survival mode on the benilla world, started by
//! [`crate::run_survivors`]. There is no server: the hero, the enemies and the spells are local
//! entities dressed by the same attach, animation and spell-visual systems a streamed unit gets,
//! on a battleground picked at the menu ([`data::MAPS`]), with the player controller and FrameXML
//! off ([`crate::run_mode::CaptureMode`]).

mod abilities;
mod actives;
mod auto;
mod cards;
mod data;
mod enemies;
mod fill;
mod grade;
mod hero;
mod hud;
mod menu;
mod minimap;
mod mobility;
mod procs;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod sim;
mod synergy;
mod units;

use benilla_world::schedule::WorldStage;
use bevy::prelude::*;

use data::{AbilityDef, ClassDef, PassiveDef, Stat};
use synergy::{RelicDef, RuneDef, School, TalentDef};

/// Set by [`crate::run_survivors`] before the app builds. wasm32 has no process environment, so
/// the flag lives here; `WOW_SURVIVORS` (`?survivors=1` / `__wenilla_env.survivors` on the web)
/// still turns it on from outside.
pub(crate) static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
        || crate::webenv::var("WOW_SURVIVORS").is_some()
}

pub(crate) struct SurvivorsPlugin;

impl Plugin for SurvivorsPlugin {
    fn build(&self, app: &mut App) {
        // Present before `OnEnter(InWorld)`, where the UI lifecycle decides not to load FrameXML.
        app.insert_resource(crate::run_mode::CaptureMode)
            .add_plugins(grade::plugin)
            .init_state::<Phase>()
            .init_resource::<Rng>()
            .init_resource::<Selection>()
            .init_resource::<Run>()
            .init_resource::<hero::CamRig>()
            .init_resource::<hero::AimPoint>()
            .init_resource::<abilities::PendingHits>()
            .init_resource::<enemies::Spawner>()
            .init_resource::<hud::Offer>()
            .init_resource::<cards::PendingPick>()
            .add_message::<abilities::Hit>()
            .add_message::<enemies::Loot>()
            .add_plugins(bevy::ui_render::UiMaterialPlugin::<hud::SweepUiMaterial>::default())
            .add_plugins(bevy::ui_render::UiMaterialPlugin::<fill::FillMaterial>::default())
            .add_systems(Startup, hud::setup_hud)
            .add_systems(Update, hero::daylight)
            .add_systems(
                Update,
                (
                    hero::drop_stray_buttons,
                    hero::aim_cursor,
                    mobility::cast_mobility,
                    actives::cast_actives,
                    hero::drive_hero,
                    hero::place_camera,
                )
                    .chain()
                    .in_set(WorldStage::Input),
            )
            .add_systems(
                Update,
                (
                    enemies::spawn_enemies,
                    enemies::move_enemies,
                    abilities::cast_abilities,
                    procs::tick_build,
                    abilities::tick_areas,
                    abilities::apply_hits,
                    enemies::tick_marks,
                    enemies::drop_loot,
                    enemies::collect_gems,
                    hero::hero_vitals,
                )
                    .chain()
                    .after(WorldStage::Input)
                    .run_if(in_state(Phase::Playing))
                    // Entering a battleground raises its loading screen: the run waits behind it.
                    .run_if(|l: Res<crate::loading_screen::LoadingScreen>| !l.covering()),
            )
            .add_systems(
                Update,
                (enemies::reap_dead, enemies::still_gems).after(WorldStage::Input),
            )
            .add_systems(
                Update,
                (
                    hero::follow_map,
                    hud::menu_preview,
                    hud::buttons,
                    hud::keys,
                    hud::refresh_menu,
                    menu::restore_pick,
                    menu::build_menu,
                    menu::menu_clicks,
                    menu::refresh_menu_ui.before(crate::glue::GlueVisuals),
                    cards::deal_cards,
                    cards::choose_card,
                    cards::animate_cards,
                    cards::offer_text,
                    hud::refresh_hud,
                    hud::refresh_results,
                    hud::fill_icons,
                    hud::update_cooldown_sweeps,
                    hud::refresh_mobility,
                    hud::show_panels,
                )
                    .chain()
                    .after(WorldStage::Input),
            )
            .add_systems(
                Update,
                (
                    hud::apply_ui_scale,
                    hud::fill_art,
                    hud::xp_hover,
                    hud::icon_tooltip,
                    hud::hero_plate,
                    minimap::map_buttons,
                    minimap::zone_text,
                    minimap::compose,
                )
                    .chain()
                    .after(hud::show_panels),
            )
            .add_systems(OnEnter(Phase::Playing), unpause)
            .add_systems(OnEnter(Phase::LevelUp), (pause, hud::roll_offer))
            .add_systems(OnEnter(Phase::Paused), pause)
            .add_systems(OnEnter(Phase::GameOver), unpause)
            .add_systems(OnEnter(Phase::Menu), (unpause, reset_run));
        if let Some(auto) = auto::from_env() {
            app.insert_resource(auto)
                .add_systems(
                    Update,
                    auto::drive.after(hud::keys).before(hud::refresh_menu),
                )
                .add_systems(Update, auto::plan.before(WorldStage::Input));
        }
    }
}

/// The screen the mode is on.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) enum Phase {
    #[default]
    Menu,
    Playing,
    LevelUp,
    Paused,
    GameOver,
}

/// Everything spawned for a run, despawned when the next begins.
#[derive(Component)]
pub(super) struct RunEntity;

/// The class, look and battleground picked at the menu; the battleground holds for the run.
#[derive(Resource, Default)]
pub(super) struct Selection {
    pub class: Option<usize>,
    /// `(display, race, sex)`, one per race and sex the class's trainers come in.
    pub looks: Vec<(u32, u8, u8)>,
    pub look: Option<usize>,
    /// An index into [`data::MAPS`].
    pub map: usize,
    /// The battleground the world is standing in: the last one entered (or the saved pick at
    /// boot). Picking another at the menu changes only `map`, so the menu hero stays put and no
    /// loading screen rises until Enter World.
    pub stage: usize,
    /// Bumped on every change, so the menu rebuilds once.
    pub version: u32,
}

impl Selection {
    pub fn class_def(&self) -> Option<&'static ClassDef> {
        self.class.map(|i| &data::CLASSES[i])
    }

    pub fn map_def(&self) -> &'static data::MapDef {
        &data::MAPS[self.map.min(data::MAPS.len() - 1)]
    }

    pub fn stage_def(&self) -> &'static data::MapDef {
        &data::MAPS[self.stage.min(data::MAPS.len() - 1)]
    }
}

/// An ability the hero has, with its own cooldown.
pub(super) struct Owned {
    pub def: &'static AbilityDef,
    pub level: u8,
    pub cd: f32,
    /// Runes on it, in the order taken.
    pub runes: Vec<&'static RuneDef>,
    /// Casts so far, for the every-nth runes.
    pub casts: u32,
    /// Rampage: damage earned by kills, as a fraction.
    pub rampage: f32,
    /// A second cast due in this many seconds, and its damage factor.
    pub echo: Option<(f32, f32)>,
    /// Reprisal's own cooldown.
    pub reprisal_cd: f32,
    /// Ranks above its own it fights at (Atiesh).
    pub boost: u8,
}

impl Owned {
    pub fn new(def: &'static AbilityDef, cd: f32) -> Self {
        Self {
            def,
            level: 1,
            cd,
            runes: Vec::new(),
            casts: 0,
            rampage: 0.0,
            echo: None,
            reprisal_cd: 0.0,
            boost: 0,
        }
    }
    fn lvl(&self) -> u8 {
        self.level + self.boost
    }
    pub fn has_rune(&self, key: &str) -> bool {
        self.runes.iter().any(|r| r.key == key)
    }
    /// Its schools: its own, and every infusion's.
    pub fn schools(&self) -> u8 {
        self.runes
            .iter()
            .filter_map(|r| r.infuse)
            .fold(synergy::school_of(self.def.key), |m, s| m | s.bit())
    }
    pub fn damage_mult(&self) -> f32 {
        let mut m = 1.0 + 0.3 * f32::from(self.lvl() - 1);
        for r in &self.runes {
            m *= match r.key {
                "barrage" => 0.8,
                "widen" | "quickened" => 0.9,
                _ if r.infuse.is_some() => 1.1,
                _ => 1.0,
            };
        }
        m * (1.0 + self.rampage)
    }
    pub fn cooldown(&self, stats: &Stats) -> f32 {
        let mut base = self.def.cooldown * 0.93f32.powi(i32::from(self.lvl()) - 1);
        if self.has_rune("quickened") {
            base *= 0.75;
        }
        (base * (1.0 - stats.haste).max(0.35)).max(0.2)
    }
    pub fn count(&self) -> u32 {
        let extra = [3u8, 5, 7].iter().filter(|&&l| self.lvl() >= l).count() as u32;
        let barrage = if self.has_rune("barrage") { 2 } else { 0 };
        self.def.count + extra + barrage
    }
    pub fn radius(&self, stats: &Stats) -> f32 {
        let widen = if self.has_rune("widen") { 1.4 } else { 1.0 };
        self.def.radius * (1.0 + 0.08 * f32::from(self.lvl() - 1)) * stats.area * widen
    }
    pub fn duration(&self) -> f32 {
        let tether = if self.has_rune("tether") { 1.25 } else { 1.0 };
        self.def.duration * (1.0 + 0.1 * f32::from(self.lvl() - 1)) * tether
    }
    /// The hit flags its runes ask for.
    pub fn flags(&self) -> u16 {
        let mut f = synergy::F_PROC;
        for r in &self.runes {
            f |= match r.key {
                "split" => synergy::F_SPLIT,
                "contagion" => synergy::F_CONTAGION,
                "haunt" => synergy::F_HAUNT,
                "bloodthirst" => synergy::F_BLOODTHIRST,
                "executioner" => synergy::F_EXECUTIONER,
                "resonance" => synergy::F_DOUBLE_STATUS,
                _ => 0,
            };
        }
        f
    }
}

/// The hero's derived numbers, from the base and the passives.
#[derive(Clone, Copy)]
pub(super) struct Stats {
    pub might: f32,
    pub max_hp: f32,
    pub speed: f32,
    pub regen: f32,
    pub haste: f32,
    pub area: f32,
    pub magnet: f32,
    pub growth: f32,
    pub armor: f32,
    pub crit: f32,
    /// What a critical strike multiplies by.
    pub crit_mult: f32,
    /// The chance a blow against the hero misses.
    pub dodge: f32,
    /// What the hero takes, as a multiplier.
    pub taken: f32,
    /// How long the hero's crowd control lasts, as a multiplier.
    pub cc: f32,
}

pub(super) const BASE_HP: f32 = 120.0;
pub(super) const BASE_SPEED: f32 = 7.0;
pub(super) const BASE_MAGNET: f32 = 4.5;

/// One run's state.
#[derive(Resource)]
pub(super) struct Run {
    pub class: &'static ClassDef,
    pub race: u8,
    pub elapsed: f32,
    pub kills: u32,
    pub level: u32,
    pub xp: f32,
    pub hp: f32,
    pub abilities: Vec<Owned>,
    pub passives: Vec<(&'static PassiveDef, u8)>,
    pub talents: Vec<&'static TalentDef>,
    pub relics: Vec<&'static RelicDef>,
    /// Level-up tools left: deal three new cards, strike a card from the run, unlearn a spell.
    pub rerolls: u32,
    pub banishes: u32,
    pub forgets: u32,
    /// Keys struck from the run's offers.
    pub banished: Vec<&'static str>,
    /// Treasure waiting to be opened, `true` for a boss's.
    pub chests: Vec<bool>,
    pub b: synergy::BuildState,
    pub shield: f32,
    pub shield_t: f32,
    pub immune_t: f32,
    pub thorns: f32,
    pub thorns_t: f32,
    pub thorns_spell: u32,
    pub heal_rate: f32,
    pub heal_t: f32,
    /// The class mobility's cooldown and running effect.
    mobile: mobility::MobilityState,
    /// The specialization's burst and guard: cooldowns and running time.
    pub act: actives::ActState,
    pub pending_levels: u32,
    pub bosses: u32,
    pub rings: u32,
    pub won: bool,
    /// Seconds since the hero fell, while the death plays out.
    pub dead_t: Option<f32>,
    pub next_guid: u64,
    pub damage_done: f64,
    /// Bumped whenever abilities or passives change, so the HUD rebuilds the icon rows once.
    pub version: u32,
    /// A centre-screen announcement and its seconds left.
    pub banner: Option<(String, f32)>,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            class: &data::CLASSES[0],
            race: 0,
            elapsed: 0.0,
            kills: 0,
            level: 1,
            xp: 0.0,
            hp: BASE_HP,
            abilities: Vec::new(),
            passives: Vec::new(),
            talents: Vec::new(),
            relics: Vec::new(),
            rerolls: 2,
            banishes: 2,
            forgets: 1,
            banished: Vec::new(),
            chests: Vec::new(),
            b: synergy::BuildState::default(),
            shield: 0.0,
            shield_t: 0.0,
            immune_t: 0.0,
            thorns: 0.0,
            thorns_t: 0.0,
            thorns_spell: 0,
            heal_rate: 0.0,
            heal_t: 0.0,
            mobile: mobility::MobilityState::default(),
            act: actives::ActState::default(),
            pending_levels: 0,
            bosses: 0,
            rings: 0,
            won: false,
            dead_t: None,
            next_guid: 1,
            damage_done: 0.0,
            version: 0,
            banner: None,
        }
    }
}

impl Run {
    pub fn stats(&self) -> Stats {
        let mut s = Stats {
            might: 1.0,
            max_hp: BASE_HP,
            speed: BASE_SPEED,
            regen: 0.0,
            haste: 0.0,
            area: 1.0,
            magnet: BASE_MAGNET,
            growth: 1.0,
            armor: 0.0,
            crit: 0.05,
            crit_mult: 2.0,
            dodge: 0.0,
            taken: 1.0,
            cc: 1.0,
        };
        // Tanky classes start tougher, cloth starts quicker.
        match self.class.id {
            1 | 2 => s.max_hp += 40.0,
            7 | 11 | 3 | 4 => s.max_hp += 15.0,
            _ => s.speed += 0.5,
        }
        for &(p, level) in &self.passives {
            let v = p.per_level * f32::from(level);
            match p.stat {
                Stat::Might => s.might += v,
                Stat::MaxHp => s.max_hp *= 1.0 + v,
                Stat::Speed => s.speed *= 1.0 + v,
                Stat::Regen => s.regen += v,
                Stat::Haste => s.haste += v,
                Stat::Area => s.area += v,
                Stat::Magnet => s.magnet *= 1.0 + v,
                Stat::Growth => s.growth += v,
                Stat::Armor => s.armor += v,
                Stat::Crit => s.crit += v,
            }
        }
        // The race.
        match self.race {
            1 => s.growth += 0.1,
            3 => s.armor += 3.0,
            4 => {
                s.dodge += 0.10;
                s.speed *= 1.05;
            }
            6 => s.max_hp *= 1.15,
            7 => s.haste += 0.08,
            8 => s.regen += 0.6,
            _ => {}
        }
        // Talents.
        for t in &self.talents {
            match t.key {
                "critical_mass" => {
                    s.crit += 0.10;
                    s.crit_mult += 0.5;
                }
                "glass_cannon" => s.max_hp *= 0.65,
                "death_wish" => {
                    s.speed *= 1.15;
                    s.taken *= 1.25;
                }
                "hunger" => s.growth += 0.4,
                "hourglass" => s.cc *= 1.5,
                "impale" => s.crit_mult += 0.5,
                "flurry" if self.b.flurry_t > 0.0 => s.haste += 0.3,
                "toughness" => s.max_hp *= 1.25,
                "anticipation" => s.dodge += 0.10,
                "conviction" => s.crit += 0.08,
                "spiritual" => s.regen += 1.5,
                "spellblade" if self.b.arcane >= self.arcane_cap() => s.crit += 0.2,
                "zeal" => s.haste += 0.04 * f32::from(self.b.zeal),
                _ => {}
            }
        }
        // Relics.
        for r in &self.relics {
            match r.key {
                "onyxia" => {
                    s.crit += 0.08;
                    s.crit_mult += 0.5;
                }
                "drake_fang" => s.dodge += 0.12,
                "kiss_spider" => {
                    s.haste += 0.12;
                    s.speed *= 1.10;
                }
                "shroud" => {
                    s.magnet *= 2.0;
                    s.speed *= 1.08;
                }
                "dalrend_r" => s.haste += 0.10,
                "dalrend_l" => s.armor += 4.0,
                "neltharion" => s.area += 0.2,
                "ashbringer" => s.taken *= 1.2,
                "cthun" => {
                    s.growth += 0.5;
                    s.crit += 0.1;
                }
                _ => {}
            }
        }
        // What rides on health, once the maximum is known.
        let low = 1.0 - (self.hp / s.max_hp).clamp(0.0, 1.0);
        if self.race == 8 {
            s.haste += 0.3 * low;
        }
        if low > 0.5 && self.has_talent("blood_frenzy") {
            s.haste += 0.3;
        }
        // The class resource.
        match self.class.id {
            1 if self.b.surge_t > 0.0 => s.haste += 0.25,
            3 if self.b.surge_t > 0.0 => s.haste += 0.4,
            2 => s.haste += 0.03 * self.b.res,
            _ => {}
        }
        // The burst and the guard running.
        let a = self.act_buff();
        s.haste += a.haste;
        s.crit += a.crit;
        s.taken *= a.taken;
        s.dodge += a.dodge;
        s.regen += a.regen * s.max_hp;
        s.crit = s.crit.min(1.0);
        s.dodge = s.dodge.min(0.6);
        s
    }

    pub fn has_talent(&self, key: &str) -> bool {
        self.talents.iter().any(|t| t.key == key)
    }

    pub fn has_relic(&self, key: &str) -> bool {
        self.relics.iter().any(|r| r.key == key)
    }

    /// Spell slots taken: deep-talent spells and combinations ride free.
    pub fn slots_used(&self) -> usize {
        self.abilities
            .iter()
            .filter(|o| !synergy::bonus_spell(o.def.key))
            .count()
    }

    /// Talents taken in one of the class's talent trees.
    pub fn tree_points(&self, tree: u8) -> u8 {
        self.talents
            .iter()
            .filter(
                |t| matches!(t.req, synergy::Req::Tree(c, i, _) if c == self.class.id && i == tree),
            )
            .count() as u8
    }

    pub fn max_abilities(&self) -> usize {
        data::MAX_ABILITIES + usize::from(self.has_relic("hakkar"))
    }

    pub fn max_talents(&self) -> usize {
        synergy::MAX_TALENTS + usize::from(self.has_relic("hakkar"))
    }

    pub fn arcane_cap(&self) -> u8 {
        if self.has_talent("arcane_mind") {
            8
        } else {
            4
        }
    }

    /// Every school among the owned spells.
    pub fn schools(&self) -> u8 {
        self.abilities.iter().fold(0, |m, o| m | o.schools())
    }

    /// Enemy health, on top of the clock's scaling.
    pub fn enemy_hp_mult(&self) -> f32 {
        let mut m = 1.0;
        if self.has_talent("hunger") {
            m *= 1.15;
        }
        if self.has_relic("hakkar") {
            m *= 1.25;
        }
        m
    }

    pub fn enemy_dmg_mult(&self) -> f32 {
        if self.has_relic("cthun") {
            1.2
        } else {
            1.0
        }
    }

    /// The build's damage multiplier for a cast of these schools, before the target's statuses.
    pub fn power(&self, schools: u8) -> f32 {
        use synergy::has;
        let low = 1.0 - (self.hp / self.stats().max_hp).clamp(0.0, 1.0);
        let mut m = 1.0;
        for t in &self.talents {
            m *= match t.key {
                "glass_cannon" => 1.45,
                "death_wish" => 1.3,
                "blood_frenzy" if low > 0.5 => 1.2,
                "specialist" => {
                    1.0 + 0.15 * self.max_abilities().saturating_sub(self.slots_used()) as f32
                }
                "polymath" => 1.0 + 0.06 * self.schools().count_ones() as f32,
                "vengeance" => 1.0 + 0.02 * self.b.res,
                _ => 1.0,
            };
        }
        let mut bonus = 0.0f32;
        for r in &self.relics {
            match r.key {
                "ashbringer" => m *= 1.5,
                "binding_l" | "binding_r" if has(schools, School::Nature) => bonus += 0.15,
                "thunderfury" if has(schools, School::Nature) => bonus += 0.30,
                "eye_sulfuras" | "sulfuron" if has(schools, School::Fire) => bonus += 0.15,
                "sulfuras" if has(schools, School::Fire) => bonus += 0.30,
                "neltharion" if !has(schools, School::Physical) => bonus += 0.10,
                _ => {}
            }
        }
        if self.has_relic("dalrend_r")
            && self.has_relic("dalrend_l")
            && has(schools, School::Physical)
        {
            bonus += 0.25;
        }
        m *= 1.0 + bonus;
        // Timed surges.
        if self.b.ephemeral_t > 0.0 {
            m *= 1.4;
        }
        if self.b.charm_t > 0.0 {
            m *= 1.0 + 0.6 * (self.b.charm_t / 10.0);
        }
        if self.b.fury_t > 0.0 {
            m *= 1.3;
        }
        if self.race == 5 && has(schools, School::Shadow) {
            m *= 1.1;
        }
        // Arcane charges lift everything.
        let per = if self.has_talent("arcane_mind") {
            0.06
        } else {
            0.04
        };
        m *= 1.0 + per * f32::from(self.b.arcane);
        match self.class.id {
            1 => {
                m *= 1.0 + 0.003 * self.b.res;
                if self.b.surge_t > 0.0 {
                    m *= 1.15;
                }
            }
            11 => {
                let e = if self.has_talent("moonkin") { 0.7 } else { 0.4 };
                if self.b.lunar_t > 0.0 && has(schools, School::Arcane) {
                    m *= 1.0 + e;
                }
                if self.b.solar_t > 0.0 && has(schools, School::Nature) {
                    m *= 1.0 + e;
                }
            }
            _ => {}
        }
        m * self.act_buff().might
    }

    /// Damage over time's own multiplier: a warlock's soul shards, a running Shadowform.
    pub fn dot_power(&self) -> f32 {
        let shards = if self.class.id == 9 {
            1.0 + 0.05 * self.b.res
        } else {
            1.0
        };
        shards * self.act_buff().dot
    }

    /// Heal the hero through a per-second budget, so lifesteal stays a trickle against a horde.
    pub fn leech(&mut self, amount: f32) {
        let max = self.stats().max_hp;
        self.hp = (self.hp + amount.max(0.0)).min(max);
    }

    /// Heal on a kill. Uncapped: a build that heals through a horde has earned it.
    pub fn kill_heal(&mut self, amount: f32) {
        self.leech(amount);
    }

    pub fn guid(&mut self) -> u64 {
        self.next_guid += 1;
        self.next_guid
    }
}

/// xorshift64*: the mode's dice, seeded from the clock.
#[derive(Resource)]
pub(super) struct Rng(u64);

impl Default for Rng {
    fn default() -> Self {
        let seed = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(seed | 1)
    }
}

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    pub fn index(&mut self, len: usize) -> usize {
        (self.next_u64() % len.max(1) as u64) as usize
    }
}

fn pause(mut time: ResMut<Time<Virtual>>) {
    time.pause();
}

fn unpause(mut time: ResMut<Time<Virtual>>) {
    time.unpause();
}

/// Back at the menu: everything the last run spawned goes, and the run starts over.
fn reset_run(
    mut commands: Commands,
    spawned: Query<Entity, With<RunEntity>>,
    mut run: ResMut<Run>,
    mut pending: ResMut<abilities::PendingHits>,
    mut spawner: ResMut<enemies::Spawner>,
    mut selection: ResMut<Selection>,
) {
    for e in &spawned {
        commands.entity(e).despawn();
    }
    // Guids stay unique across runs: caches keyed by guid may outlive a despawn by a frame.
    let next_guid = run.next_guid;
    *run = Run {
        next_guid,
        ..Run::default()
    };
    pending.0.clear();
    *spawner = enemies::Spawner::default();
    // The preview hero is respawned for the look still picked.
    selection.version += 1;
}
