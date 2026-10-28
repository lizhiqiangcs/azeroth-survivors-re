//! The mode's screens, in bevy_ui since FrameXML is off: the class and hero pick, the in-run HUD,
//! the level-up cards, pause, and the results.

use std::collections::HashMap;

use benilla_ui::script::nameplate;
use benilla_world::view::WorldCamera;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::ui_render::ui_material::{MaterialNode, UiMaterial};
use bevy::window::PrimaryWindow;

use super::data::{self, AbilityDef, Effect, Kind, PassiveDef, Stat};
use super::hero::{self, Hero};
use super::i18n::{self, tf, w};
use super::minimap::{self, MiniMap};
use super::synergy::{self, ComboDef, Need, RelicDef, Req, RuneDef, TalentDef, Tier, TreeDef};
use super::{Owned, Phase, Rng, Run, Selection, Stats};

const GOLD: Color = Color::srgb(1.0, 0.82, 0.0);
/// The mobility button's edge, an action button's 36 units.
const MOBILITY_SIZE: f32 = 40.0;
const EDGE: Color = Color::srgb(0.55, 0.45, 0.25);
const BUTTON: Color = Color::srgba(0.18, 0.06, 0.04, 0.95);
const BUTTON_HOT: Color = Color::srgba(0.36, 0.12, 0.06, 0.98);
const DIM: Color = Color::srgb(0.6, 0.6, 0.6);
pub(super) const DIM_GREY: Color = DIM;
pub(super) const GOLD_TONE: Color = GOLD;

#[derive(Component)]
pub(super) struct MenuRoot;
#[derive(Component)]
pub(super) struct HudRoot;
#[derive(Component)]
pub(super) struct LevelUpRoot;
#[derive(Component)]
pub(super) struct PauseRoot;
#[derive(Component)]
pub(super) struct OverRoot;
#[derive(Component)]
pub(super) struct XpBar;
#[derive(Component)]
pub(super) struct XpFill;
#[derive(Component)]
pub(super) struct XpText;
#[derive(Component)]
pub(super) struct LevelText;
#[derive(Component)]
pub(super) struct TimerText;
#[derive(Component)]
pub(super) struct KillText;
#[derive(Component)]
pub(super) struct HpFrame;
#[derive(Component)]
pub(super) struct HpBar;
#[derive(Component)]
pub(super) struct HpLevelSeat;
#[derive(Component)]
pub(super) struct HpLevel;
#[derive(Component)]
pub(super) struct HpFill;
#[derive(Component)]
pub(super) struct ShieldFill;
#[derive(Component)]
pub(super) struct BannerText;
#[derive(Component)]
pub(super) struct AbilityRow;
#[derive(Component)]
pub(super) struct PassiveRow;
#[derive(Component)]
pub(super) struct OfferRow;
/// The line under LEVEL UP: which level this is.
#[derive(Component)]
pub(super) struct LevelUpSub;
/// The picker's title: LEVEL UP, TREASURE, FORGET.
#[derive(Component)]
pub(super) struct LevelUpTitle;
/// The line of tools under the cards.
#[derive(Component)]
pub(super) struct OfferHint;
/// The class resource, under the clock.
#[derive(Component)]
pub(super) struct ResourceText;

/// A radial cooldown sweep over one ability icon: `sweep` is the remaining fraction
/// (1.0 = just cast, 0.0 = ready), drawn by `ui_sweep.wgsl`.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(super) struct SweepUiMaterial {
    #[uniform(0)]
    pub(super) sweep: f32,
}

impl UiMaterial for SweepUiMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_app/shaders/ui_sweep.wgsl".into()
    }
}

/// The cooldown overlay on one ability slot, driven every frame.
#[derive(Component)]
pub(super) struct AbilitySweep {
    handle: Handle<SweepUiMaterial>,
    index: usize,
}

/// The class mobility's action button, bottom centre over the XP bar; shown once it is learned.
#[derive(Component)]
pub(super) struct MobilitySlot(u8);
#[derive(Component)]
pub(super) struct MobilityIcon(u8);
#[derive(Component)]
pub(super) struct MobilitySweep(Handle<SweepUiMaterial>, u8);
#[derive(Component)]
pub(super) struct OverTitle;
#[derive(Component)]
pub(super) struct OverStats;
#[derive(Component)]
pub(super) struct OverLine;
#[derive(Component)]
pub(super) struct OverStat(usize);

/// An icon slot filled from the spell's `SpellIcon` once the client data is up.
#[derive(Component)]
pub(super) struct SpellIcon(pub(super) u32);
#[derive(Component)]
pub(super) struct IconDone;

/// A FrameXML `Texture` region: a client art file, its `TexCoords` (left, right, top, bottom) and
/// vertex colour, drawn once the archives are readable.
#[derive(Component)]
pub(super) struct UiArt {
    path: &'static str,
    coords: [f32; 4],
    tint: Color,
}

impl UiArt {
    pub(super) fn crop(path: &'static str, coords: [f32; 4]) -> Self {
        Self {
            path,
            coords,
            tint: Color::WHITE,
        }
    }

    fn tinted(path: &'static str, tint: Color) -> Self {
        Self {
            path,
            coords: [0.0, 1.0, 0.0, 1.0],
            tint,
        }
    }
}

#[derive(Component)]
pub(super) struct ArtDone;

/// What an icon slot's tooltip describes: an index into the run's abilities or passives.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TipFor {
    Ability(usize),
    Passive(usize),
    /// The hand-cast buttons: 0 the burst (Q), 1 the guard (E), 2 the move (F).
    Action(u8),
    Talent(usize),
    Relic(usize),
    Race,
    Class,
}

/// The one `GameTooltip`, and the column its lines go in.
#[derive(Component)]
pub(super) struct GameTooltip;
#[derive(Component)]
pub(super) struct TipBody;

#[derive(Component, Clone, Copy, PartialEq)]
pub(super) enum UiAction {
    Resume,
    MainMenu,
    Quit,
}

/// The colour a button returns to when not hovered.
#[derive(Component)]
pub(super) struct BaseColor(Color);

