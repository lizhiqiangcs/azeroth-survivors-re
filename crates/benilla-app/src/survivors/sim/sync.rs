//! Keeping the simulator honest. The numbers and the cards come straight from the game's source,
//! but the casts, hits and procs the game runs as ECS systems are redone in `world.rs`. This test
//! reads the game's systems, collects every enchant, talent and item they test by key, and fails
//! when one is handled neither by the code the simulator shares (`Run::stats`, `Keys::of`,
//! `hud::pick`, `hero::hurt_hero`) nor by `world.rs`: a new mechanic added to the game has to be
//! added to the simulator, or named below as having no effect on the balance.

/// Keys the game tests that do not change the numbers: visuals, sounds, the HUD.
const NO_BALANCE: &[(&str, &str)] = &[];

/// The keys tested in `src` through `has_rune("…")`, `has_talent("…")`, `has_relic("…")` and
/// `.key == "…"`.
fn keys_tested(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for pat in ["has_rune(\"", "has_talent(\"", "has_relic(\"", ".key == \""] {
        let mut rest = src;
        while let Some(i) = rest.find(pat) {
            rest = &rest[i + pat.len()..];
            if let Some(end) = rest.find('"') {
                out.push(rest[..end].to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn quoted(src: &str, key: &str) -> bool {
    src.contains(&format!("\"{key}\""))
}

#[test]
fn sim_handles_every_mechanic() {
    let game = [
        ("abilities.rs", include_str!("../abilities.rs")),
        ("procs.rs", include_str!("../procs.rs")),
        ("enemies.rs", include_str!("../enemies.rs")),
        ("hero.rs", include_str!("../hero.rs")),
    ];
    let abilities = include_str!("../abilities.rs");
    let keys_of = abilities
        .find("fn of(")
        .map(|i| &abilities[i..(i + 6000).min(abilities.len())])
        .unwrap_or("");
    let hurt = include_str!("../hero.rs");
    let hurt = hurt
        .find("fn hurt_hero")
        .map(|i| &hurt[i..(i + 4000).min(hurt.len())])
        .unwrap_or("");
    let shared = [
        include_str!("world.rs"),
        include_str!("policy.rs"),
        include_str!("../mod.rs"),
        include_str!("../hud.rs"),
        keys_of,
        hurt,
    ];
    let mut missing = Vec::new();
    for (file, src) in game {
        for k in keys_tested(src) {
            if NO_BALANCE.iter().any(|(n, _)| *n == k) {
                continue;
            }
            if !shared.iter().any(|s| quoted(s, &k)) {
                missing.push(format!("{file}: \"{k}\""));
            }
        }
    }
    missing.dedup();
    assert!(
        missing.is_empty(),
        "the simulator does not model these mechanics the game tests; add them to sim/world.rs, \
         or to NO_BALANCE in sim/sync.rs with the reason:\n  {}",
        missing.join("\n  ")
    );
}
