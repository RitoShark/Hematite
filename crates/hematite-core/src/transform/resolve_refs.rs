//! `resolve_dead_refs` transform — game-TOC-aware dead-reference ladder.
//!
//! Rewrites dead asset-path strings (ones that don't exist in either the
//! mod's own WAD or the live game index) to a live form. Tries, in order:
//! an extension "twin" (`.dds`↔`.tex`, `.sco`↔`.scb`) in the mod WAD or the
//! game, then a Riot inner-suffix-stripped rename in the game, then the
//! extension twin of *that* stripped form in the game. Strings that already
//! exist somewhere reachable (mod WAD or game) are left untouched — this
//! transform only repairs genuinely dead references.
//!
//! No-op (returns 0) when `ctx.game` is `None` — without a live game index
//! there's nothing to consult past the mod's own WAD, and guessing would
//! risk pointing a mod at content that isn't actually there.
//!
//! ## Used by
//! - `resolve_dead_refs` config rule: sweeps asset-path strings ending in a
//!   configured extension and repairs dead ones using the game TOC.

use crate::context::FixContext;
use crate::walk::{walk_tree, PropertyVisitor, VisitResult};
use hematite_types::hash::FieldHash;

/// Compute the "twin" extension for a path: `.dds`↔`.tex`, `.sco`↔`.scb`.
/// Returns `None` for extensions without a known twin.
pub(crate) fn ext_twin(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    let (from, to) = if lower.ends_with(".dds") {
        (".dds", ".tex")
    } else if lower.ends_with(".tex") {
        (".tex", ".dds")
    } else if lower.ends_with(".sco") {
        (".sco", ".scb")
    } else if lower.ends_with(".scb") {
        (".scb", ".sco")
    } else {
        return None;
    };
    crate::strings::replace_extension(path, from, to)
}

/// Strip an inner "tag" segment from a filename's stem.
///
/// Riot occasionally ships files under a stripped name in the live game
/// WAD even though the BIN string references a tagged variant — e.g.
/// `attack1.matcha_ambessa.anm` lives in the WAD as `attack1.anm`. Mods
/// authored against the tagged variant lose the reference unless we look
/// the bytes up under both spellings.
///
/// Returns `Some(stripped)` when the filename has the shape
/// `<stem>.<inner>.<ext>` (two dots, three segments). The inner segment is
/// dropped:
///
/// ```text
///   data/c/yone/anim/attack1.matcha_ambessa.anm
/// → data/c/yone/anim/attack1.anm
/// ```
///
/// Returns `None` for filenames that don't carry an inner tag segment
/// (the common case — single-dot filenames pass through untouched).
///
/// Deliberately duplicated from `hematite_file::wad_adapter::strip_inner_suffix`
/// — `hematite-core` cannot depend on file-format crates, so this is a
/// standalone copy of the same contract. Keep both in sync by hand if the
/// rule ever changes.
pub(crate) fn strip_inner_suffix(path: &str) -> Option<String> {
    let (dir, file) = match path.rsplit_once('/') {
        Some((d, f)) => (Some(d), f),
        None => (None, path),
    };
    // Require exactly three segments split by '.': stem . inner . ext
    let parts: Vec<&str> = file.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    if parts[0].is_empty() || parts[1].is_empty() || parts[2].is_empty() {
        return None;
    }
    let stripped_file = format!("{}.{}", parts[0], parts[2]);
    Some(match dir {
        Some(d) => format!("{}/{}", d, stripped_file),
        None => stripped_file,
    })
}

struct DeadRefResolver<'a> {
    extensions: &'a [String],
    wad: &'a dyn crate::traits::WadProvider,
    game: &'a dyn crate::traits::GameProvider,
}

impl DeadRefResolver<'_> {
    fn matches_configured_extension(&self, value: &str) -> bool {
        let lower = value.to_lowercase();
        self.extensions.iter().any(|ext| {
            let ext_lower = ext.to_lowercase();
            let suffix = format!(".{ext_lower}");
            lower.ends_with(&suffix)
        })
    }
}

