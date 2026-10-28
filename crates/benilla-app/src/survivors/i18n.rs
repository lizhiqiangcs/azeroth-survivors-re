//! Simplified Chinese beside English.
//!
//! The game is written in English, and its text stays English in the code. The translation sits
//! at the screen: every `Text` and `TextSpan` the game sets is looked up in `i18n/zh_CN.tsv` as it
//! changes, line by line, and shown in Chinese with a Chinese font. A line with values in it
//! (`Level 12`, `You have learned Blink! Press F.`) matches the catalog's `{}` templates, and the
//! values are translated in turn, so a name inside a sentence comes out Chinese too. Sentences the
//! game assembles from pieces translate their pieces where they are built ([`tf`]), since the whole
//! never matches one template.
//!
//! `scripts/i18n-extract.py` lists the English the catalog lacks; the `catalog_covers_the_game`
//! test fails on it, so a new spell or talent cannot ship untranslated.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use bevy::prelude::*;
use bevy::text::detect_text_needs_rerender;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lang {
    En,
    Zh,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub(crate) fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 {
        Lang::Zh
    } else {
        Lang::En
    }
}

fn set_lang(l: Lang) {
    LANG.store(u8::from(l == Lang::Zh), Ordering::Relaxed);
}

/// Simplified Chinese, Noto Sans SC Medium cut to GB 2312 (SIL Open Font License,
/// `i18n/NotoSansSC-OFL.txt`).
const CJK_FONT: &[u8] = include_bytes!("i18n/NotoSansSC-Medium-GB2312.ttf");
const ZH_CN: &str = include_str!("i18n/zh_CN.tsv");

struct Template {
    /// The English around the values: `n + 1` pieces for `n` values.
    parts: Vec<&'static str>,
    zh: &'static str,
}

struct Catalog {
    exact: HashMap<&'static str, &'static str>,
    /// Spell names by ability key: the names on screen come from the client's own spell data.
    spells: HashMap<&'static str, &'static str>,
    /// Longest literal first, so the most specific template wins.
    templates: Vec<Template>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut c = Catalog {
            exact: HashMap::new(),
            spells: HashMap::new(),
            templates: Vec::new(),
        };
        for line in ZH_CN.lines() {
            if line.starts_with('#') {
                continue;
            }
            let Some((en, zh)) = line.split_once('\t') else {
                continue;
            };
            if let Some(key) = en.strip_prefix('@') {
                c.spells.insert(key, zh);
            } else if en.contains("{}") {
                let parts: Vec<&str> = en.split("{}").collect();
                // A template of values alone would match anything.
                if parts.iter().any(|p| p.chars().any(char::is_alphabetic)) {
                    c.templates.push(Template { parts, zh });
                }
                // `tf` looks a template up whole.
                c.exact.insert(en, zh);
            } else {
                c.exact.insert(en, zh);
            }
        }
        c.templates
            .sort_by_key(|t| std::cmp::Reverse(t.parts.iter().map(|p| p.len()).sum::<usize>()));
        c
    })
}

/// The text in the current language, translated line by line; what the catalog lacks stays
/// English.
pub(crate) fn t(s: &str) -> Cow<'_, str> {
    if lang() == Lang::En || !s.chars().any(|c| c.is_ascii_alphabetic()) {
        return Cow::Borrowed(s);
    }
    if !s.contains('\n') {
        return line(s, 0).map_or(Cow::Borrowed(s), Cow::Owned);
    }
    let lines: Vec<Cow<str>> = s
        .split('\n')
        .map(|l| line(l, 0).map_or(Cow::Borrowed(l), Cow::Owned))
        .collect();
    Cow::Owned(lines.join("\n"))
}

/// One line, or `None` when the catalog has nothing for it. Leading and trailing spaces are
/// kept: the HUD pads with them.
fn line(s: &str, depth: u8) -> Option<String> {
    let body = s.trim();
    if body.is_empty() || !body.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let c = catalog();
    let zh = c.exact.get(body).map(|z| (*z).to_string()).or_else(|| {
        if depth > 2 {
            return None;
        }
        c.templates.iter().find_map(|t| {
            let caps = capture(body, &t.parts)?;
            let caps: Vec<String> = caps
                .into_iter()
                .map(|v| line(v, depth + 1).unwrap_or_else(|| v.to_string()))
                .collect();
            Some(fill(t.zh, &caps))
        })
    })?;
    let lead = &s[..s.len() - s.trim_start().len()];
    let trail = &s[s.trim_end().len()..];
    Some(format!("{lead}{zh}{trail}"))
}

