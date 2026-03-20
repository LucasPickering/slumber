//! Filesystem node definitions

use fuser::{Errno, FileType, INodeNo};
use indexmap::IndexMap;
use slumber_core::{
    collection::{
        Collection, Profile, ProfileId, Recipe, RecipeId, RecipeNode,
    },
    http::RequestId,
};
use std::{collections::HashMap, mem};

/// An abstraction for the files and directories that can appear in the
/// filesystem
///
/// This uniquely defines a single file in the system with as little information
/// as possible. File metadata/contents are populated on demand based on the
/// collection and other external context.
#[derive(Debug)]
pub struct Node {
    /// TODO
    pub inode: INodeNo,
    /// TODO
    pub parent: Option<INodeNo>,
    /// TODO
    pub kind: NodeKind,
}

impl Node {
    /// Get the node's type (file or directory)
    pub fn file_type(&self) -> FileType {
        match &self.kind {
            NodeKind::Root
            | NodeKind::Profiles
            | NodeKind::Profile(_)
            | NodeKind::Recipes
            | NodeKind::Folder(_)
            | NodeKind::Recipe(_)
            | NodeKind::RecipeHistory(_)
            | NodeKind::RecipeHistoryExchange(_, _) => FileType::Directory,
            NodeKind::ProfileDefinition(_)
            | NodeKind::RecipeDefinition(_)
            | NodeKind::RecipeSend(_) => FileType::RegularFile,
            NodeKind::CollectionFile => FileType::Symlink,
        }
    }
}

/// TODO
#[derive(Debug)]
pub enum NodeKind {
    /// Root directory
    Root,
    /// Collection definition file. This is a symlink to the actual file
    CollectionFile,
    /// Directory containing all profiles
    Profiles,
    /// Directory with detailed files for a specific profile
    Profile(ProfileId),
    /// Definition of a profile as YAML
    ProfileDefinition(ProfileId),
    /// Root directory for all recipes/folders
    Recipes,
    /// Subdirectory containing recipes and folders
    Folder(RecipeId),
    /// Directory with detailed files for a specific recipe
    Recipe(RecipeId),
    /// Definition of a recipe as YAML
    RecipeDefinition(RecipeId),
    /// A script to send a request
    RecipeSend(RecipeId),
    /// Directory containing all past requests for a recipe
    RecipeHistory(RecipeId),
    /// Directory with request/response for a single historical exchange
    RecipeHistoryExchange(RecipeId, RequestId),
}

/// TODO
#[derive(Debug)]
pub struct NodeMap {
    nodes: HashMap<INodeNo, Node>,
    /// The next inode that has not been used yet
    ///
    /// This will be assigned to the next new node
    next_inode: INodeNo,
}

impl NodeMap {
    /// Build a virtual version of the filesystem. This will construct the
    /// general layout of the fs, but most of the metadata/data will be provided
    /// lazily upon request
    pub fn new(collection: &Collection) -> Self {
        let mut slf = Self {
            nodes: HashMap::new(),
            next_inode: INodeNo::ROOT,
        };

        // Insert root specially because it doesn't have a parent. This saves
        // us from having to make parent optional in add(), which would add a
        // ton of annoying Some() wrappers
        let root_inode = slf.next_inode();
        slf.nodes.insert(
            root_inode,
            Node {
                inode: root_inode,
                parent: None,
                kind: NodeKind::Root,
            },
        );

        slf.add(root_inode, NodeKind::CollectionFile);
        slf.add_profiles(root_inode, &collection.profiles);
        let recipes_inode = slf.add(root_inode, NodeKind::Recipes);
        slf.add_recipes(recipes_inode, collection.recipes.tree());

        slf
    }

    /// Get an iterator over all nodes
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Get a node by inode
    ///
    /// If the node isn't in the filesystem, return [Errno::ENOENT]
    pub fn get(&self, inode: INodeNo) -> Result<&Node, Errno> {
        self.nodes.get(&inode).ok_or(Errno::ENOENT)
    }

    /// Add all profiles to the map
    fn add_profiles(
        &mut self,
        root_inode: INodeNo,
        profiles: &IndexMap<ProfileId, Profile>,
    ) {
        // Add profiles
        let profiles_inode = self.add(root_inode, NodeKind::Profiles);
        for profile in profiles.values() {
            // Create a subdir with files for each profile
            let dir_inode =
                self.add(profiles_inode, NodeKind::Profile(profile.id.clone()));
            self.add(
                dir_inode,
                NodeKind::ProfileDefinition(profile.id.clone()),
            );
        }
    }

    /// Add the recipe tree to the map recursively
    fn add_recipes(
        &mut self,
        parent_inode: INodeNo,
        recipes: &IndexMap<RecipeId, RecipeNode>,
    ) {
        for recipe_node in recipes.values() {
            match recipe_node {
                RecipeNode::Folder(folder) => {
                    let folder_inode = self
                        .add(parent_inode, NodeKind::Folder(folder.id.clone()));
                    // Recursion!!
                    self.add_recipes(folder_inode, &folder.children);
                }
                RecipeNode::Recipe(recipe) => {
                    self.add_recipe(parent_inode, recipe);
                }
            }
        }
    }

    /// Add a single recipe to the map
    fn add_recipe(&mut self, parent_inode: INodeNo, recipe: &Recipe) {
        let id = &recipe.id;
        let recipe_inode = self.add(parent_inode, NodeKind::Recipe(id.clone()));
        self.add(recipe_inode, NodeKind::RecipeDefinition(id.clone()));
        self.add(recipe_inode, NodeKind::RecipeSend(id.clone()));
        self.add(recipe_inode, NodeKind::RecipeHistory(id.clone()));
    }

    /// Add a node to the map with a new inode
    ///
    /// Return the assigned inode.
    fn add(&mut self, parent: INodeNo, kind: NodeKind) -> INodeNo {
        let inode = self.next_inode();
        let node = Node {
            inode,
            parent: Some(parent),
            kind,
        };
        self.nodes.insert(inode, node);
        inode
    }

    /// Get the next available inode
    fn next_inode(&mut self) -> INodeNo {
        let new = INodeNo(self.next_inode.0 + 1);
        mem::replace(&mut self.next_inode, new)
    }
}

/// TODO
#[macro_export]
macro_rules! get_node {
    ($map:expr, $inode:expr, $reply:expr) => {
        match $map.get($inode) {
            Ok(node) => node,
            Err(error) => {
                $reply.error(error);
                return;
            }
        }
    };
}