#[derive(Clone, Copy)]
pub(super) enum Choice {
    NewAbility(&'static AbilityDef),
    Upgrade(usize),
    NewPassive(&'static PassiveDef),
    UpPassive(usize),
    /// A rune for an owned ability.
    Rune(usize, &'static RuneDef),
    Talent(&'static TalentDef),
    Relic(&'static RelicDef),
    /// Learn a talent tree's deep spell.
    Learn(&'static TreeDef),
    /// Join two mastered spells (or a spell and an item) into a new one.
    Combo(&'static ComboDef),
    /// Unlearn an owned ability.
    Forget(usize),
    Heal,
}

impl Choice {
    /// What banishing it strikes from the run, if it can be banished.
    pub(super) fn key(self) -> Option<&'static str> {
        match self {
            Choice::NewAbility(d) => Some(d.key),
            Choice::NewPassive(p) => Some(p.key),
            Choice::Rune(_, r) => Some(r.key),
            Choice::Talent(t) => Some(t.key),
            Choice::Relic(r) => Some(r.key),
            Choice::Learn(t) => Some(t.capstone),
            _ => None,
        }
    }

    /// Two offers are the same card.
    fn same(self, other: Choice) -> bool {
        match (self, other) {
            (Choice::Upgrade(a), Choice::Upgrade(b)) => a == b,
            (Choice::UpPassive(a), Choice::UpPassive(b)) => a == b,
            (Choice::Rune(a, r), Choice::Rune(b, q)) => a == b || r.key == q.key,
            (Choice::Forget(a), Choice::Forget(b)) => a == b,
            (Choice::Heal, Choice::Heal) => true,
            (Choice::Combo(a), Choice::Combo(b)) => a.key == b.key,
            (a, b) => a.key().is_some() && a.key() == b.key(),
        }
    }
}

/// What the cards on offer are for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum OfferMode {
    #[default]
    Level,
    /// Treasure: `true` for a boss's.
    Treasure(bool),
    /// Choosing a spell to unlearn.
    Forget,
}

/// The level-up cards on offer.
#[derive(Resource, Default)]
pub(super) struct Offer {
    choices: Vec<Choice>,
    pub(super) mode: OfferMode,
    /// The next card picked is banished instead.
    pub(super) banishing: bool,
    /// The level-up cards to return to when a forget is called off.
    back: Vec<Choice>,
}

impl Offer {
    pub(super) fn len(&self) -> usize {
        self.choices.len()
    }

    pub(super) fn choices(&self) -> &[Choice] {
        &self.choices
    }
}

#[derive(Resource)]
pub(super) struct HudFont(pub(super) Handle<Font>);

pub(super) fn rgb(hex: u32) -> Color {
    Color::srgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn text(font: &Handle<Font>, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont {
            font: font.clone(),
            font_size: size,
            ..default()
        },
        TextColor(color),
        TextShadow {
            offset: Vec2::splat(1.5),
            color: Color::BLACK,
        },
    )
}

pub(super) fn text_bundle(
    font: &Handle<Font>,
    s: impl Into<String>,
    size: f32,
    color: Color,
) -> impl Bundle {
    text(font, s, size, color)
}

pub(super) fn centered_bundle(
    font: &Handle<Font>,
    s: impl Into<String>,
    size: f32,
    color: Color,
) -> impl Bundle {
    centered(font, s, size, color)
}

fn centered(font: &Handle<Font>, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        text(font, s, size, color),
        TextLayout::new_with_justify(Justify::Center),
    )
}

fn button(action: UiAction, base: Color) -> impl Bundle {
    edged_button(action, base, EDGE)
}

fn edged_button(action: UiAction, base: Color, edge: Color) -> impl Bundle {
    (
        Button,
        action,
        BaseColor(base),
        BackgroundColor(base),
        BorderColor::all(edge),
    )
}

fn icon(spell: u32, size: f32) -> impl Bundle {
    (
        Node {
            width: Val::Px(size),
            height: Val::Px(size),
            ..default()
        },
        ImageNode {
            color: Color::srgba(0.0, 0.0, 0.0, 0.6),
            ..default()
        },
        SpellIcon(spell),
    )
}

fn spell_name(spells: Option<&crate::ui_action::Spells>, spell: u32, fallback: &str) -> String {
    if let Some(zh) = super::i18n::spell(fallback) {
        return zh.to_string();
    }
    spells
        .and_then(|s| s.catalog.get(spell))
        .map(|d| d.name.clone())
        .unwrap_or_else(|| fallback.replace('_', " "))
}

pub(super) fn setup_hud(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut sweeps_init: ResMut<Assets<SweepUiMaterial>>,
    mut fills: ResMut<Assets<super::fill::FillMaterial>>,
) {
    let font = crate::char_select::wow_font(&asset_server);
    let number_font: Handle<Font> = asset_server.load("mpq://Fonts/ARIALN.ttf");
    commands.insert_resource(HudFont(font.clone()));
    let f = &font;
    let map = MiniMap::new(&mut images);

    // --- The in-run HUD. ---
    commands
        .spawn((
            HudRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            GlobalZIndex(400),
            Visibility::Hidden,
        ))
        .with_children(|root| {
            // `MainMenuExpBar`, 1024×13 at the bottom centre. The reference stacks it on the
            // 40-unit action bar; with no action bar here it rests on the screen's edge.
            root.spawn((
                XpBar,
                Interaction::default(),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    bottom: Val::Px(0.0),
                    margin: UiRect::left(Val::Px(-512.0)),
                    width: Val::Px(1024.0),
                    height: Val::Px(13.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            ))
            .with_children(|bar| {
                bar.spawn((
                    XpFill,
                    Node {
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    ImageNode {
                        color: Color::NONE,
                        ..default()
                    },
                    UiArt::tinted(
                        "Interface\\TargetingFrame\\UI-StatusBar",
                        Color::srgb(0.58, 0.0, 0.55),
                    ),
                ));
                // `MainMenuXPBarTexture0`..`3`: the segment rules, 256×10 on the bar's top.
                for (i, top) in [0.792_968_75, 0.542_968_75, 0.292_968_75, 0.042_968_75]
                    .into_iter()
                    .enumerate()
                {
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(256.0 * i as f32),
                            top: Val::Px(0.0),
                            width: Val::Px(256.0),
                            height: Val::Px(10.0),
                            ..default()
                        },
                        ImageNode {
                            color: Color::NONE,
                            ..default()
                        },
                        UiArt::crop(
                            "Interface\\MainMenuBar\\UI-MainMenuBar-Dwarf",
                            [0.0, 1.0, top, top + 0.039_062_5],
                        ),
                    ));
                }
                // `MainMenuBarExpText`, `TextStatusBarText` centred one unit up, shown on hover.
                bar.spawn(Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    top: Val::Px(-1.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|n| {
                    n.spawn((
                        XpText,
                        Text::new(""),
                        TextFont {
                            font: number_font.clone(),
                            font_size: 14.0,
                            ..default()
                        },
                        TextColor(Color::WHITE),
                        TextShadow {
                            offset: Vec2::new(1.0, 1.0),
                            color: Color::BLACK,
                        },
                        TextLayout::new(Justify::Center, LineBreak::NoWrap),
                        Visibility::Hidden,
                    ));
                });
            });
            minimap::spawn_cluster(root, f, &map);
            // The hand-cast buttons, Q E F: 36-unit action slots in `UI-Quickslot2`, the key over
            // each.
            for (i, key) in ["Q", "E", "F"].into_iter().enumerate() {
            let i = i as u8;
            let offset = (f32::from(i) - 1.0) * (MOBILITY_SIZE + 16.0) - MOBILITY_SIZE / 2.0;
            root.spawn((
                MobilitySlot(i),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    bottom: Val::Px(24.0),
                    margin: UiRect::left(Val::Px(offset)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(2.0),
                    display: Display::None,
                    ..default()
                },
            ))
            .with_children(|col| {
                col.spawn(centered(f, key, 18.0, Color::WHITE));
                col.spawn((
                    Node {
                        width: Val::Px(MOBILITY_SIZE),
                        height: Val::Px(MOBILITY_SIZE),
                        ..default()
                    },
                    Interaction::default(),
                    TipFor::Action(i),
                ))
                .with_children(|slot| {
                    // `UI-Quickslot2` is the empty slot's 64×64 frame, the button 36 inside it.
                    slot.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(-MOBILITY_SIZE * 0.42),
                            top: Val::Px(-MOBILITY_SIZE * 0.42),
                            width: Val::Px(MOBILITY_SIZE * 1.84),
                            height: Val::Px(MOBILITY_SIZE * 1.84),
                            ..default()
                        },
                        ImageNode {
                            color: Color::NONE,
                            ..default()
                        },
                        UiArt::crop("Interface\\Buttons\\UI-Quickslot2", [0.0, 1.0, 0.0, 1.0]),
                    ));
                    slot.spawn((
                        MobilityIcon(i),
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        ImageNode {
                            color: Color::srgba(0.0, 0.0, 0.0, 0.6),
                            ..default()
                        },
                    ));
                    let handle = sweeps_init.add(SweepUiMaterial { sweep: 0.0 });
                    slot.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        MaterialNode(handle.clone()),
                        MobilitySweep(handle, i),
                    ));
                });
            });
            }
            root.spawn(Node {
                width: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(10.0)),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::FlexStart,
                ..default()
            })
            .with_children(|row| {
                row.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(6.0),
                    width: Val::Px(300.0),
                    ..default()
                })
                .with_children(|col| {
                    col.spawn((
                        AbilityRow,
                        Node {
                            column_gap: Val::Px(4.0),
                            ..default()
                        },
                    ));
                    // Traits, passives, talents and relics: wraps onto more lines as the build grows.
                    col.spawn((
                        PassiveRow,
                        Node {
                            column_gap: Val::Px(4.0),
                            row_gap: Val::Px(4.0),
                            flex_wrap: FlexWrap::Wrap,
                            width: Val::Percent(100.0),
                            ..default()
                        },
                    ));
                });
                // The clock, with the level and the kill count on one line under it.
                row.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(0.0),
                    ..default()
                })
                .with_children(|col| {
                    col.spawn((TimerText, centered(f, "00:00", 34.0, Color::WHITE)));
                    col.spawn(Node {
                        column_gap: Val::Px(14.0),
                        ..default()
                    })
                    .with_children(|line| {
                        line.spawn((LevelText, text(f, "Level 1", 14.0, GOLD)));
                        line.spawn((KillText, text(f, "Kills 0", 14.0, Color::WHITE)));
                    });
                    col.spawn((
                        ResourceText,
                        centered(f, "", 13.0, Color::srgb(1.0, 0.85, 0.5)),
                    ));
                });
                // The minimap cluster's column.
                row.spawn(Node {
                    width: Val::Px(300.0),
                    ..default()
                });
            });
            root.spawn(Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(22.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            })
            .with_children(|n| {
                n.spawn((BannerText, centered(f, "", 30.0, GOLD)));
            });
            // The hero's health as a nameplate (`CGNamePlateFrame`): the bar under the border that
            // caps it, the level in the border's seat; [`hero_plate`] sizes and seats it.
            let absolute = || Node {
                position_type: PositionType::Absolute,
                ..default()
            };
            let unpainted = || ImageNode {
                color: Color::NONE,
                ..default()
            };
            let [r, g, b, _] = crate::vplates::PLATE_FRIENDLY;
            root.spawn((HpFrame, absolute(), Visibility::Hidden))
                .with_children(|plate| {
                    plate.spawn((HpBar, absolute())).with_children(|bar| {
                        bar.spawn((
                            HpFill,
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            unpainted(),
                            UiArt::tinted(nameplate::BAR_FILL_TEXTURE, Color::srgb(r, g, b)),
                        ));
                        bar.spawn((
                            ShieldFill,
                            Node {
                                width: Val::Percent(0.0),
                                height: Val::Percent(100.0),
                                ..absolute()
                            },
                            unpainted(),
                            UiArt::tinted(
                                nameplate::BAR_FILL_TEXTURE,
                                Color::srgba(0.85, 0.9, 1.0, 0.75),
                            ),
                        ));
                    });
                    plate.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..absolute()
                        },
                        unpainted(),
                        UiArt::crop(nameplate::BORDER_TEXTURE, [0.0, 1.0, 0.0, 1.0]),
                    ));
                    let [r, g, b, _] = crate::vplates::CON_YELLOW;
                    plate
                        .spawn((
                            HpLevelSeat,
                            Node {
                                width: Val::Px(0.0),
                                height: Val::Px(0.0),
                                justify_content: JustifyContent::Center,
                                align_items: AlignItems::Center,
                                overflow: Overflow::visible(),
                                ..absolute()
                            },
                        ))
                        .with_children(|seat| {
                            seat.spawn((
                                HpLevel,
                                Text::new("1"),
                                TextFont {
                                    font: f.clone(),
                                    font_size: 11.0,
                                    ..default()
                                },
                                TextColor(Color::srgb(r, g, b)),
                                TextShadow {
                                    offset: Vec2::new(1.0, 1.0),
                                    color: Color::BLACK,
                                },
                                TextLayout::new(Justify::Center, LineBreak::NoWrap),
                            ));
                        });
                });
        });
    commands.insert_resource(map);

    // --- The run's pick screen; [`super::menu`] builds into it once the glue art is in. ---
    commands.spawn((
        MenuRoot,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        GlobalZIndex(410),
        bevy::ui::FocusPolicy::Pass,
    ));

    // --- Level up. ---
    commands
        .spawn((
            LevelUpRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(18.0),
                ..default()
            },
            // A dark vignette closing in on the cards, a slow golden light at its heart (the
            // 1400-unit burst's radius over the 768-unit canvas).
            MaterialNode(
                fills.add(
                    super::fill::FillMaterial::radial(
                        Color::srgba(0.04, 0.025, 0.0, 0.72),
                        Color::srgba(0.0, 0.0, 0.0, 0.9),
                    )
                    .with_glow(
                        Color::srgba(1.0, 0.75, 0.25, 0.22),
                        Vec2::splat(0.5),
                        Vec2::splat(700.0 / 768.0),
                    ),
                ),
            ),
            GlobalZIndex(420),
            Visibility::Hidden,
            // The build's icons stay hoverable under the cards.
            bevy::ui::FocusPolicy::Pass,
        ))
        .with_children(|root| {
            // The slow golden burst behind everything.
            root.spawn((
                super::cards::Rays,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(1400.0),
                    height: Val::Px(1400.0),
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    margin: UiRect::new(
                        Val::Px(-700.0),
                        Val::Px(0.0),
                        Val::Px(-700.0),
                        Val::Px(0.0),
                    ),
                    ..default()
                },
                BackgroundGradient(vec![Gradient::Conic(ConicGradient::new(
                    UiPosition::CENTER,
                    (0..24)
                        .map(|i| {
                            let a = i as f32 / 24.0 * std::f32::consts::TAU;
                            let on = i % 2 == 0;
                            AngularColorStop::new(
                                if on {
                                    Color::srgba(1.0, 0.8, 0.3, 0.07)
                                } else {
                                    Color::srgba(1.0, 0.8, 0.3, 0.0)
                                },
                                a,
                            )
                        })
                        .collect(),
                ))]),
                UiTransform::default(),
                bevy::ui::FocusPolicy::Pass,
            ));
            root.spawn((LevelUpTitle, centered(f, "LEVEL UP", 52.0, GOLD)));
            root.spawn((
                LevelUpSub,
                centered(f, "Choose your power", 16.0, Color::srgb(0.85, 0.78, 0.6)),
            ));
            root.spawn((
                OfferRow,
                Node {
                    column_gap: Val::Px(22.0),
                    // Room for a hovered card's lift.
                    margin: UiRect::new(Val::ZERO, Val::ZERO, Val::Px(28.0), Val::Px(16.0)),
                    ..default()
                },
            ));
            root.spawn((
                OfferHint,
                centered(f, "Click a card, or press 1, 2 or 3", 14.0, DIM),
            ));
        });

    // --- Pause. ---
    commands
        .spawn((
            PauseRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(12.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(430),
            Visibility::Hidden,
            bevy::ui::FocusPolicy::Pass,
        ))
        .with_children(|root| {
            root.spawn(centered(f, "Paused", 46.0, GOLD));
            for (label, action) in [
                ("Resume", UiAction::Resume),
                ("Abandon run", UiAction::MainMenu),
                ("Quit game", UiAction::Quit),
            ] {
                root.spawn((
                    button(action, BUTTON),
                    Node {
                        width: Val::Px(240.0),
                        justify_content: JustifyContent::Center,
                        padding: UiRect::axes(Val::Px(20.0), Val::Px(8.0)),
                        border: UiRect::all(Val::Px(2.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                ))
                .with_children(|b| {
                    b.spawn(text(f, label, 20.0, Color::WHITE));
                });
            }
        });

    // --- Results. ---
    commands
        .spawn((
            OverRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
            MaterialNode(fills.add(super::fill::FillMaterial::radial(
                Color::srgba(0.07, 0.02, 0.0, 0.8),
                Color::srgba(0.0, 0.0, 0.0, 0.94),
            ))),
            GlobalZIndex(440),
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((OverTitle, centered(f, "", 64.0, GOLD)));
            root.spawn((
                OverLine,
                centered(f, "", 18.0, Color::srgb(0.85, 0.78, 0.6)),
            ));
            // The run in numbers, one tile each.
            root.spawn(Node {
                column_gap: Val::Px(12.0),
                margin: UiRect::vertical(Val::Px(14.0)),
                ..default()
            })
            .with_children(|row| {
                for (i, label) in ["Survived", "Level", "Kills", "Damage", "Bosses"]
                    .into_iter()
                    .enumerate()
                {
                    row.spawn((
                        Node {
                            width: Val::Px(128.0),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            padding: UiRect::axes(Val::Px(10.0), Val::Px(12.0)),
                            row_gap: Val::Px(4.0),
                            border: UiRect::all(Val::Px(1.0)),
                            border_radius: BorderRadius::all(Val::Px(8.0)),
                            ..default()
                        },
                        BackgroundGradient::from(LinearGradient::to_bottom(vec![
                            ColorStop::percent(Color::srgba(0.2, 0.14, 0.05, 0.92), 0.0),
                            ColorStop::percent(Color::srgba(0.04, 0.03, 0.02, 0.94), 100.0),
                        ])),
                        BorderColor::all(EDGE),
                    ))
                    .with_children(|tile| {
                        tile.spawn((OverStat(i), centered(f, "", 26.0, Color::WHITE)));
                        tile.spawn(centered(f, label, 12.0, DIM));
                    });
                }
            });
            root.spawn((OverStats, centered(f, "", 15.0, DIM)));
            root.spawn((
                edged_button(UiAction::MainMenu, BUTTON, GOLD),
                Node {
                    width: Val::Px(260.0),
                    justify_content: JustifyContent::Center,
                    padding: UiRect::axes(Val::Px(20.0), Val::Px(11.0)),
                    margin: UiRect::top(Val::Px(8.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                BoxShadow::new(
                    Color::srgba(1.0, 0.7, 0.2, 0.35),
                    Val::Px(0.0),
                    Val::Px(0.0),
                    Val::Px(0.0),
                    Val::Px(16.0),
                ),
            ))
            .with_children(|b| {
                b.spawn(text(f, "Play again", 22.0, GOLD));
            });
            root.spawn((
                button(UiAction::Quit, Color::NONE),
                Node {
                    justify_content: JustifyContent::Center,
                    padding: UiRect::axes(Val::Px(16.0), Val::Px(6.0)),
                    ..default()
                },
            ))
            .with_children(|b| {
                b.spawn(text(f, "Quit game", 15.0, DIM));
            });
        });
}

/// The menu's hero is the run's hero: respawned whenever the pick changes.
pub(super) fn menu_preview(
    mut commands: Commands,
    phase: Res<State<Phase>>,
    selection: Res<Selection>,
    mut run: ResMut<Run>,
    heroes: Query<Entity, With<Hero>>,
    world: benilla_world::world_point::WorldPoint,
    mut shown: Local<Option<(usize, u32, usize)>>,
) {
    if *phase.get() != Phase::Menu {
        return;
    }
    let want = selection
        .class
        .zip(selection.look.and_then(|i| selection.looks.get(i)).copied());
    let key = want.map(|(c, l)| (c, l.0, selection.stage));
    if key == *shown && (key.is_none() || !heroes.is_empty()) {
        return;
    }
    for e in &heroes {
        commands.entity(e).despawn();
    }
    *shown = key;
    let Some((class, (display, race, sex))) = want else {
        return;
    };
    let mut pos = hero::arena(selection.stage_def());
    if let Some(h) = world.terrain_height_under(pos) {
        pos.y = h;
    }
    let serial = run.guid();
    hero::spawn_hero(
        &mut commands,
        &data::CLASSES[class],
        display,
        race,
        sex,
        serial,
        pos,
    );
}

pub(super) fn start_run(run: &mut Run, selection: &mut Selection) -> bool {
    let (Some(class), Some(&(_, race, _))) = (
        selection.class_def(),
        selection.look.and_then(|i| selection.looks.get(i)),
    ) else {
        return false;
    };
    // The world goes to the battleground picked; until now it stood in the last one.
    selection.stage = selection.map;
    let next_guid = run.next_guid;
    *run = Run {
        class,
        race,
        next_guid,
        ..Run::default()
    };
    run.abilities
        .push(Owned::new(data::ability(class.start), 0.5));
    // Humans talk their way into more choices.
    if race == 1 {
        run.rerolls += 1;
        run.banishes += 1;
    }
    run.hp = run.stats().max_hp;
    run.version += 1;
    true
}

/// Draw up to `n` distinct choices from a weighted pool.
fn draw(mut pool: Vec<(Choice, f32)>, rng: &mut Rng, n: usize) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::with_capacity(n);
    while out.len() < n && !pool.is_empty() {
        let total: f32 = pool.iter().map(|(_, w)| w).sum();
        let mut pick = rng.f32() * total;
        let mut idx = pool.len() - 1;
        for (i, (_, w)) in pool.iter().enumerate() {
            if pick < *w {
                idx = i;
                break;
            }
            pick -= w;
        }
        let c = pool.swap_remove(idx).0;
        if !out.iter().any(|o| o.same(c)) {
            out.push(c);
        }
    }
    out
}

/// Whether a talent's requirement is met by the build.
pub(super) fn talent_open(t: &TalentDef, run: &Run) -> bool {
    let schools = run.schools();
    match t.req {
        // A general talent that one of the class's trees holds is taken there instead.
        Req::None => !synergy::TALENTS
            .iter()
            .any(|x| x.key == t.key && matches!(x.req, Req::Tree(c, ..) if c == run.class.id)),
        Req::Duo(a, b) => synergy::has(schools, a) && synergy::has(schools, b),
        Req::Tree(c, tree, tier) => run.class.id == c && run.tree_points(tree) >= tier,
    }
}

/// Whether the build has taken any talent-tree specialization yet.
fn specialized(run: &Run) -> bool {
    run.talents.iter().any(|t| matches!(t.req, Req::Tree(..)))
}

/// Trees whose two talents are taken and whose deep spell is not yet learned.
fn learnable(run: &Run) -> Vec<&'static TreeDef> {
    synergy::trees(run.class.id)
        .filter(|(i, t)| {
            run.tree_points(*i) >= 2 && !run.abilities.iter().any(|o| o.def.key == t.capstone)
        })
        .map(|(_, t)| t)
        .collect()
}

/// Whether a combination's needs are all met, and its spell not yet learned.
pub(super) fn combo_ready(c: &ComboDef, run: &Run) -> bool {
    if run.abilities.iter().any(|o| o.def.key == c.key) || run.banished.contains(&c.key) {
        return false;
    }
    let max = |o: &Owned| o.level >= data::MAX_ABILITY_LEVEL;
    c.needs.iter().all(|n| match *n {
        Need::Max(k) => run.abilities.iter().any(|o| o.def.key == k && max(o)),
        Need::MaxSchool(s) => run
            .abilities
            .iter()
            .any(|o| max(o) && synergy::has(o.schools(), s) && !synergy::bonus_spell(o.def.key)),
        Need::Relic(k) => run.has_relic(k),
    })
}

fn level_pool(run: &Run, rng: &mut Rng) -> Vec<(Choice, f32)> {
    let banned = |k: &str| run.banished.contains(&k);
    let mut pool: Vec<(Choice, f32)> = Vec::new();
    for (i, o) in run.abilities.iter().enumerate() {
        if o.level < data::MAX_ABILITY_LEVEL {
            pool.push((Choice::Upgrade(i), 1.2));
        }
        // An open rune slot offers one rune that fits.
        if o.runes.len() < synergy::rune_slots(o.level) {
            let fits: Vec<&'static RuneDef> = synergy::RUNES
                .iter()
                .filter(|r| synergy::rune_fits(r, o.def.kind, o.schools()))
                .filter(|r| !o.has_rune(r.key) && !banned(r.key))
                .collect();
            if !fits.is_empty() {
                pool.push((Choice::Rune(i, fits[rng.index(fits.len())]), 1.4));
            }
        }
    }
    if run.slots_used() < run.max_abilities() {
        for key in run.class.pool {
            if !run.abilities.iter().any(|o| o.def.key == *key) && !banned(key) {
                pool.push((Choice::NewAbility(data::ability(key)), 1.0));
            }
        }
    }
    for (i, (_, level)) in run.passives.iter().enumerate() {
        if *level < data::MAX_PASSIVE_LEVEL {
            pool.push((Choice::UpPassive(i), 0.8));
        }
    }
    if run.passives.len() < data::MAX_PASSIVES {
        for p in data::PASSIVES {
            if !run.passives.iter().any(|(q, _)| q.key == p.key) && !banned(p.key) {
                pool.push((Choice::NewPassive(p), 0.5));
            }
        }
    }
    if run.level >= 3 && run.talents.len() < run.max_talents() {
        for t in synergy::TALENTS {
            if run.has_talent(t.key) || banned(t.key) || !talent_open(t, run) {
                continue;
            }
            let w = match t.req {
                Req::Duo(..) => 0.8,
                Req::Tree(..) => 0.6,
                Req::None if t.cursed => 0.18,
                Req::None => 0.25,
            };
            pool.push((Choice::Talent(t), w));
        }
    }
    // A tree's deep spell, once both its talents are taken.
    for t in learnable(run) {
        if !banned(t.capstone) {
            pool.push((Choice::Learn(t), 2.5));
        }
    }
    pool
}

fn level_choices(run: &Run, rng: &mut Rng) -> Vec<Choice> {
    // The first level-up chooses the specialization: one card per talent tree.
    if !specialized(run) {
        let specs: Vec<Choice> = synergy::TALENTS
            .iter()
            .filter(|t| matches!(t.req, Req::Tree(c, _, 0) if c == run.class.id))
            .filter(|t| !run.banished.contains(&t.key))
            .map(Choice::Talent)
            .collect();
        if !specs.is_empty() {
            return specs;
        }
    }
    let pool = level_pool(run, rng);
    let mut out = draw(pool, rng, 3);
    // A ready combination always shows, first.
    if let Some(c) = synergy::COMBOS.iter().find(|c| combo_ready(c, run)) {
        out.truncate(2);
        out.insert(0, Choice::Combo(c));
    }
    if out.is_empty() {
        out.push(Choice::Heal);
    }
    out
}

/// Whether the cards on offer are the specialization choice.
pub(super) fn spec_offer(run: &Run, offer: &Offer) -> bool {
    offer.mode == OfferMode::Level
        && !specialized(run)
        && offer
            .choices
            .iter()
            .all(|c| matches!(c, Choice::Talent(t) if matches!(t.req, Req::Tree(..))))
}

fn treasure_pool(run: &Run, boss: bool) -> Vec<(Choice, f32)> {
    // A piece whose forged whole is already carried never drops again.
    let forged_away = |key: &str| {
        synergy::FORGES
            .iter()
            .any(|(a, b, c)| (key == *a || key == *b) && run.has_relic(c))
    };
    synergy::RELICS
        .iter()
        .filter(|r| !r.forged && (boss || !r.boss))
        .filter(|r| !run.has_relic(r.key) && !run.banished.contains(&r.key))
        .filter(|r| !forged_away(r.key))
        .map(|r| {
            let w = match (boss, r.tier) {
                (true, _) if r.boss => 2.0,
                (true, Tier::Legendary | Tier::Epic) => 1.0,
                (true, Tier::Rare) => 0.5,
                (false, Tier::Rare) => 1.0,
                (false, _) => 0.5,
            };
            (Choice::Relic(r), w)
        })
        .collect()
}

fn treasure_choices(run: &Run, rng: &mut Rng, boss: bool) -> Vec<Choice> {
    let mut out = draw(treasure_pool(run, boss), rng, 3);
    if out.is_empty() {
        out.push(Choice::Heal);
    }
    out
}

/// The next offer: treasure first, then the level-ups.
pub(super) fn next_offer(run: &Run, rng: &mut Rng, offer: &mut Offer) {
    offer.banishing = false;
    offer.back.clear();
    if let Some(&boss) = run.chests.first() {
        offer.mode = OfferMode::Treasure(boss);
        offer.choices = treasure_choices(run, rng, boss);
    } else {
        offer.mode = OfferMode::Level;
        offer.choices = level_choices(run, rng);
    }
}

pub(super) fn roll_offer(run: Res<Run>, mut rng: ResMut<Rng>, mut offer: ResMut<Offer>) {
    next_offer(&run, &mut rng, &mut offer);
}

/// R: three fresh cards.
pub(super) fn reroll(run: &mut Run, offer: &mut Offer, rng: &mut Rng) -> bool {
    if run.rerolls == 0 || offer.mode == OfferMode::Forget || spec_offer(run, offer) {
        return false;
    }
    run.rerolls -= 1;
    offer.banishing = false;
    offer.choices = match offer.mode {
        OfferMode::Treasure(boss) => treasure_choices(run, rng, boss),
        _ => level_choices(run, rng),
    };
    true
}

/// B: the next card picked is struck from the run instead.
pub(super) fn toggle_banish(run: &Run, offer: &mut Offer) -> bool {
    if offer.mode == OfferMode::Forget
        || spec_offer(run, offer)
        || (run.banishes == 0 && !offer.banishing)
    {
        return false;
    }
    offer.banishing = !offer.banishing;
    true
}

/// Strike card `i` from the run, and deal another in its place.
pub(super) fn banish(i: usize, run: &mut Run, offer: &mut Offer, rng: &mut Rng) -> bool {
    let Some(key) = offer.choices.get(i).and_then(|c| c.key()) else {
        return false;
    };
    run.banished.push(key);
    run.banishes -= 1;
    offer.banishing = false;
    let pool = match offer.mode {
        OfferMode::Treasure(boss) => treasure_pool(run, boss),
        _ => level_pool(run, rng),
    };
    let others: Vec<Choice> = offer
        .choices
        .iter()
        .enumerate()
        .filter(|&(j, _)| j != i)
        .map(|(_, c)| *c)
        .collect();
    let pool: Vec<(Choice, f32)> = pool
        .into_iter()
        .filter(|(c, _)| !others.iter().any(|o| o.same(*c)))
        .collect();
    match draw(pool, rng, 1).first() {
        Some(&c) => offer.choices[i] = c,
        None => {
            offer.choices.remove(i);
            if offer.choices.is_empty() {
                offer.choices.push(Choice::Heal);
            }
        }
    }
    true
}

/// X: choose a spell to unlearn, or go back to the level-up.
pub(super) fn toggle_forget(run: &Run, offer: &mut Offer) -> bool {
    match offer.mode {
        OfferMode::Forget => {
            offer.mode = OfferMode::Level;
            offer.choices = std::mem::take(&mut offer.back);
            true
        }
        OfferMode::Level if run.forgets > 0 && run.abilities.len() > 1 => {
            offer.banishing = false;
            offer.back = std::mem::take(&mut offer.choices);
            offer.mode = OfferMode::Forget;
            offer.choices = (0..run.abilities.len()).map(Choice::Forget).collect();
            true
        }
        _ => false,
    }
}

/// What forgetting an ability gives back: a pick for every rank past the first, and every rune.
pub(super) fn refund(o: &Owned) -> u32 {
    u32::from(o.level - 1) + o.runes.len() as u32
}

/// Take a relic, and forge any pair it completes.
fn take_relic(run: &mut Run, r: &'static RelicDef) {
    run.relics.push(r);
    match r.key {
        "atiesh" => {
            for o in &mut run.abilities {
                o.boost = 1;
            }
        }
        "hourglass" => {
            run.rerolls += 2;
            run.banishes += 2;
            run.forgets += 2;
        }
        _ => {}
    }
    for (a, b, c) in synergy::FORGES {
        if run.has_relic(a) && run.has_relic(b) {
            run.relics.retain(|x| x.key != *a && x.key != *b);
            let forged = synergy::relic_def(c);
            run.relics.push(forged);
            run.banner = Some((format!("Forged {}!", forged.name), 4.0));
        }
    }
}

/// A specialization's first talent turns the class's starting spell into the tree's own,
/// keeping its rank and whatever enchants still fit.
fn specialize(run: &mut Run, class: u8, tree: u8) {
    let Some(def) = synergy::tree_def(class, tree) else {
        return;
    };
    run.banner = Some(match run.kit() {
        Some(k) => (
            format!(
                "{}: Q {}, E {}, F {}",
                def.name, k.burst.name, k.guard.name, k.mobility.name
            ),
            5.0,
        ),
        None => (format!("{} specialization", def.name), 3.0),
    });
    let Some(to) = def.swap else { return };
    if run.abilities.iter().any(|o| o.def.key == to) {
        return;
    }
    let start = run.class.start;
    if let Some(o) = run.abilities.iter_mut().find(|o| o.def.key == start) {
        o.def = data::ability(to);
        let (kind, schools) = (o.def.kind, o.schools());
        o.runes.retain(|r| synergy::rune_fits(r, kind, schools));
    }
}

pub(super) fn pick(
    i: usize,
    run: &mut Run,
    offer: &mut Offer,
    rng: &mut Rng,
    next: &mut NextState<Phase>,
) {
    let Some(&choice) = offer.choices.get(i) else {
        return;
    };
    let mode = offer.mode;
    match choice {
        Choice::NewAbility(def) => {
            let boost = u8::from(run.has_relic("atiesh"));
            run.abilities.push(Owned {
                boost,
                ..Owned::new(def, 0.3)
            });
        }
        Choice::Upgrade(k) => run.abilities[k].level += 1,
        Choice::NewPassive(p) => run.passives.push((p, 1)),
        Choice::UpPassive(k) => run.passives[k].1 += 1,
        Choice::Rune(k, r) => run.abilities[k].runes.push(r),
        Choice::Talent(t) => {
            run.talents.push(t);
            if let Req::Tree(c, tree, 0) = t.req {
                specialize(run, c, tree);
            }
        }
        Choice::Relic(r) => take_relic(run, r),
        Choice::Learn(t) => {
            let boost = u8::from(run.has_relic("atiesh"));
            run.abilities.push(Owned {
                boost,
                ..Owned::new(data::ability(t.capstone), 0.3)
            });
            run.banner = Some((format!("{} mastery: a new spell!", t.name), 3.0));
        }
        Choice::Combo(c) => {
            let boost = u8::from(run.has_relic("atiesh"));
            run.abilities.push(Owned {
                boost,
                ..Owned::new(data::ability(c.key), 0.3)
            });
            run.banner = Some(("Combination unlocked!".into(), 3.5));
        }
        Choice::Forget(k) => {
            let gone = run.abilities.remove(k);
            run.pending_levels += refund(&gone);
            run.forgets -= 1;
            run.banner = Some(("Unlearned. Your training returns to you.".into(), 2.5));
        }
        Choice::Heal => run.hp = run.stats().max_hp,
    }
    run.version += 1;
    match mode {
        OfferMode::Treasure(_) => {
            if !run.chests.is_empty() {
                run.chests.remove(0);
            }
        }
        OfferMode::Level => {
            run.pending_levels = run.pending_levels.saturating_sub(1);
            if run.has_relic("hero_charm") {
                run.b.charm_t = 10.0;
            }
        }
        OfferMode::Forget => {}
    }
    let max = run.stats().max_hp;
    run.hp = run.hp.min(max);
    if run.pending_levels > 0 || !run.chests.is_empty() {
        next_offer(run, rng, offer);
    } else {
        offer.choices.clear();
        offer.mode = OfferMode::Level;
        next.set(Phase::Playing);
    }
}

pub(super) fn looks_for(
    class: &data::ClassDef,
    creatures: &crate::entities::Creatures,
) -> Vec<(u32, u8, u8)> {
    let mut out: Vec<(u32, u8, u8)> = Vec::new();
    for &d in class.looks {
        if let Some((race, sex)) = creatures.display_race_sex(d) {
            if !out.iter().any(|&(_, r, s)| r == race && s == sex) {
                out.push((d, race, sex));
            }
        }
    }
    out.sort_by_key(|&(_, r, s)| (r, s));
    out
}

#[allow(clippy::too_many_arguments)]
pub(super) fn buttons(
    mut presses: Query<
        (&Interaction, &UiAction, &BaseColor, &mut BackgroundColor),
        Changed<Interaction>,
    >,
    mut next: ResMut<NextState<Phase>>,
    mut exit: MessageWriter<AppExit>,
    phase: Res<State<Phase>>,
) {
    for (interaction, action, base, mut bg) in &mut presses {
        match interaction {
            Interaction::Hovered => bg.0 = BUTTON_HOT,
            Interaction::None => bg.0 = base.0,
            Interaction::Pressed => {
                bg.0 = BUTTON_HOT;
                match (*action, *phase.get()) {
                    (UiAction::Resume, Phase::Paused) => next.set(Phase::Playing),
                    (UiAction::MainMenu, Phase::Paused | Phase::GameOver) => next.set(Phase::Menu),
                    (UiAction::Quit, Phase::Paused | Phase::GameOver) => {
                        exit.write(AppExit::Success);
                    }
                    _ => {}
                }
            }
        }
    }
}

pub(super) fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    phase: Res<State<Phase>>,
    mut selection: ResMut<Selection>,
    mut run: ResMut<Run>,
    mut next: ResMut<NextState<Phase>>,
) {
    match phase.get() {
        Phase::Menu => {
            if keys.just_pressed(KeyCode::Enter) && start_run(&mut run, &mut selection) {
                next.set(Phase::Playing);
            }
        }
        Phase::Playing => {
            if keys.just_pressed(KeyCode::Escape) {
                next.set(Phase::Paused);
            }
        }
        Phase::Paused => {
            if keys.just_pressed(KeyCode::Escape) {
                next.set(Phase::Playing);
            }
        }
        Phase::LevelUp => {}
        Phase::GameOver => {
            if keys.just_pressed(KeyCode::Enter) {
                next.set(Phase::Menu);
            }
        }
    }
}

/// The class's looks once the creature data is in, defaulting to the first. The auto-runner picks
/// a class with no look; the menu itself always sets both.
pub(super) fn refresh_menu(
    mut selection: ResMut<Selection>,
    creatures: Option<Res<crate::entities::Creatures>>,
) {
    if let (Some(class), Some(creatures)) = (selection.class_def(), creatures.as_deref()) {
        if selection.looks.is_empty() {
            let looks = looks_for(class, creatures);
            if !looks.is_empty() {
                selection.looks = looks;
                selection.look = Some(0);
                selection.version += 1;
            }
        }
    }
}

/// What a level-up card shows.
pub(super) struct CardText {
    pub spell: u32,
    pub name: String,
    pub tag: String,
    pub tag_color: Color,
    pub desc: String,
    /// `(rank now, top rank)`, for the pips; `None` for the healthstone.
    pub ranks: Option<(u8, u8)>,
}

pub(super) fn describe_card(
    choice: Choice,
    run: &Run,
    spells: Option<&crate::ui_action::Spells>,
) -> CardText {
    let new = Color::srgb(0.3, 1.0, 0.3);
    match choice {
        Choice::NewAbility(def) => {
            let stats = run.stats();
            let c = cast_at(def, 1, &stats);
            CardText {
                spell: def.spell,
                name: spell_name(spells, def.spell, def.key),
                tag: "New spell".into(),
                tag_color: new,
                // The body ends its own sentence.
                desc: ability_body(def, &c),
                ranks: Some((0, data::MAX_ABILITY_LEVEL)),
            }
        }
        Choice::Upgrade(k) => {
            let o = &run.abilities[k];
            let next = o.level + 1;
            let stats = run.stats();
            let (a, b) = (
                cast_at(o.def, o.level, &stats),
                cast_at(o.def, next, &stats),
            );
            // What actually changes, as before → after.
            let mut lines = Vec::new();
            let (da, db) = if a.dmg > 0.0 {
                (a.dmg, b.dmg)
            } else {
                (a.dot, b.dot)
            };
            if db > da {
                lines.push(format!("Damage {} » {}", num(da), num(db)));
            }
            if b.cooldown < a.cooldown - 0.01 {
                lines.push(format!(
                    "Cooldown {}s » {}s",
                    num(a.cooldown),
                    num(b.cooldown)
                ));
            }
            if b.count > a.count {
                lines.push(format!("Targets {} » {}", a.count, b.count));
            }
            if b.radius > a.radius + 0.05 {
                lines.push(format!("Area {} » {} yd", num(a.radius), num(b.radius)));
            }
            if b.duration > a.duration + 0.05 {
                lines.push(format!("Lasts {}s » {}s", num(a.duration), num(b.duration)));
            }
            CardText {
                spell: o.def.spell,
                name: spell_name(spells, o.def.spell, o.def.key),
                tag: format!("Rank {} » {next}", o.level),
                tag_color: Color::WHITE,
                desc: lines.join("\n"),
                ranks: Some((o.level, data::MAX_ABILITY_LEVEL)),
            }
        }
        Choice::NewPassive(p) => CardText {
            spell: p.spell,
            name: p.name.into(),
            tag: "New talent".into(),
            tag_color: new,
            desc: passive_text(p.stat, p.per_level),
            ranks: Some((0, data::MAX_PASSIVE_LEVEL)),
        },
        Choice::UpPassive(k) => {
            let (p, level) = run.passives[k];
            let fmt = |v: f32| match p.stat {
                Stat::Regen | Stat::Armor => num(v),
                _ => pct(v),
            };
            CardText {
                spell: p.spell,
                name: p.name.into(),
                tag: format!("Rank {level} » {}", level + 1),
                tag_color: Color::WHITE,
                desc: format!(
                    "{}\n{} » {}",
                    stat_label(p.stat),
                    fmt(p.per_level * f32::from(level)),
                    fmt(p.per_level * f32::from(level + 1)),
                ),
                ranks: Some((level, data::MAX_PASSIVE_LEVEL)),
            }
        }
        Choice::Heal => CardText {
            spell: 5720,
            name: "Healthstone".into(),
            tag: "Restore".into(),
            tag_color: Color::WHITE,
            desc: "Everything is mastered. Restores all health.".into(),
            ranks: None,
        },
        Choice::Rune(k, r) => {
            let o = &run.abilities[k];
            CardText {
                spell: r.spell,
                name: r.name.into(),
                tag: format!("Enchant {}", spell_name(spells, o.def.spell, o.def.key)),
                tag_color: Color::srgb(0.55, 0.85, 1.0),
                desc: r.desc.into(),
                ranks: Some((
                    o.runes.len() as u8,
                    synergy::rune_slots(data::MAX_ABILITY_LEVEL) as u8,
                )),
            }
        }
        Choice::Talent(t) => {
            let mut desc = t.desc.to_string();
            if let Req::Tree(c, tree, 0) = t.req {
                if let Some(swap) = synergy::tree_def(c, tree).and_then(|d| d.swap) {
                    let from = data::ability(run.class.start);
                    if run.abilities.iter().any(|o| o.def.key == from.key) {
                        let to = data::ability(swap);
                        desc = format!(
                            "{desc}\nYour {} becomes {}.",
                            spell_name(spells, from.spell, from.key),
                            spell_name(spells, to.spell, to.key)
                        );
                    }
                }
            }
            CardText {
                spell: t.spell,
                name: t.name.into(),
                tag: talent_tag(t),
                tag_color: if t.cursed { CURSED } else { new },
                desc,
                ranks: None,
            }
        }
        Choice::Learn(t) => {
            let def = data::ability(t.capstone);
            let c = cast_at(def, 1, &run.stats());
            CardText {
                spell: def.spell,
                name: spell_name(spells, def.spell, def.key),
                tag: format!("{} deep talent: new spell", t.name),
                tag_color: super::cards::LEGENDARY,
                desc: tf("{} Takes no spell slot.", &[&ability_body(def, &c)]),
                ranks: Some((0, data::MAX_ABILITY_LEVEL)),
            }
        }
        Choice::Combo(c) => {
            let def = data::ability(c.key);
            let parts: Vec<String> = c
                .needs
                .iter()
                .map(|n| match *n {
                    Need::Max(k) => {
                        let d = data::ability(k);
                        spell_name(spells, d.spell, d.key)
                    }
                    Need::MaxSchool(s) => format!("a {} spell", s.name()),
                    Need::Relic(k) => synergy::relic_def(k).name.to_string(),
                })
                .collect();
            CardText {
                spell: def.spell,
                name: spell_name(spells, def.spell, def.key),
                tag: format!("Combination: {}", parts.join(" + ")),
                tag_color: super::cards::LEGENDARY,
                desc: format!("{} Takes no spell slot.", c.desc),
                ranks: Some((0, data::MAX_ABILITY_LEVEL)),
            }
        }
        Choice::Relic(r) => CardText {
            spell: r.spell,
            name: r.name.into(),
            tag: item_tag(r).into(),
            tag_color: if r.cursed { CURSED } else { GOLD },
            desc: r.desc.into(),
            ranks: None,
        },
        Choice::Forget(k) => {
            let o = &run.abilities[k];
            let back = refund(o);
            CardText {
                spell: o.def.spell,
                name: spell_name(spells, o.def.spell, o.def.key),
                tag: format!("Unlearn  (rank {})", o.level),
                tag_color: CURSED,
                desc: if back > 0 {
                    format!("Unlearn it and its enchants, and win back {back} picks.")
                } else {
                    "Unlearn it, freeing its slot.".into()
                },
                ranks: None,
            }
        }
    }
}

/// An item's line under its name, by its quality.
pub(super) fn item_tag(r: &RelicDef) -> &'static str {
    match (r.cursed, r.tier) {
        (true, _) => "Cursed item",
        (_, Tier::Rare) => "Rare item",
        (_, Tier::Epic) => "Epic item",
        (_, Tier::Legendary) => "Legendary item",
    }
}

/// The red of a curse and an unlearn.
pub(super) const CURSED: Color = Color::srgb(1.0, 0.3, 0.25);

/// A talent's line under its name: what kind of talent it is.
pub(super) fn talent_tag(t: &TalentDef) -> String {
    match t.req {
        _ if t.cursed => "Cursed talent".into(),
        Req::Duo(a, b) => format!("Hybrid talent: {} + {}", a.name(), b.name()),
        Req::Tree(c, tree, tier) => {
            let name = synergy::tree_def(c, tree).map_or("", |d| d.name);
            if tier == 0 {
                format!("{name} specialization")
            } else {
                format!("{name} talent, tier 2")
            }
        }
        Req::None => "Talent".into(),
    }
}

/// The class resource and the build's running surges, in a line under the clock.
fn resource_line(run: &Run) -> String {
    let b = &run.b;
    let mut parts: Vec<String> = Vec::new();
    match run.class.id {
        1 if b.surge_t > 0.0 => parts.push(format!("ENRAGED {}s", b.surge_t.ceil())),
        1 => parts.push(format!("Rage {}", b.res as u32)),
        2 => parts.push(format!("Holy Power {}", b.res as u32)),
        3 if b.surge_t > 0.0 => parts.push(format!("RAPID FIRE {}s", b.surge_t.ceil())),
        3 => parts.push("Hunter's Mark".into()),
        4 if b.res >= 5.0 => parts.push("Combo 5/5  FINISHER READY".into()),
        4 => parts.push(format!("Combo {}/5", b.res as u32)),
        7 => {
            let n = if run.has_talent("elemental_mastery") {
                3
            } else {
                5
            };
            parts.push(format!("Overload in {}", n - b.casts % n))
        }
        8 if b.last_school != 0 => parts.push(format!(
            "Master of Elements: next non-{} spell +{}",
            synergy::primary(b.last_school).name(),
            if run.has_talent("combustion") {
                "45%"
            } else {
                "20%"
            }
        )),
        9 => parts.push(format!("Soul Shards {}/5", b.res as u32)),
        11 if b.lunar_t > 0.0 => parts.push(format!("LUNAR ECLIPSE {}s", b.lunar_t.ceil())),
        11 if b.solar_t > 0.0 => parts.push(format!("SOLAR ECLIPSE {}s", b.solar_t.ceil())),
        11 => parts.push(format!("Lunar {}/5  Solar {}/5", b.nature_n, b.arcane_n)),
        _ => {}
    }
    if b.arcane > 0 {
        parts.push(format!("Arcane Charges {}", b.arcane));
    }
    for (on, name) in [
        (b.fury_t > 0.0, "Blood Fury"),
        (b.stone_t > 0.0, "Stoneform"),
        (b.ephemeral_t > 0.0, "Ephemeral Power"),
        (b.charm_t > 0.0, "Unstable Power"),
        (b.zeal > 0, "Judgement of Light"),
        (b.flurry_t > 0.0, "Flurry"),
    ] {
        if on {
            parts.push(name.into());
        }
    }
    parts.join("   ")
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn refresh_hud(
    mut commands: Commands,
    time: Res<Time>,
    phase: Res<State<Phase>>,
    mut run: ResMut<Run>,
    selection: Res<Selection>,
    font: Res<HudFont>,
    mut texts: ParamSet<(
        Query<&mut Text, With<LevelText>>,
        Query<&mut Text, With<TimerText>>,
        Query<&mut Text, With<KillText>>,
        Query<&mut Text, With<BannerText>>,
        Query<&mut Text, With<OverTitle>>,
        Query<&mut Text, With<OverStats>>,
        Query<&mut Text, With<XpText>>,
        Query<&mut Text, With<ResourceText>>,
    )>,
    mut xp_fill: Query<&mut Node, With<XpFill>>,
    rows: Query<(Entity, Has<AbilityRow>), Or<(With<AbilityRow>, With<PassiveRow>)>>,
    mut sweeps: ResMut<Assets<SweepUiMaterial>>,
    mut last_version: Local<Option<u32>>,
) {
    let need = data::xp_to_next(run.level);
    if let Ok(mut n) = xp_fill.single_mut() {
        n.width = Val::Percent((run.xp / need * 100.0).clamp(0.0, 100.0));
    }
    if let Ok(mut t) = texts.p6().single_mut() {
        let xp = format!("XP {} / {}", run.xp as u32, need as u32);
        if **t != xp {
            **t = xp;
        }
    }
    if let Ok(mut t) = texts.p0().single_mut() {
        **t = format!("Level {}", run.level);
    }
    let secs = run.elapsed as u32;
    if let Ok(mut t) = texts.p1().single_mut() {
        **t = format!("{:02}:{:02}", secs / 60, secs % 60);
    }
    let kills = run.kills;
    if let Ok(mut t) = texts.p2().single_mut() {
        **t = format!("Kills {kills}");
    }
    let resource = resource_line(&run);
    if let Ok(mut t) = texts.p7().single_mut() {
        if **t != resource {
            **t = resource;
        }
    }
    let banner = match run.banner.as_mut() {
        Some((msg, t)) => {
            *t -= time.delta_secs();
            if *t > 0.0 {
                msg.clone()
            } else {
                String::new()
            }
        }
        None => String::new(),
    };
    if banner.is_empty() {
        run.banner = None;
    }
    // The picker owns the screen: an announcement waits until it closes.
    let banner = if *phase.get() == Phase::LevelUp {
        String::new()
    } else {
        banner
    };
    if let Ok(mut t) = texts.p3().single_mut() {
        if **t != banner {
            **t = banner;
        }
    }
    let title = if run.won {
        "Victory!"
    } else {
        "You have fallen"
    }
    .to_string();
    if let Ok(mut t) = texts.p4().single_mut() {
        **t = title;
    }
    let summary = format!(
        "{} {} in {}   —   press Enter to play again",
        crate::char_select::race_name(run.race),
        run.class.name,
        selection.map_def().name,
    );
    if let Ok(mut t) = texts.p5().single_mut() {
        if **t != summary {
            **t = summary;
        }
    }

    // The ability and passive icons, rebuilt when the build changes.
    if *last_version == Some(run.version) {
        return;
    }
    *last_version = Some(run.version);
    let f = &font.0;
    for (row, is_abilities) in &rows {
        commands.entity(row).despawn_related::<Children>();
        commands.entity(row).with_children(|row| {
            // `(icon, rank, size, tooltip, edge)`; no rank for the talents, relics and traits.
            let entries: Vec<(u32, Option<u8>, f32, TipFor, Color)> = if is_abilities {
                run.abilities
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        // A runed spell's edge glows rune blue.
                        let edge = if o.runes.is_empty() {
                            EDGE
                        } else {
                            Color::srgb(0.35, 0.7, 1.0)
                        };
                        (o.def.spell, Some(o.level), 40.0, TipFor::Ability(i), edge)
                    })
                    .collect()
            } else {
                let mut v: Vec<(u32, Option<u8>, f32, TipFor, Color)> = Vec::new();
                let (race_icon, ..) = synergy::race_trait(run.race);
                if race_icon != 0 {
                    v.push((race_icon, None, 26.0, TipFor::Race, DIM));
                }
                let (class_icon, ..) = synergy::class_trait(run.class.id);
                if class_icon != 0 {
                    v.push((class_icon, None, 26.0, TipFor::Class, DIM));
                }
                v.extend(
                    run.passives
                        .iter()
                        .enumerate()
                        .map(|(i, (p, l))| (p.spell, Some(*l), 30.0, TipFor::Passive(i), EDGE)),
                );
                v.extend(run.talents.iter().enumerate().map(|(i, t)| {
                    let edge = if t.cursed {
                        CURSED
                    } else if matches!(t.req, Req::Duo(..)) {
                        super::cards::LEGENDARY
                    } else if matches!(t.req, Req::Tree(..)) {
                        super::cards::EPIC
                    } else {
                        super::cards::UNCOMMON
                    };
                    (t.spell, None, 30.0, TipFor::Talent(i), edge)
                }));
                v.extend(run.relics.iter().enumerate().map(|(i, r)| {
                    let edge = if r.cursed { CURSED } else { GOLD };
                    (r.spell, None, 30.0, TipFor::Relic(i), edge)
                }));
                v
            };
            for (spell, level, size, tip, edge) in entries {
                let ability = match &tip {
                    TipFor::Ability(i) => Some(*i),
                    _ => None,
                };
                row.spawn((
                    Node {
                        width: Val::Px(size),
                        height: Val::Px(size),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(edge),
                    Interaction::default(),
                    tip,
                ))
                .with_children(|slot| {
                    slot.spawn(icon(spell, size - 2.0));
                    // The cooldown sweep sits over the icon, under the level text.
                    if let Some(i) = ability {
                        let handle = sweeps.add(SweepUiMaterial { sweep: 0.0 });
                        slot.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            MaterialNode(handle.clone()),
                            AbilitySweep { handle, index: i },
                        ));
                    }
                    if let Some(level) = level {
                        slot.spawn(Node {
                            position_type: PositionType::Absolute,
                            right: Val::Px(2.0),
                            bottom: Val::Px(0.0),
                            ..default()
                        })
                        .with_children(|n| {
                            n.spawn(text(f, level.to_string(), 12.0, Color::WHITE));
                        });
                    }
                });
            }
        });
    }
}

