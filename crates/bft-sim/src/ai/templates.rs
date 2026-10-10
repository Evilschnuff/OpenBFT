//! The parentless gamez subtrees, and cloning one into the world.
//!
//! # Templates are spawned, marked and hidden
//!
//! The level spawner spawns every parentless subtree, deliberately: every node
//! becomes exactly one entity, so [`NodeIndex`](crate::level::NodeIndex) can
//! resolve any node by id or name for interp (docs/data/interp.md) and anim
//! (docs/data/anims.md), both of which address template nodes.
//!
//! So there is one hierarchy and one index;
//! [`spawn_level`](crate::level::spawn_level) marks the roots that are not the
//! World's subtree with [`Template`], and `bft-game` hides them. A template
//! is addressable by name, is not drawn, and is the thing this module clones
//! from. Nothing is unspawned and no second index exists.
//!
//! # What is in the map
//!
//! 174–220 root trees per level, of which the World is one. The rest are the
//! scene-setup singletons (`window1`, `camera1`, `display`), ~40 pickup
//! prototypes (`pu000`…), the enemy prototypes (`scou`, `rumv`, `ntank`, …)
//! and the effect templates (`redsprks.flt`, `md_impact.flt`, …). Their
//! subtrees are 25–49 nodes each.
//!
//! # A node id is a `u32`
//!
//! [`NodeIndex`](crate::level::NodeIndex) is the id→entity resource, and a
//! node id is a plain `u32` in the `NodeId` component; that is how
//! [`LevelTemplates`] and [`clone_template`] spell a root.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::geometry::triangulate;
use bft_assets::level::LevelData;

use crate::colliders::collider_from;
use crate::level::{ModelRef, NodeActive, NodeName, node_transform};

/// A root tree that is a prototype rather than part of the world.
///
/// Every root but the World's, which is 173–219 of a level's 174–220. The
/// marker is on the **root only**, not on every node of the subtree: hiding a
/// root hides its children through Bevy's inherited visibility, and a marker
/// on 4000 entities would cost more to query than it saves.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template;

/// An entity cloned from a template, and which template it came from.
///
/// Carried so a clone can be told from the prototype it was made from, and so
/// a debug view can say what a mystery entity in the world is.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct ClonedFrom(pub String);

/// Template name → the node id of its root.
///
/// Built from [`LevelData::root_nodes`] and the nodes' own names, which is
/// all it takes: a root that is not the World's is a template.
#[derive(Resource, Debug, Clone, Default)]
pub struct LevelTemplates {
    /// `"scou"`, `"pu000"`, `"he_morta.flt"` → node id.
    pub roots: HashMap<String, u32>,
}

impl LevelTemplates {
    /// Index a level's parentless subtrees.
    pub fn from_level(level: &LevelData) -> LevelTemplates {
        let mut roots = HashMap::default();

        for id in &level.root_nodes {
            if *id == level.world_node {
                continue;
            }
            let Some(node) = level.node(*id) else {
                continue;
            };

            // First wins, and one name really does repeat: **every level has
            // exactly two `rfpg_mzl.flt` roots**, the pulse gun's muzzle
            // flash. Same one-child shape, *different* model indices (m1:
            // 790 and 696), so they are two variants rather than a
            // duplicate — presumably the weak and strong gun's. Nothing
            // clones them through this map; `FIRE/ANIMATION` plays them,
            // and it needs both, so this map is the wrong home for them
            // and [`Self::all`] exists for when that is sorted out.
            //
            // Until then: first wins, and the map is one shorter than the
            // root list on all 13 levels. Saying so beats a silent
            // overwrite.
            roots.entry(node.name().to_string()).or_insert(*id);
        }

        LevelTemplates { roots }
    }

    /// The node id of a template by name.
    pub fn root(&self, name: &str) -> Option<u32> {
        self.roots.get(name).copied()
    }

    /// Every root of this name, for the one name that repeats.
    ///
    /// `rfpg_mzl.flt` is two roots with different models on every level (see
    /// [`Self::from_level`]); everything else is one. Takes the level back
    /// because the map keeps only the first — a second map for a single
    /// name that nothing clones yet would be carried on all 13 levels to no
    /// purpose.
    pub fn all(level: &LevelData, name: &str) -> Vec<u32> {
        level
            .root_nodes
            .iter()
            .copied()
            .filter(|id| *id != level.world_node)
            .filter(|id| level.node(*id).is_some_and(|node| node.name() == name))
            .collect()
    }