impl PropertyVisitor for DeadRefResolver<'_> {
    fn visit_string(&mut self, value: &str, _hash: FieldHash) -> VisitResult {
        if !self.matches_configured_extension(value) {
            return VisitResult::Skip;
        }

        // 1. Exact match in the mod's own WAD -> already live, skip.
        if self.wad.has_path(value) {
            return VisitResult::Skip;
        }

        // 2. Exact match in the live game index -> already live, skip.
        if self.game.has_path(value) {
            return VisitResult::Skip;
        }

        // 3. Extension twin (.dds<->.tex, .sco<->.scb) in mod WAD or game.
        if let Some(twin) = ext_twin(value) {
            if self.wad.has_path(&twin) || self.game.has_path(&twin) {
                return VisitResult::Mutate(twin);
            }
        }

        // 4. Inner-suffix-stripped form in the game.
        if let Some(stripped) = strip_inner_suffix(value) {
            if self.game.has_path(&stripped) {
                return VisitResult::Mutate(stripped);
            }

            // 5. Extension twin of the stripped form in the game.
            if let Some(stripped_twin) = ext_twin(&stripped) {
                if self.game.has_path(&stripped_twin) {
                    return VisitResult::Mutate(stripped_twin);
                }
            }
        }

        // 6. Nothing resolves it -> leave unchanged.
        VisitResult::Skip
    }
}