/// Fill icon slots from `SpellIcon.dbc` once the client data and the asset reader are up.
pub(super) fn fill_icons(
    mut commands: Commands,
    mut slots: Query<(Entity, &mut ImageNode, &SpellIcon), Without<IconDone>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut assets: Option<ResMut<benilla_assets::WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut cache: Local<HashMap<u32, Handle<Image>>>,
) {
    let (Some(spells), Some(assets)) = (spells.as_deref(), assets.as_deref_mut()) else {
        return;
    };
    for (e, mut img, icon) in &mut slots {
        let handle = match cache.get(&icon.0) {
            Some(h) => Some(h.clone()),
            None => {
                let path = spells
                    .catalog
                    .get(icon.0)
                    .and_then(|d| d.icon.clone())
                    .unwrap_or_else(|| "Interface\\Icons\\INV_Misc_QuestionMark".into());
                let h = assets.sprite_texture(&format!("{path}.blp"), &mut images);
                if let Some(h) = &h {
                    cache.insert(icon.0, h.clone());
                }
                h
            }
        };
        if let Some(h) = handle {
            img.image = h;
            img.color = Color::WHITE;
        }
        commands.entity(e).insert(IconDone);
    }
}

/// The hand-cast buttons: each shown once learned, its icon the spell, its sweep the cooldown.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_mobility(
    run: Res<Run>,
    mut commands: Commands,
    mut slots: Query<(&mut Node, &MobilitySlot)>,
    icons: Query<(Entity, Option<&SpellIcon>, &MobilityIcon)>,
    sweeps: Query<&MobilitySweep>,
    mut mats: ResMut<Assets<SweepUiMaterial>>,
) {
    // Each button's spell and cooldown fraction left.
    let action = |i: u8| -> Option<(u32, f32)> {
        match i {
            0 | 1 => run
                .active(usize::from(i))
                .map(|d| (d.spell, run.act_sweep(usize::from(i)))),
            _ => run.mobility().map(|m| {
                (
                    m.spell,
                    (run.mobile.cd / m.cooldown.max(0.01)).clamp(0.0, 1.0),
                )
            }),
        }
    };
    for (mut node, slot) in &mut slots {
        let display = if action(slot.0).is_some() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    // A new spell re-arms the icon fill ([`fill_icons`]).
    for (e, have, icon) in &icons {
        let Some((spell, _)) = action(icon.0) else { continue };
        if have.is_none_or(|s| s.0 != spell) {
            commands
                .entity(e)
                .remove::<IconDone>()
                .insert(SpellIcon(spell));
        }
    }
    for s in &sweeps {
        let Some((_, f)) = action(s.1) else { continue };
        if let Some(m) = mats.get_mut(&s.0) {
            if m.sweep != f {
                m.sweep = f;
            }
        }
    }
}

/// Drive each ability icon's cooldown sweep from its remaining cooldown.
pub(super) fn update_cooldown_sweeps(
    run: Res<Run>,
    mut mats: ResMut<Assets<SweepUiMaterial>>,
    sweeps: Query<&AbilitySweep>,
) {
    let stats = run.stats();
    for s in &sweeps {
        let Some(o) = run.abilities.get(s.index) else {
            continue;
        };
        let total = o.cooldown(&stats).max(0.01);
        let f = (o.cd / total).clamp(0.0, 1.0);
        if let Some(m) = mats.get_mut(&s.handle) {
            m.sweep = f;
        }
    }
}

/// Draw each [`UiArt`] region once its file can be read.
pub(super) fn fill_art(
    mut commands: Commands,
    mut regions: Query<(Entity, &mut ImageNode, &UiArt), Without<ArtDone>>,
    assets: Option<ResMut<benilla_assets::WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut assets) = assets else { return };
    for (e, mut img, art) in &mut regions {
        commands.entity(e).insert(ArtDone);
        let Some(h) = assets.sprite_texture(art.path, &mut images) else {
            warn!("survivors: no UI art {}", art.path);
            continue;
        };
        let [l, r, t, b] = art.coords;
        if let Some(size) = images.get(&h).map(Image::size_f32) {
            img.rect = Some(Rect::new(l * size.x, t * size.y, r * size.x, b * size.y));
        }
        img.image = h;
        img.color = art.tint;
    }
}