    /// Whether a template of this name exists.
    ///
    /// The check `spawn_enemy` makes before cloning: m1 has a `scou` template
    /// and no `htank` one, because a level only carries the prototypes its
    /// own `aiv.zrd` places.
    pub fn has(&self, name: &str) -> bool {
        self.roots.contains_key(name)
    }

    /// How many templates this level carries.
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

/// What a clone produced: the new root, and its nodes by name.
#[derive(Debug, Clone)]
pub struct ClonedTemplate {
    /// The root of the copy. Not parented to anything: the caller decides.
    pub root: Entity,
    /// Every cloned node's name → its entity, in subtree order.
    ///
    /// A `Vec` of pairs, not a map: template subtrees repeat names — a
    /// vehicle has several `collideNN` and two `l1` LOD nodes — and
    /// [`Self::find`] wants the first in tree order, which is the shallowest.
    pub nodes: Vec<(String, Entity)>,
}

impl ClonedTemplate {
    /// The shallowest node of this name in the copy.
    ///
    /// Breadth-first order, so `healthy/turret` is found before a
    /// `destroyed/turret` deeper in a sibling branch.
    pub fn find(&self, name: &str) -> Option<Entity> {
        self.nodes
            .iter()
            .find(|(node, _)| node == name)
            .map(|(_, entity)| *entity)
    }

    /// Every node of this name, shallowest first.
    pub fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Entity> + 'a {
        self.nodes
            .iter()
            .filter(move |(node, _)| node == name)
            .map(|(_, entity)| *entity)
    }

    /// How many nodes the copy has.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Copy a template subtree into the world at `at`.
///
/// The copy carries the same [`NodeName`]s, transforms and [`ModelRef`]s as
/// the prototype, so `bft-game` draws it with the machinery it already has,
/// but **not** the prototype's [`NodeId`](crate::level::NodeId): an id identifies one node of the
/// level and two entities claiming the same one would make
/// [`NodeIndex`](crate::level::NodeIndex) ambiguous for anim and interp. The
/// clone is addressed through [`ClonedTemplate::find`] instead.
///
/// Colliders are **not** copied either. A vehicle prototype's `collideNN`
/// children are the original's own collision proxies and the vehicle module gives a
/// vehicle one hull box from `vehicle.zrd`'s `collision` ring; spawning both
/// would put a trimesh inside a cuboid on the same body. The caller adds
/// whatever body it wants to the returned root. `with_colliders` is the way
/// in for a caller that does want them — a destructible prop, say.
///
/// `rename` becomes the root's [`Name`], so a cloned enemy shows up in an
/// inspector as `scou_03` rather than as a second `scou`.
pub fn clone_template(
    commands: &mut Commands,
    level: &LevelData,
    axes: &AxisConvention,
    root: u32,
    at: Transform,
    rename: &str,
) -> ClonedTemplate {
    clone_template_with(commands, level, axes, root, at, rename, false)
}

/// [`clone_template`], with the choice of copying the prototype's colliders.
pub fn clone_template_with(
    commands: &mut Commands,
    level: &LevelData,
    axes: &AxisConvention,
    root: u32,
    at: Transform,
    rename: &str,
    with_colliders: bool,
) -> ClonedTemplate {
    let Some(root_node) = level.node(root) else {
        warn!("no template node {root} in this level; cloning nothing");
        return ClonedTemplate {
            root: commands.spawn((Name::new(rename.to_string()), at)).id(),
            nodes: Vec::new(),
        };
    };

    let template_name = root_node.name().to_string();
    let root_entity = commands
        .spawn((
            Name::new(rename.to_string()),
            NodeName(template_name.clone()),
            ClonedFrom(template_name),
            // The caller's transform replaces the prototype's own, which is
            // wherever the build script parked it.
            at,
            // What the anim VM switches: a dead enemy's death animation
            // plays on its clone and turns `healthy` and `shadow` off.
            NodeActive(root_node.common().flags.active),
        ))
        .id();

    let mut cloned = ClonedTemplate {
        root: root_entity,
        nodes: vec![(root_node.name().to_string(), root_entity)],
    };

    // Breadth-first, so `ClonedTemplate::find` returns the shallowest match
    // and a parent's entity always exists before its children are spawned.
    let mut queue: std::collections::VecDeque<(u32, Entity)> = root_node
        .common()
        .children
        .iter()
        .map(|child| (*child, root_entity))
        .collect();

    while let Some((id, parent)) = queue.pop_front() {
        let Some(node) = level.node(id) else {
            continue;
        };
        let common = node.common();

        let mut entity = commands.spawn((
            NodeName(common.name.clone()),
            Name::new(common.name.clone()),
            node_transform(node, axes),
            ChildOf(parent),
            NodeActive(common.flags.active),
        ));

        if let Some(model_index) = common.model_index {
            entity.insert(ModelRef(model_index));

            if with_colliders
                && common.flags.collides()
                && let Some(collider) = level
                    .models
                    .get(model_index)
                    .and_then(|model| collider_from(model.model_type, &triangulate(model, axes)))
            {
                entity.insert(collider);
            }
        }

        let entity = entity.id();
        cloned.nodes.push((common.name.clone(), entity));

        for child in &common.children {
            queue.push_back((*child, entity));
        }
    }

    cloned
}

/// Whether a node id is inside the World's subtree.
///
/// Used by [`spawn_level`](crate::level::spawn_level) to decide which roots are
/// templates: a root that is not the World is one. The walk is over parents,
/// which is the edge set the loader trusts (docs/data/nodes.md), so it costs
/// one pass and no recursion.
pub fn template_roots(level: &LevelData) -> Vec<u32> {
    level
        .root_nodes
        .iter()
        .copied()
        .filter(|id| *id != level.world_node)
        .collect()
}

/// Hide every template root, so a prototype is not drawn in the world.
///
/// `bft-game`'s job, not this crate's: `Visibility` is a `bevy_render` type
/// and `bft-sim` builds without a renderer (NFR-4). The marker is here and
/// the hiding is there; this function only names what "hidden" means so the
/// two agree.
pub const TEMPLATES_ARE_HIDDEN: bool = true;

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::level::parse_gamez;
    use bft_assets::test_support::minimal_gamez;