/// The values a template's pieces leave in `s`, if it fits.
fn capture<'a>(s: &'a str, parts: &[&str]) -> Option<Vec<&'a str>> {
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    let rest = s.strip_prefix(first)?;
    let mut rest = rest.strip_suffix(last)?;
    let mut caps = Vec::with_capacity(parts.len() - 1);
    for p in &parts[1..parts.len() - 1] {
        // The value is at least one character, so the next piece is looked for after it.
        let at = rest.get(1..)?.find(p)? + 1;
        caps.push(&rest[..at]);
        rest = &rest[at + p.len()..];
    }
    if rest.is_empty() {
        return None;
    }
    caps.push(rest);
    Some(caps)
}

/// The Chinese with its values in: `{}` takes them in order, `{0}`, `{1}`… by position.
fn fill(zh: &str, caps: &[String]) -> String {
    let mut out = String::with_capacity(zh.len() + 16);
    let mut next = 0;
    let mut rest = zh;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let i = if inner.is_empty() {
            next += 1;
            next - 1
        } else if let Ok(i) = inner.parse::<usize>() {
            i
        } else {
            out.push('{');
            rest = after;
            continue;
        };
        out.push_str(caps.get(i).map_or("", String::as_str));
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A sentence built where the game assembles it: `template` is the English with `{}` for each
/// value, as the catalog keys it.
pub(crate) fn tf(template: &str, args: &[&dyn Display]) -> String {
    let caps: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let zh = (lang() == Lang::Zh)
        .then(|| catalog().exact.get(template).copied())
        .flatten();
    let caps: Vec<String> = if zh.is_some() {
        caps.iter().map(|v| t(v).into_owned()).collect()
    } else {
        caps
    };
    fill(zh.unwrap_or(template), &caps)
}

/// A word in the current language: [`t`] for a piece the game joins itself.
pub(crate) fn w(s: &str) -> String {
    t(s).into_owned()
}

/// The joint between a sentence's clauses, and its end.
pub(crate) fn comma() -> &'static str {
    if lang() == Lang::Zh {
        "，"
    } else {
        ", "
    }
}

pub(crate) fn stop() -> char {
    if lang() == Lang::Zh {
        '。'
    } else {
        '.'
    }
}

/// A spell's name by its ability key, in Chinese when there is one.
pub(crate) fn spell(key: &str) -> Option<&'static str> {
    (lang() == Lang::Zh)
        .then(|| catalog().spells.get(key).copied())
        .flatten()
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c) || ('\u{ff00}'..='\u{ffef}').contains(&c))
}

// ── Saved choice ───────────────────────────────────────────────────────────────────────────────

/// `benilla-config/survivors-lang.txt` (`localStorage` on the page): `zh` or `en`, written by the
/// menu's language button. Without one, the browser's or the system's language decides.
fn lang_path() -> Option<std::path::PathBuf> {
    crate::local_state::home().map(|h| h.join("survivors-lang.txt"))
}

fn initial() -> Lang {
    let saved = lang_path().and_then(|p| crate::local_state::read_to_string(&p).ok());
    // `?survivors_lang=zh` on the page (`WOW_SURVIVORS_LANG` natively) overrides the saved choice.
    let wanted = crate::webenv::var("WOW_SURVIVORS_LANG").or(saved).or_else(system);
    match wanted.as_deref().map(str::trim) {
        Some(l) if l.to_ascii_lowercase().starts_with("zh") => Lang::Zh,
        _ => Lang::En,
    }
}

#[cfg(target_arch = "wasm32")]
fn system() -> Option<String> {
    web_sys::window().and_then(|w| w.navigator().language())
}

#[cfg(not(target_arch = "wasm32"))]
fn system() -> Option<String> {
    std::env::var("LANG").ok()
}

/// Switches the language, keeps the choice and re-shows every text in it.
#[derive(Message)]
pub(crate) struct ToggleLang;

// ── The screen ─────────────────────────────────────────────────────────────────────────────────

#[derive(Resource)]
pub(crate) struct CjkFont(pub(crate) Handle<Font>);

/// What the game set, before translation, and the font it chose.
#[derive(Component)]
struct Source {
    en: String,
    font: Handle<Font>,
}

pub(crate) fn plugin(app: &mut App) {
    set_lang(initial());
    app.add_message::<ToggleLang>().add_systems(Startup, load_font).add_systems(
        PostUpdate,
        (
            toggle,
            translate::<Text>,
            translate::<TextSpan>,
        )
            .chain()
            .before(detect_text_needs_rerender::<Text>),
    );
}

fn load_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let font = Font::try_from_bytes(CJK_FONT.to_vec()).expect("the bundled Chinese font parses");
    commands.insert_resource(CjkFont(fonts.add(font)));
}