/// The reference lays its UI out on a screen 768 units tall at any resolution (`useUiScale` off).
pub(super) fn apply_ui_scale(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut scale: ResMut<UiScale>,
) {
    let Ok(window) = windows.single() else { return };
    let want = window.height() / 768.0;
    if want > 0.0 && (scale.0 - want).abs() > 1e-4 {
        scale.0 = want;
    }
}

/// Between the hero's feet and the top of its plate's bar, in gx units.
const PLATE_FEET_GAP: f32 = 0.004;
/// A couple of screen pixels lower than the gap alone sets it.
const PLATE_NUDGE: f32 = 2.0;

/// The hero's nameplate under its feet, sized from the screen diagonal as [`crate::vplates`]
/// sizes a plate, outside the UI scale. The border's top half seats a name the hero has none of,
/// so the plate rises until its bar sits just under the feet.
#[allow(clippy::type_complexity)]
pub(super) fn hero_plate(
    run: Res<Run>,
    ui_scale: Res<UiScale>,
    hero: Query<&Transform, With<Hero>>,
    cams: Query<(&Camera, &Transform), With<WorldCamera>>,
    mut nodes: ParamSet<(
        Query<(&mut Node, &mut Visibility), With<HpFrame>>,
        Query<&mut Node, With<HpBar>>,
        Query<&mut Node, With<HpFill>>,
        Query<&mut Node, With<ShieldFill>>,
        Query<&mut Node, With<HpLevelSeat>>,
    )>,
    mut level: Query<(&mut Text, &mut TextFont), With<HpLevel>>,
) {
    use crate::vplates::{
        gx_px, plate_basis, text_px, BAR_H, BAR_OFF_X, BAR_OFF_Y, BAR_W, LEVEL_H, LEVEL_OFF_X,
        LEVEL_OFF_Y, PLATE_H, PLATE_W,
    };
    let seat = hero.single().ok().and_then(|h| {
        let (cam, tf) = cams.single().ok()?;
        let viewport = cam.logical_viewport_size()?;
        let feet = cam
            .world_to_viewport(&GlobalTransform::from(*tf), h.translation)
            .ok()?;
        Some((feet, viewport))
    });
    let mut frames = nodes.p0();
    let Ok((mut frame, mut vis)) = frames.single_mut() else {
        return;
    };
    let Some((feet, viewport)) = seat else {
        *vis = Visibility::Hidden;
        return;
    };
    *vis = Visibility::Inherited;
    let s = ui_scale.0;
    let basis = plate_basis(viewport);
    let gx = |v: f32| gx_px(v, basis) / s;
    let (pw, ph) = (gx(PLATE_W), gx(PLATE_H));
    let bar_top = ph - gx(BAR_OFF_Y) - gx(BAR_H);
    frame.width = Val::Px(pw);
    frame.height = Val::Px(ph);
    frame.left = Val::Px(feet.x / s - pw / 2.0);
    frame.top = Val::Px(feet.y / s + gx(PLATE_FEET_GAP) - bar_top + PLATE_NUDGE / s);
    if let Ok(mut bar) = nodes.p1().single_mut() {
        bar.left = Val::Px(gx(BAR_OFF_X));
        bar.top = Val::Px(bar_top);
        bar.width = Val::Px(gx(BAR_W));
        bar.height = Val::Px(gx(BAR_H));
    }
    let stats = run.stats();
    let hp = (run.hp / stats.max_hp).clamp(0.0, 1.0);
    let shield = if run.shield_t > 0.0 {
        (run.shield / stats.max_hp).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if let Ok(mut n) = nodes.p2().single_mut() {
        n.width = Val::Percent(hp * 100.0);
    }
    if let Ok(mut n) = nodes.p3().single_mut() {
        n.width = Val::Percent(shield * 100.0);
    }
    if let Ok(mut seat) = nodes.p4().single_mut() {
        seat.left = Val::Px(pw - gx(LEVEL_OFF_X));
        seat.top = Val::Px(ph - gx(LEVEL_OFF_Y));
    }
    if let Ok((mut text, mut font)) = level.single_mut() {
        let n = run.level.to_string();
        if text.0 != n {
            text.0 = n;
        }
        let size = text_px(LEVEL_H, basis) / s;
        if font.font_size != size {
            font.font_size = size;
        }
    }
}

/// `MainMenuExpBar`'s `OnEnter` and `OnLeave`: the numbers show while the cursor is on the bar.
pub(super) fn xp_hover(
    bars: Query<&Interaction, (With<XpBar>, Changed<Interaction>)>,
    mut texts: Query<&mut Visibility, With<XpText>>,
) {
    for interaction in &bars {
        let want = if *interaction == Interaction::None {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        for mut v in &mut texts {
            *v = want;
        }
    }
}

// ── The build icons' `GameTooltip` ──────────────────────────────────────────────────────────────

/// `TOOLTIP_DEFAULT_BACKGROUND_COLOR`, the `GameTooltip` backdrop tint (`GameTooltip.lua`).
const TIP_FILL: Color = Color::srgb(0.09, 0.09, 0.19);
/// The grey a spell tooltip prints its rank in, right of the name.
const TIP_GREY: Color = Color::srgb(0.5, 0.5, 0.5);

/// One tooltip line: the left text, an optional right-aligned one, the colour and font size
/// (`GameTooltipHeaderText` 14, `GameTooltipText` 12).
struct TipLine {
    left: String,
    right: Option<(String, Color)>,
    color: Color,
    size: f32,
}

impl TipLine {
    fn new(left: impl Into<String>, color: Color) -> Self {
        Self {
            left: left.into(),
            right: None,
            color,
            size: 12.0,
        }
    }

    fn right(mut self, text: impl Into<String>, color: Color) -> Self {
        self.right = Some((text.into(), color));
        self
    }

    fn header(mut self) -> Self {
        self.size = 14.0;
        self
    }
}

/// A number as the tooltips print it: whole when near enough, else to one decimal.

/// The results screen's tiles and its line under the title.
#[allow(clippy::type_complexity)]
pub(super) fn refresh_results(
    run: Res<Run>,
    selection: Res<Selection>,
    mut over_texts: ParamSet<(
        Query<(&OverStat, &mut Text)>,
        Query<&mut Text, With<OverLine>>,
    )>,
) {
    let secs = run.elapsed as u32;
    let stats_line = if run.won {
        selection.map_def().victory
    } else {
        "The horde was too much this time."
    };
    let values = [
        format!("{:02}:{:02}", secs / 60, secs % 60),
        run.level.to_string(),
        run.kills.to_string(),
        compact(run.damage_done),
        run.bosses.to_string(),
    ];
    for (stat, mut t) in &mut over_texts.p0() {
        if let Some(v) = values.get(stat.0) {
            if **t != *v {
                **t = v.clone();
            }
        }
    }
    if let Ok(mut t) = over_texts.p1().single_mut() {
        if **t != stats_line {
            **t = stats_line.to_string();
        }
    }
}

/// 12.3k, 4.5M.
fn compact(v: f64) -> String {
    if v >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if v >= 1e4 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{}", v.round() as u64)
    }
}

fn num(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

fn pct(v: f32) -> String {
    format!("{}%", num(v * 100.0))
}

/// The numbers an ability casts with at a level, as [`super::abilities::cast_abilities`] takes
/// them from [`Owned`].
struct Cast {
    dmg: f32,
    dot: f32,
    count: u32,
    radius: f32,
    duration: f32,
    cooldown: f32,
}

fn cast_at(def: &'static AbilityDef, level: u8, stats: &Stats) -> Cast {
    let o = Owned {
        level,
        ..Owned::new(def, 0.0)
    };
    let mult = o.damage_mult() * stats.might;
    Cast {
        dmg: def.damage * mult,
        dot: def.dot * mult,
        count: o.count(),
        radius: o.radius(stats),
        duration: o.duration(),
        cooldown: o.cooldown(stats),
    }
}

fn effect_text(effect: Effect) -> Option<String> {
    Some(match effect {
        Effect::None => return None,
        Effect::Slow(m, t) => tf("slowing them by {} for {} sec", &[&pct(1.0 - m), &num(t)]),
        Effect::Root(t) => tf("freezing them in place for {} sec", &[&num(t)]),
        Effect::Stun(t) => tf("stunning them for {} sec", &[&num(t)]),
        Effect::Fear(t) => tf("sending them fleeing for {} sec", &[&num(t)]),
        Effect::Weaken(m, t) => tf("cutting their damage by {} for {} sec", &[&pct(1.0 - m), &num(t)]),
        Effect::Drain(f) => tf("healing you for {} of the damage", &[&pct(f)]),
        Effect::Execute(f) => tf("killing them outright below {} health", &[&pct(f)]),
    })
}

/// What an ability does at these numbers, in the voice of a spell description.
fn ability_body(def: &AbilityDef, c: &Cast) -> String {
    let d = num(c.dmg);
    let burst = def.duration <= 0.0;
    let mut s = match def.kind {
        Kind::Bolt | Kind::Strike | Kind::Melee => {
            let toughest = matches!(def.key, "pyroblast" | "aimed_shot" | "exorcism" | "starfire");
            let melee = def.kind == Kind::Melee;
            let who = match (toughest, melee, c.count) {
                (true, _, 1) => w("the toughest enemy in range"),
                (true, _, n) => tf("the {} toughest enemies in range", &[&n]),
                (false, true, 1) => w("the closest enemy"),
                (false, true, n) => tf("the {} closest enemies", &[&n]),
                (false, false, 1) => w("the nearest enemy"),
                (false, false, n) => tf("the {} nearest enemies", &[&n]),
            };
            let verb = if melee { "Strikes" } else { "Hits" };
            let mut s = tf("{} {} for {} damage", &[&verb, &who, &d]);
            if c.radius > 0.0 && !melee {
                s += &tf(", and everything within {} yd of the hit", &[&num(c.radius)]);
            }
            s
        }
        Kind::Nova => tf("Deals {} damage to every enemy within {} yd of you", &[&d, &num(c.radius)]),
        Kind::Cone => tf(
            "Deals {} damage to every enemy in a {} yd cone toward the closest",
            &[&d, &num(c.radius)],
        ),
        Kind::Ground | Kind::GroundSelf => {
            let at = if def.kind == Kind::GroundSelf { "around you" } else { "on the thickest pack" };
            if burst {
                tf("Strikes a {} yd area {} for {} damage", &[&num(c.radius), &at, &d])
            } else {
                tf(
                    "Covers a {} yd area {} for {} sec, dealing {} damage every second",
                    &[&num(c.radius), &at, &num(c.duration), &d],
                )
            }
        }
        Kind::Chain => tf(
            "Strikes the nearest enemy for {} damage, then jumps to {} more within {} yd of each other, 15% weaker with every jump",
            &[&d, &c.count, &num(c.radius)],
        ),
        Kind::Dot => tf(
            "Afflicts {} enemies in range with {} damage every second for {} sec",
            &[&c.count, &num(c.dot), &num(c.duration)],
        ),
        Kind::Aura => tf(
            "Every {} sec, deals {} damage to every enemy within {} yd of you",
            &[&num(c.cooldown), &d, &num(c.radius)],
        ),
        Kind::Orbit => {
            let who = if c.count == 1 { w("an enemy") } else { tf("{} enemies", &[&c.count]) };
            tf(
                "Every {} sec, zaps {} within {} yd of you for {} damage",
                &[&num(c.cooldown), &who, &num(c.radius), &d],
            )
        }
        Kind::Shield => tf("Absorbs the next {} damage you take within {} sec", &[&d, &num(c.duration)]),
        Kind::Immune => tf(
            "When an enemy closes in, makes you immune to all damage for {} sec",
            &[&num(c.duration)],
        ),
        Kind::Thorns => tf(
            "When enemies close in, deals {} damage to every attacker for {} sec",
            &[&d, &num(c.duration)],
        ),
        Kind::Heal => tf("Below 85% health, heals you for {} over {} sec", &[&d, &num(c.duration)]),
        Kind::Totem if def.key == "magma_totem" => tf(
            "Drops a totem for {} sec that burns every enemy within {} yd for {} damage every 2 sec",
            &[&num(c.duration), &num(c.radius), &d],
        ),
        Kind::Totem => tf(
            "Drops a totem for {} sec that hits the nearest enemy within {} yd for {} damage every second",
            &[&num(c.duration), &num(def.range), &d],
        ),
    };
    // Areas that tick carry no over-time part; a burst leaves a 5 second one.
    let ground = matches!(def.kind, Kind::Ground | Kind::GroundSelf);
    if c.dot > 0.0 && def.kind != Kind::Dot && (!ground || burst) {
        let secs = if ground { 5.0 } else { c.duration };
        s += &tf(", then {} damage every second for {} sec", &[&num(c.dot), &num(secs)]);
    }
    if let Some(e) = effect_text(def.effect) {
        s += i18n::comma();
        s += &e;
    }
    s.push(i18n::stop());
    s
}

/// What the next rank changes, at the current stats.
fn next_rank(def: &AbilityDef, now: &Cast, next: &Cast) -> String {
    let what = match def.kind {
        Kind::Shield => "absorbed",
        Kind::Heal => "healing",
        _ => "damage",
    };
    let mut gains = Vec::new();
    if next.dmg > 0.0 && def.kind != Kind::Dot && def.kind != Kind::Immune {
        gains.push(format!("{} {}", num(next.dmg), w(what)));
    }
    if next.dot > 0.0 {
        gains.push(tf("{} damage per second", &[&num(next.dot)]));
    }
    if next.count != now.count
        && matches!(
            def.kind,
            Kind::Bolt | Kind::Strike | Kind::Melee | Kind::Dot | Kind::Chain | Kind::Orbit
        )
    {
        gains.push(match def.kind {
            Kind::Chain => tf("{} jumps", &[&next.count]),
            _ => tf("{} targets", &[&next.count]),
        });
    }
    if next.radius > 0.0 {
        gains.push(tf("{} yd", &[&num(next.radius)]));
    }
    if next.duration > 0.0 {
        gains.push(tf("{} sec", &[&num(next.duration)]));
    }
    gains.push(tf("{} sec cooldown", &[&num(next.cooldown)]));
    gains.join(i18n::comma())
}

fn ability_tip(
    o: &Owned,
    stats: &Stats,
    spells: Option<&crate::ui_action::Spells>,
) -> Vec<TipLine> {
    let def = o.def;
    let now = cast_at(def, o.level, stats);
    let mut lines = vec![
        TipLine::new(spell_name(spells, def.spell, def.key), Color::WHITE)
            .header()
            .right(format!("Rank {}", o.level), TIP_GREY),
    ];
    let range = match def.kind {
        Kind::Melee => Some("Melee Range".to_string()),
        Kind::Bolt | Kind::Strike | Kind::Chain | Kind::Dot | Kind::Ground | Kind::Totem => {
            Some(format!("{} yd range", num(def.range)))
        }
        _ => None,
    };
    let mut cast = TipLine::new("Casts itself", Color::WHITE);
    if let Some(range) = range {
        cast = cast.right(range, Color::WHITE);
    }
    lines.push(cast);
    lines.push(
        TipLine::new("Instant", Color::WHITE)
            .right(format!("{} sec cooldown", num(now.cooldown)), Color::WHITE),
    );
    lines.push(TipLine::new(ability_body(def, &now), GOLD));
    let schools: Vec<&str> = synergy::School::ALL
        .into_iter()
        .filter(|s| synergy::has(o.schools(), *s))
        .map(|s| s.name())
        .collect();
    lines.push(TipLine::new(format!("{} damage", schools.join(" + ")), DIM));
    if !matches!(
        def.kind,
        Kind::Shield | Kind::Immune | Kind::Heal | Kind::Dot
    ) {
        lines.push(TipLine::new(
            format!(
                "{} chance to critically strike for {} damage",
                pct(stats.crit),
                pct(stats.crit_mult)
            ),
            DIM,
        ));
    }
    for r in &o.runes {
        lines.push(TipLine::new(r.name, Color::srgb(0.55, 0.85, 1.0)));
        lines.push(TipLine::new(r.desc, Color::WHITE));
    }
    let slots = synergy::rune_slots(o.level);
    if o.runes.len() < slots {
        lines.push(TipLine::new(
            "An enchant slot is open.",
            Color::srgb(0.55, 0.85, 1.0),
        ));
    }
    if o.rampage > 0.0 {
        lines.push(TipLine::new(format!("Rampage: +{}", pct(o.rampage)), GOLD));
    }
    if o.level < data::MAX_ABILITY_LEVEL {
        let next = cast_at(def, o.level + 1, stats);
        lines.push(TipLine::new("Next rank:", Color::WHITE));
        lines.push(TipLine::new(next_rank(def, &now, &next), GOLD));
    } else {
        lines.push(TipLine::new("Highest rank", DIM));
    }
    lines
}

/// What a passive's bonus `v` does, in the voice of a buff.
/// A stat's short name, for the card's before » after line.
fn stat_label(stat: Stat) -> &'static str {
    match stat {
        Stat::Might => "All damage",
        Stat::MaxHp => "Maximum health",
        Stat::Speed => "Movement speed",
        Stat::Regen => "Health per second",
        Stat::Haste => "Recharge speed",
        Stat::Area => "Area size",
        Stat::Magnet => "Pickup range",
        Stat::Growth => "Experience gained",
        Stat::Armor => "Damage blocked per hit",
        Stat::Crit => "Critical strike chance",
    }
}

fn passive_text(stat: Stat, v: f32) -> String {
    match stat {
        Stat::Might => format!("Increases all damage you deal by {}.", pct(v)),
        Stat::MaxHp => format!("Increases your maximum health by {}.", pct(v)),
        Stat::Speed => format!("Increases your movement speed by {}.", pct(v)),
        Stat::Regen => format!("Restores {} health every second.", num(v)),
        Stat::Haste => format!("Your abilities recharge {} faster.", pct(v)),
        Stat::Area => format!("Increases the size of your areas of effect by {}.", pct(v)),
        Stat::Magnet => format!("Increases your pickup range by {}.", pct(v)),
        Stat::Growth => format!("Increases the experience you gain by {}.", pct(v)),
        Stat::Armor => format!("Reduces the damage of every hit you take by {}.", num(v)),
        Stat::Crit => format!(
            "Increases your chance to critically strike for double damage by {}.",
            pct(v)
        ),
    }
}

/// The hero's total for the stat a passive raises, every source counted.
fn stat_total(stat: Stat, s: &Stats) -> String {
    match stat {
        Stat::Might => format!("Damage dealt: {}", pct(s.might)),
        Stat::MaxHp => format!("Maximum health: {}", num(s.max_hp)),
        Stat::Speed => format!("Movement speed: {} yd per second", num(s.speed)),
        Stat::Regen => format!("Health restored: {} per second", num(s.regen)),
        Stat::Haste => format!("Cooldowns: {} shorter", pct(s.haste.min(0.65))),
        Stat::Area => format!("Area of effect: {}", pct(s.area)),
        Stat::Magnet => format!("Pickup range: {} yd", num(s.magnet)),
        Stat::Growth => format!("Experience gained: {}", pct(s.growth)),
        Stat::Armor => format!("Damage blocked per hit: {}", num(s.armor)),
        Stat::Crit => format!("Critical strike chance: {}", pct(s.crit)),
    }
}

fn passive_tip(p: &PassiveDef, level: u8, stats: &Stats) -> Vec<TipLine> {
    let mut lines = vec![
        TipLine::new(p.name, Color::WHITE)
            .header()
            .right(format!("Rank {level}"), TIP_GREY),
        TipLine::new(passive_text(p.stat, p.per_level * f32::from(level)), GOLD),
        TipLine::new(stat_total(p.stat, stats), Color::WHITE),
    ];
    if level < data::MAX_PASSIVE_LEVEL {
        lines.push(TipLine::new("Next rank:", Color::WHITE));
        lines.push(TipLine::new(
            passive_text(p.stat, p.per_level * f32::from(level + 1)),
            GOLD,
        ));
    } else {
        lines.push(TipLine::new("Highest rank", DIM));
    }
    lines
}

/// The `GameTooltip` frame: `UI-Tooltip-Background` tiled under `UI-Tooltip-Border`, 16-unit edges
/// and 5-unit insets, its lines 10 units in. Returns the frame and its line column.
fn spawn_tooltip(
    commands: &mut Commands,
    assets: &mut benilla_assets::WorldAssets,
    images: &mut Assets<Image>,
    ui_scale: f32,
) -> (Entity, Entity) {
    use crate::glue::backdrop::{backdrop_border, backdrop_edges, tiled_bg_node};
    let bg = assets.sprite_texture("Interface\\Tooltips\\UI-Tooltip-Background", images);
    let edges = backdrop_edges(assets, "Interface\\Tooltips\\UI-Tooltip-Border", images);
    let mut body = Entity::PLACEHOLDER;
    let mut root = commands.spawn((
        GameTooltip,
        Node {
            position_type: PositionType::Absolute,
            max_width: Val::Px(300.0),
            ..default()
        },
        GlobalZIndex(1000),
        Visibility::Hidden,
        bevy::ui::FocusPolicy::Pass,
    ));
    if bg.is_none() || edges.is_none() {
        root.insert(BackgroundColor(TIP_FILL.with_alpha(0.95)));
    }
    root.with_children(|t| {
        if let Some(bg) = bg {
            t.spawn((
                tiled_bg_node(bg, 16.0, ui_scale, TIP_FILL),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(5.0),
                    right: Val::Px(5.0),
                    top: Val::Px(5.0),
                    bottom: Val::Px(5.0),
                    ..default()
                },
            ));
        }
        if let Some(edges) = &edges {
            backdrop_border(t, edges, 16.0, Color::WHITE);
        }
        body = t
            .spawn((
                TipBody,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    padding: UiRect::all(Val::Px(10.0)),
                    ..default()
                },
            ))
            .id();
    });
    (root.id(), body)
}