    fn level() -> LevelData {
        parse_gamez(&minimal_gamez(), "test.gamez.zip").expect("the fixture loads")
    }

    /// The fixture is the World plus one child, so it has no templates. The
    /// check that matters is that the *World* is never one of them.
    #[test]
    fn the_world_tree_is_not_a_template() {
        let level = level();
        let templates = LevelTemplates::from_level(&level);

        assert!(
            !templates.has(level.node(level.world_node).expect("a world node").name()),
            "the world is the level, not a prototype"
        );
        assert!(
            template_roots(&level)
                .iter()
                .all(|id| *id != level.world_node)
        );
    }

    #[test]
    fn a_missing_template_is_reported_rather_than_assumed() {
        let templates = LevelTemplates::from_level(&level());

        assert!(!templates.has("scou"));
        assert_eq!(templates.root("scou"), None);
    }

    /// The find order is what a turret's part lookup depends on: `healthy`
    /// comes before `destroyed`, so `healthy/turret` wins over a `turret`
    /// buried under the wreck.
    #[test]
    fn find_returns_the_shallowest_match() {
        let cloned = ClonedTemplate {
            root: Entity::from_raw_u32(0).expect("a valid id"),
            nodes: vec![
                ("scou".to_string(), Entity::from_raw_u32(0).expect("id")),
                ("healthy".to_string(), Entity::from_raw_u32(1).expect("id")),
                (
                    "destroyed".to_string(),
                    Entity::from_raw_u32(2).expect("id"),
                ),
                ("turret".to_string(), Entity::from_raw_u32(3).expect("id")),
                ("turret".to_string(), Entity::from_raw_u32(4).expect("id")),
            ],
        };

        assert_eq!(cloned.find("turret"), Entity::from_raw_u32(3));
        assert_eq!(cloned.find_all("turret").count(), 2);
        assert_eq!(cloned.find("nowhere"), None);
    }
}