/// Rewrite dead asset-path strings ending in a configured extension to a
/// live form, consulting both the mod WAD and the live game index. See the
/// module docs for the full ladder. Returns the number of strings rewritten.
///
/// No-op (returns 0) when `ctx.game` is `None`.
pub fn apply(ctx: &mut FixContext, extensions: &[String]) -> u32 {
    let Some(game) = ctx.game else {
        tracing::debug!("resolve_dead_refs: skipped, no game provider available");
        return 0;
    };

    let mut visitor = DeadRefResolver {
        extensions,
        wad: ctx.wad,
        game,
    };

    walk_tree(&mut ctx.tree, &mut visitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{GameProvider, HashProvider, WadProvider};
    use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue};
    use hematite_types::champion::CharacterRelations;
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;
    use std::collections::HashSet;

    struct MockWadProvider {
        has: HashSet<String>,
    }
    impl MockWadProvider {
        fn new(paths: &[&str]) -> Self {
            Self {
                has: paths.iter().map(|s| s.to_lowercase()).collect(),
            }
        }
        fn empty() -> Self {
            Self::new(&[])
        }
    }
    impl WadProvider for MockWadProvider {
        fn has_path(&self, path: &str) -> bool {
            self.has.contains(&path.to_lowercase())
        }
        fn has_hash(&self, _hash: u64) -> bool {
            false
        }
    }

    struct MockGameProvider {
        has: HashSet<String>,
    }
    impl MockGameProvider {
        fn new(paths: &[&str]) -> Self {
            Self {
                has: paths.iter().map(|s| s.to_lowercase()).collect(),
            }
        }
        fn empty() -> Self {
            Self::new(&[])
        }
    }
    impl GameProvider for MockGameProvider {
        fn has_path(&self, path: &str) -> bool {
            self.has.contains(&path.to_lowercase())
        }
        fn pull_raw(&self, _path: &str) -> Option<Vec<u8>> {
            None
        }
        fn game_bin(&self, _path: &str) -> Option<BinTree> {
            None
        }
    }

    struct NoopHashProvider;
    impl HashProvider for NoopHashProvider {
        fn resolve_type(&self, _hash: TypeHash) -> Option<&str> {
            None
        }
        fn resolve_field(&self, _hash: FieldHash) -> Option<&str> {
            None
        }
        fn resolve_entry(&self, _hash: PathHash) -> Option<&str> {
            None
        }
        fn resolve_game_path(&self, _hash: hematite_types::hash::GameHash) -> Option<&str> {
            None
        }
        fn type_hash(&self, _name: &str) -> Option<TypeHash> {
            None
        }
        fn field_hash(&self, _name: &str) -> Option<FieldHash> {
            None
        }
        fn is_loaded(&self) -> bool {
            true
        }
        fn has_game_path(&self, _path: &str) -> bool {
            false
        }
    }

    fn tree_with_string(value: &str) -> BinTree {
        let mut tree = BinTree::default();
        let mut properties = IndexMap::new();
        properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String(value.to_string()),
            },
        );
        tree.objects.insert(
            0xAAAA,
            BinObject {
                class_hash: TypeHash(0x1234),
                path_hash: PathHash(0xAAAA),
                properties,
            },
        );
        tree
    }

    fn string_value(tree: &BinTree) -> &str {
        match &tree.objects[&0xAAAA].properties[&0x1].value {
            PropertyValue::String(s) => s.as_str(),
            other => panic!("expected String value, got {other:?}"),
        }
    }

    fn base_ctx<'a>(
        tree: BinTree,
        hashes: &'a dyn HashProvider,
        wad: &'a dyn WadProvider,
    ) -> FixContext<'a> {
        FixContext {
            tree,
            hashes,
            wad,
            champions: Box::leak(Box::new(CharacterRelations::default())),
            file_path: "main.bin".to_string(),
            files_to_remove: Vec::new(),
            linked_trees: std::collections::HashMap::new(),
            shader_validator: None,
            game: None,
            additional_bins: Vec::new(),
        }
    }

    fn extensions() -> Vec<String> {
        vec![
            "dds".to_string(),
            "tex".to_string(),
            "anm".to_string(),
            "skn".to_string(),
            "skl".to_string(),
            "scb".to_string(),
            "sco".to_string(),
        ]
    }

    #[test]
    fn skips_when_mod_ships_file() {
        let hashes = NoopHashProvider;
        let wad = MockWadProvider::new(&["assets/foo.dds"]);
        let game = MockGameProvider::empty();

        let tree = tree_with_string("assets/foo.dds");
        let mut ctx = base_ctx(tree, &hashes, &wad);
        ctx.game = Some(&game);

        let count = apply(&mut ctx, &extensions());

        assert_eq!(count, 0);
        assert_eq!(string_value(&ctx.tree), "assets/foo.dds");
    }

    #[test]
    fn skips_when_game_ships_file() {
        let hashes = NoopHashProvider;
        let wad = MockWadProvider::empty();
        let game = MockGameProvider::new(&["assets/foo.dds"]);

        let tree = tree_with_string("assets/foo.dds");
        let mut ctx = base_ctx(tree, &hashes, &wad);
        ctx.game = Some(&game);

        let count = apply(&mut ctx, &extensions());

        assert_eq!(count, 0);
        assert_eq!(string_value(&ctx.tree), "assets/foo.dds");
    }

    #[test]
    fn rewrites_to_tex_twin_in_game() {
        let hashes = NoopHashProvider;
        let wad = MockWadProvider::empty();
        // foo.dds is dead everywhere, but the game has foo.tex.
        let game = MockGameProvider::new(&["assets/foo.tex"]);

        let tree = tree_with_string("assets/foo.dds");
        let mut ctx = base_ctx(tree, &hashes, &wad);
        ctx.game = Some(&game);

        let count = apply(&mut ctx, &extensions());

        assert_eq!(count, 1);
        assert_eq!(string_value(&ctx.tree), "assets/foo.tex");
    }

    #[test]
    fn rewrites_suffix_stripped_anm() {
        let hashes = NoopHashProvider;
        let wad = MockWadProvider::empty();
        let game = MockGameProvider::new(&["data/c/yone/anim/attack1.anm"]);

        let tree = tree_with_string("data/c/yone/anim/attack1.matcha_ambessa.anm");
        let mut ctx = base_ctx(tree, &hashes, &wad);
        ctx.game = Some(&game);

        let count = apply(&mut ctx, &extensions());

        assert_eq!(count, 1);
        assert_eq!(string_value(&ctx.tree), "data/c/yone/anim/attack1.anm");
    }

    #[test]
    fn noop_without_game() {
        let hashes = NoopHashProvider;
        let wad = MockWadProvider::empty();

        let tree = tree_with_string("assets/foo.dds");
        let mut ctx = base_ctx(tree, &hashes, &wad); // ctx.game == None

        let count = apply(&mut ctx, &extensions());

        assert_eq!(count, 0);
        assert_eq!(string_value(&ctx.tree), "assets/foo.dds");
    }

    #[test]
    fn ignores_unlisted_extensions() {
        let hashes = NoopHashProvider;
        let wad = MockWadProvider::empty();
        // Game even has a would-be resolution, but .bnk isn't configured.
        let game = MockGameProvider::new(&["assets/foo.bnk2"]);

        let tree = tree_with_string("assets/foo.bnk");
        let mut ctx = base_ctx(tree, &hashes, &wad);
        ctx.game = Some(&game);

        let count = apply(&mut ctx, &extensions());

        assert_eq!(count, 0);
        assert_eq!(string_value(&ctx.tree), "assets/foo.bnk");
    }
}