/// Hovering a build icon raises its `GameTooltip` under the icon, every number from the current
/// rank and stats, and what the next rank brings.
#[allow(clippy::too_many_arguments)]
pub(super) fn icon_tooltip(
    mut commands: Commands,
    phase: Res<State<Phase>>,
    run: Res<Run>,
    spells: Option<Res<crate::ui_action::Spells>>,
    font: Res<HudFont>,
    ui_scale: Res<UiScale>,
    mut assets: Option<ResMut<benilla_assets::WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    slots: Query<(
        &Interaction,
        &TipFor,
        &ComputedNode,
        &bevy::ui::UiGlobalTransform,
    )>,
    mut frames: Query<(&mut Node, &mut Visibility), With<GameTooltip>>,
    mut built: Local<Option<(Entity, Entity)>>,
    mut shown: Local<Option<(TipFor, u32)>>,
) {
    let Some((frame, body)) = *built else {
        if let Some(assets) = assets.as_deref_mut() {
            *built = Some(spawn_tooltip(
                &mut commands,
                assets,
                &mut images,
                ui_scale.0,
            ));
        }
        return;
    };
    let Ok((mut node, mut vis)) = frames.get_mut(frame) else {
        return;
    };
    let hovered = slots
        .iter()
        .find(|(i, ..)| **i != Interaction::None)
        .filter(|_| *phase.get() != Phase::Menu);
    let Some((_, &tip, computed, at)) = hovered else {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        *shown = None;
        return;
    };
    // Physical pixels to the 768-unit canvas the `Val`s are on.
    let inv = computed.inverse_scale_factor;
    let (left, top) = (
        Val::Px((at.translation.x - computed.size().x / 2.0) * inv),
        Val::Px((at.translation.y + computed.size().y / 2.0) * inv + 4.0),
    );
    if node.left != left || node.top != top {
        node.left = left;
        node.top = top;
    }
    if *shown == Some((tip, run.version)) {
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }
        return;
    }
    let stats = run.stats();
    let lines = match tip {
        TipFor::Ability(i) => run
            .abilities
            .get(i)
            .map(|o| ability_tip(o, &stats, spells.as_deref())),
        TipFor::Passive(i) => run
            .passives
            .get(i)
            .map(|&(p, level)| passive_tip(p, level, &stats)),
        TipFor::Talent(i) => run.talents.get(i).map(|t| {
            vec![
                TipLine::new(t.name, Color::WHITE).header(),
                TipLine::new(talent_tag(t), if t.cursed { CURSED } else { DIM }),
                TipLine::new(t.desc, GOLD),
            ]
        }),
        TipFor::Relic(i) => run.relics.get(i).map(|r| {
            vec![
                TipLine::new(r.name, Color::WHITE).header(),
                TipLine::new(item_tag(r), DIM),
                TipLine::new(r.desc, GOLD),
            ]
        }),
        TipFor::Race => {
            let (_, name, desc) = synergy::race_trait(run.race);
            Some(vec![
                TipLine::new(name, Color::WHITE).header(),
                TipLine::new(
                    format!("{} racial trait", crate::char_select::race_name(run.race)),
                    DIM,
                ),
                TipLine::new(desc, GOLD),
            ])
        }
        TipFor::Class => {
            let (_, name, desc) = synergy::class_trait(run.class.id);
            Some(vec![
                TipLine::new(name, Color::WHITE).header(),
                TipLine::new(format!("{} class trait", run.class.name), DIM),
                TipLine::new(desc, GOLD),
            ])
        }
        TipFor::Action(i @ (0 | 1)) => run.active(usize::from(i)).map(|a| {
            let what = if i == 0 { "Burst" } else { "Guard" };
            let key = if i == 0 { "Q" } else { "E" };
            let mut lines = vec![
                TipLine::new(spell_name(spells.as_deref(), a.spell, a.name), Color::WHITE).header(),
                TipLine::new(format!("{what}: press {key}"), Color::WHITE)
                    .right(format!("{} sec cooldown", num(a.cooldown)), Color::WHITE),
            ];
            if a.secs > 0.0 {
                lines.push(TipLine::new(format!("Lasts {} sec", num(a.secs)), DIM));
            }
            lines.push(TipLine::new(a.desc, GOLD));
            lines
        }),
        TipFor::Action(_) => run.mobility().map(|m| {
            vec![
                TipLine::new(spell_name(spells.as_deref(), m.spell, m.name), Color::WHITE).header(),
                TipLine::new("Press F", Color::WHITE)
                    .right(format!("{} sec cooldown", num(m.cooldown)), Color::WHITE),
                TipLine::new(m.desc, GOLD),
            ]
        }),
    };
    let Some(lines) = lines else { return };
    *shown = Some((tip, run.version));
    // Shown from the next frame, once the new lines have laid out.
    *vis = Visibility::Hidden;
    let f = &font.0;
    commands.entity(body).despawn_related::<Children>();
    commands.entity(body).with_children(|b| {
        for line in lines {
            let face = |s: String, size: f32, color: Color| {
                (
                    Text::new(s),
                    TextFont {
                        font: f.clone(),
                        font_size: size,
                        ..default()
                    },
                    TextColor(color),
                    TextShadow {
                        offset: Vec2::splat(1.0),
                        color: Color::BLACK,
                    },
                )
            };
            b.spawn(Node {
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(16.0),
                ..default()
            })
            .with_children(|row| {
                row.spawn(face(line.left, line.size, line.color));
                if let Some((text, color)) = line.right {
                    row.spawn((
                        face(text, line.size, color),
                        TextLayout::new(Justify::Right, LineBreak::NoWrap),
                    ));
                }
            });
        }
    });
}

#[allow(clippy::type_complexity)]
pub(super) fn show_panels(
    phase: Res<State<Phase>>,
    mut roots: ParamSet<(
        Query<(&mut Visibility, &mut Node), With<MenuRoot>>,
        Query<(&mut Visibility, &mut Node), With<HudRoot>>,
        Query<(&mut Visibility, &mut Node), With<LevelUpRoot>>,
        Query<(&mut Visibility, &mut Node), With<PauseRoot>>,
        Query<(&mut Visibility, &mut Node), With<OverRoot>>,
    )>,
) {
    let p = *phase.get();
    // A hidden panel leaves the layout too: a merely invisible button still takes clicks.
    let set = |(mut v, mut node): (Mut<Visibility>, Mut<Node>), on: bool| {
        let want = if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
        let display = if on { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    };
    for root in &mut roots.p0() {
        set(root, p == Phase::Menu);
    }
    for root in &mut roots.p1() {
        set(root, p != Phase::Menu && p != Phase::GameOver);
    }
    for root in &mut roots.p2() {
        set(root, p == Phase::LevelUp);
    }
    for root in &mut roots.p3() {
        set(root, p == Phase::Paused);
    }
    for root in &mut roots.p4() {
        set(root, p == Phase::GameOver);
    }
}