fn toggle(
    mut msgs: MessageReader<ToggleLang>,
    mut texts: Query<(&mut Text, Option<&Source>)>,
    mut spans: Query<(&mut TextSpan, Option<&Source>)>,
) {
    if msgs.read().count() % 2 == 0 {
        return;
    }
    let l = if lang() == Lang::Zh { Lang::En } else { Lang::Zh };
    set_lang(l);
    if let Some(p) = lang_path() {
        let _ = crate::local_state::write_atomic(&p, if l == Lang::Zh { "zh" } else { "en" });
    }
    // Back to what the game set, marked changed: `translate` shows it again in the new language.
    for (mut text, src) in &mut texts {
        if let Some(src) = src {
            text.0.clone_from(&src.en);
        }
        text.set_changed();
    }
    for (mut span, src) in &mut spans {
        if let Some(src) = src {
            span.0.clone_from(&src.en);
        }
        span.set_changed();
    }
}

/// Each text the game set this frame, in the current language. The rewrite bypasses change
/// detection, so the game's own change still drives the re-shape and this does not see its own
/// writes.
fn translate<T>(
    mut commands: Commands,
    cjk: Option<Res<CjkFont>>,
    mut texts: Query<(Entity, &mut T, &mut TextFont, Option<&mut Source>), Changed<T>>,
    mut cache: Local<HashMap<String, String>>,
) where
    T: Component<Mutability = bevy::ecs::component::Mutable>
        + std::ops::DerefMut<Target = String>,
{
    let Some(cjk) = cjk else { return };
    for (e, mut text, mut font, src) in &mut texts {
        let en: String = String::clone(&text);
        let en = &en;
        let shown: Option<String> = if lang() == Lang::Zh {
            if let Some(hit) = cache.get(en) {
                Some(hit.clone()).filter(|o| o != en)
            } else {
                let out = t(en);
                let out = matches!(out, Cow::Owned(_)).then(|| out.into_owned());
                if cache.len() > 4096 {
                    cache.clear();
                }
                cache.insert(en.clone(), out.clone().unwrap_or_else(|| en.clone()));
                out.filter(|o| o != en)
            }
        } else {
            None
        };
        let final_cjk = has_cjk(shown.as_deref().unwrap_or(en));
        // The game's font, from the record if this text was rewritten before.
        let game_font = match &src {
            Some(s) if font.font == cjk.0 => s.font.clone(),
            _ => font.font.clone(),
        };
        match src {
            Some(mut s) => {
                s.en.clone_from(en);
                s.font = game_font.clone();
            }
            None if shown.is_some() || final_cjk => {
                commands.entity(e).insert(Source {
                    en: en.clone(),
                    font: game_font.clone(),
                });
            }
            None => {}
        }
        if let Some(shown) = shown {
            **text.bypass_change_detection() = shown;
        }
        let want = if final_cjk { cjk.0.clone() } else { game_font };
        if font.font != want {
            font.bypass_change_detection().font = want;
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The language is global: tests that switch it take turns.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn zh<R>(f: impl FnOnce() -> R) -> R {
        let _turn = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_lang(Lang::Zh);
        let r = f();
        set_lang(Lang::En);
        r
    }

    #[test]
    fn exact_templates_and_nesting() {
        zh(|| {
            assert_eq!(t("Resume"), "继续");
            assert_eq!(t("Level 12"), "等级 12");
            assert_eq!(t("You have learned Blink! Press F."), "你学会了闪现术！按 F 施放。");
            assert_eq!(t("  Paused  "), "  已暂停  ");
            assert_eq!(t("Resume\nQuit game"), "继续\n退出游戏");
            assert_eq!(t("Not in the catalog"), "Not in the catalog");
            assert_eq!(
                tf("Strikes a {} yd area {} for {} damage", &[&8, &"around you", &30]),
                "在你周围打击 8 码范围，造成 30 点伤害"
            );
        });
        let _turn = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(t("Resume"), "Resume");
    }

    /// Every display string in the survivors code has its Chinese: the extractor's list must be
    /// empty. Run `python scripts/i18n-extract.py` to see what is missing.
    #[test]
    fn catalog_covers_the_game() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/i18n-extract.py");
        let Ok(out) = std::process::Command::new("python").arg(root).output() else {
            eprintln!("no python; skipped");
            return;
        };
        let missing = String::from_utf8_lossy(&out.stdout);
        assert!(missing.trim().is_empty(), "untranslated:\n{missing}");
    }

    #[test]
    fn every_ability_has_a_chinese_name() {
        zh(|| {
            let mut missing = Vec::new();
            for a in super::super::data::ABILITIES {
                if spell(a.key).is_none() {
                    missing.push(a.key);
                }
            }
            assert!(missing.is_empty(), "no Chinese name: {missing:?}");
        });
    }
}
