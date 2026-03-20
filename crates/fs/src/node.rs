//! Filesystem node definitions
//!
//! The primary type in this module is [NodeMap], which holds the structure of
//! the filesystem. The various node types are implemented in this module, but
//! they aren't exposed directory. The [Node] type exposes the functionality
//! implemented by each node type. The file structure looks like this:
//!
//! ```notrust
//! mount_dir/
//!   slumber.yml
//!   profiles/
//!     profile1/
//!       profile.yml
//!       preview.yml
//!   requests/
//!     folder1/
//!       request1/
//!         recipe.yml
//!         preview.yml
//!         go.sh
//!         history/
//!           2026-02-28T112233_guid/
//!             request.txt
//!             request_body.json
//!             response.txt
//!             response_body.json
//! ```

use crate::Context;
use bytes::Bytes;
use fuser::{Errno, FileAttr, FileType, INodeNo};
use slumber_core::{
    collection::{ProfileId, RecipeId, RecipeNode as RecipeTreeNode},
    database::ProfileFilter,
    http::RequestId,
};
use std::{
    borrow::Cow, collections::HashMap, ffi::OsStr, fmt::Debug, mem, path::Path,
    time::UNIX_EPOCH,
};

/// TODO
#[derive(Debug)]
pub struct NodeMap {
    /// TODO
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
    pub fn new(context: &Context) -> Self {
        let mut slf = Self {
            nodes: HashMap::new(),
            next_inode: INodeNo::ROOT,
        };

        // Build out the node tree
        // TODO do this lazily
        slf.add_recursive(None, RootDirectory.boxed(), context);

        slf
    }

    /// Get a node by inode
    ///
    /// If the node isn't in the filesystem, return [Errno::ENOENT]
    pub fn get(&self, inode: INodeNo) -> Result<&Node, Errno> {
        self.nodes.get(&inode).ok_or(Errno::ENOENT)
    }

    /// TODO
    pub fn children(
        &self,
        parent_inode: INodeNo,
    ) -> impl Iterator<Item = &Node> {
        self.nodes
            .values()
            .filter(move |node| node.parent == Some(parent_inode))
    }

    /// Add a node to the map, then recursively add all its descendants
    fn add_recursive(
        &mut self,
        parent: Option<INodeNo>,
        node: Box<dyn FileNode>,
        context: &Context,
    ) {
        let inode = self.next_inode();
        let children = node.children(context);
        let node = Node {
            inode,
            parent,
            kind: node,
        };
        self.nodes.insert(inode, node);
        for child in children {
            self.add_recursive(Some(inode), child, context);
        }
    }

    /// Get the next available inode
    fn next_inode(&mut self) -> INodeNo {
        let new = INodeNo(self.next_inode.0 + 1);
        mem::replace(&mut self.next_inode, new)
    }
}

/// An abstraction for the files and directories that can appear in the
/// filesystem
///
/// This uniquely defines a single file in the system with as little information
/// as possible. File metadata/contents are populated on demand based on the
/// collection and other external context.
#[derive(Debug)]
pub struct Node {
    /// Unique identifier for this node within the fs
    pub inode: INodeNo,
    /// Inode of the parent node in the fs
    ///
    /// `None` **only** for the root node
    parent: Option<INodeNo>,
    /// Behavior definition for this node
    /// TODO rename this
    kind: Box<dyn FileNode>,
}

impl Node {
    /// Get a node's name, i.e. the end of its path
    pub fn name<'a>(&'a self, context: &'a Context) -> Cow<'a, OsStr> {
        self.kind.name(context)
    }

    /// Get the node's type (file or directory)
    pub fn file_type(&self) -> FileType {
        self.kind.file_type()
    }

    /// Get a node's attributes
    pub fn attr(&self, context: &Context) -> FileAttr {
        let ino = self.inode;
        // This is inefficient, maybe we need to change it?
        let size = self.kind.content(context).len() as u64;
        FileAttr {
            ino,
            size,
            blocks: 0, // TODO set dynamically based on size?
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: self.kind.file_type(),
            perm: self.kind.permissions(),
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    /// Get the contents of a file node
    ///
    /// If the node is a directory, return empty bytes.
    pub fn content(&self, context: &Context) -> Bytes {
        self.kind.content(context)
    }

    /// Get the target for a symbol link
    ///
    /// If the node is not a link, return `None`.
    pub fn link<'a>(&'a self, context: &'a Context) -> Option<&'a Path> {
        self.kind.link(context)
    }
}

/// Behavior definition for a node type
///
/// This is used as a trait object within the file tree.
///
/// TODO rename this
trait FileNode: 'static + Debug + Send + Sync {
    /// Get a node's name, i.e. the end of its path
    fn name<'a>(&'a self, context: &'a Context) -> Cow<'a, OsStr>;

    /// Get the node's type (file or directory)
    fn file_type(&self) -> FileType;

    /// Get the node's permissions, as a 3-digit octal number `rwx/rwx/rwx`
    ///
    /// The default implementation works for all read-only directories and
    /// files. It needs to be overridden for writable files.
    fn permissions(&self) -> u16 {
        match self.file_type() {
            // Directories are readable and traversable
            FileType::Directory => 0o555,
            // Everything else is read-only by default
            _ => 0o444,
        }
    }

    /// Get the contents of a file node
    ///
    /// If the node is a directory, return empty bytes. This only needs to be
    /// overridden for files.
    fn content(&self, _context: &Context) -> Bytes {
        match self.file_type() {
            FileType::RegularFile => {
                unimplemented!("Regular files must implement content()")
            }
            _ => Bytes::new(),
        }
    }

    /// TODO
    fn children(&self, _context: &Context) -> Vec<Box<dyn FileNode>> {
        match self.file_type() {
            FileType::Directory => {
                unimplemented!("Directories must implement children()")
            }
            _ => vec![],
        }
    }

    /// Get the target for a symbol link
    ///
    /// Return `None` if the node is not a link. This only needs to be
    /// overridden for links.
    fn link<'a>(&'a self, _context: &'a Context) -> Option<&'a Path> {
        match self.file_type() {
            FileType::RegularFile => {
                unimplemented!("Symlinks must implement link()")
            }
            _ => None,
        }
    }

    /// Helper to box a statically typed `FileNode` implementor into a trait
    /// object
    fn boxed(self) -> Box<dyn FileNode>
    where
        Self: Sized,
    {
        Box::new(self)
    }
}

/// Root of the file system
#[derive(Debug)]
struct RootDirectory;

impl FileNode for RootDirectory {
    fn name<'a>(&'a self, context: &'a Context) -> Cow<'a, OsStr> {
        context.mount_path.file_name().unwrap_or("".as_ref()).into()
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, _context: &Context) -> Vec<Box<dyn FileNode>> {
        vec![
            CollectionLink.boxed(),
            ProfilesDirectory.boxed(),
            RecipesDirectory.boxed(),
        ]
    }
}

/// Link to the collection definition file
///
/// This is a symlink to the loaded `slumber.yml` file, wherever it is.
#[derive(Debug)]
struct CollectionLink;

impl FileNode for CollectionLink {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("slumber.yml")
    }

    fn file_type(&self) -> FileType {
        FileType::Symlink
    }

    fn link<'a>(&'a self, context: &'a Context) -> Option<&'a Path> {
        Some(context.collection_file.path())
    }
}

/// All profiles
#[derive(Debug)]
struct ProfilesDirectory;

impl FileNode for ProfilesDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("profiles")
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, context: &Context) -> Vec<Box<dyn FileNode>> {
        context
            .collection
            .profiles
            .values()
            .map(|profile| ProfileDirectory(profile.id.clone()).boxed())
            .collect()
    }
}

/// Files for a specific profile
#[derive(Debug)]
struct ProfileDirectory(ProfileId);

impl FileNode for ProfileDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow(&self.0)
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, _context: &Context) -> Vec<Box<dyn FileNode>> {
        vec![ProfileFile(self.0.clone()).boxed()]
    }
}

/// Definition of a profile as YAML
#[derive(Debug)]
struct ProfileFile(ProfileId);

impl FileNode for ProfileFile {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("profile.yml")
    }

    fn file_type(&self) -> FileType {
        FileType::RegularFile
    }

    fn content(&self, context: &Context) -> Bytes {
        // TODO use a snippet from the actual file instead of serializing?
        let profile = context
            .collection
            .profiles
            .get(&self.0)
            .expect("TODO error");
        serde_yaml::to_string(profile).unwrap().into()
    }
}

/// Root for all recipes/folders
#[derive(Debug)]
struct RecipesDirectory;

impl FileNode for RecipesDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("recipes")
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, context: &Context) -> Vec<Box<dyn FileNode>> {
        context
            .collection
            .recipes
            .tree()
            .values()
            .map(recipe_to_file)
            .collect()
    }
}

/// Subdirectory containing recipes and folders
#[derive(Debug)]
struct FolderDirectory(RecipeId);

impl FileNode for FolderDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow(&self.0)
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, context: &Context) -> Vec<Box<dyn FileNode>> {
        let folder = context
            .collection
            .recipes
            .get_folder(&self.0)
            .expect("TODO");
        folder.children.values().map(recipe_to_file).collect()
    }
}

/// Detailed files for a specific recipe
#[derive(Debug)]
struct RecipeDirectory(RecipeId);

impl FileNode for RecipeDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow(&self.0)
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, _context: &Context) -> Vec<Box<dyn FileNode>> {
        let recipe_id = &self.0;
        vec![
            RecipeFile(recipe_id.clone()).boxed(),
            RecipeSendFile(recipe_id.clone()).boxed(),
            RecipeHistoryDirectory(recipe_id.clone()).boxed(),
        ]
    }
}

/// Definition of a recipe as YAML
#[derive(Debug)]
struct RecipeFile(RecipeId);

impl FileNode for RecipeFile {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("recipe.yml")
    }

    fn file_type(&self) -> FileType {
        FileType::RegularFile
    }

    fn content(&self, context: &Context) -> Bytes {
        // TODO use a snippet from the actual file instead of serializing?
        let recipe = context
            .collection
            .recipes
            .get_recipe(&self.0)
            .expect("TODO error");
        serde_yaml::to_string(recipe).unwrap().into()
    }
}

/// Script to send a request for a specific recipe
#[derive(Debug)]
struct RecipeSendFile(RecipeId);

impl FileNode for RecipeSendFile {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("go.sh")
    }

    fn permissions(&self) -> u16 {
        0x555
    }

    fn file_type(&self) -> FileType {
        FileType::RegularFile
    }

    fn content(&self, context: &Context) -> Bytes {
        format!(
            "#!/bin/sh
slumber --file {collection} request {recipe_id}
",
            collection = context.collection_file.path().display(),
            recipe_id = self.0
        )
        .into()
    }
}

/// All past requests for a specific recipe
#[derive(Debug)]
struct RecipeHistoryDirectory(RecipeId);

impl FileNode for RecipeHistoryDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        to_cow("history")
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, context: &Context) -> Vec<Box<dyn FileNode>> {
        // TODO filter by active profile?
        let Ok(exchanges) = context
            .database
            .get_recipe_requests(ProfileFilter::All, &self.0)
        else {
            return vec![];
        };
        exchanges
            .into_iter()
            .map(|exchange| RecipeHistoryExchangeDirectory(exchange.id).boxed())
            .collect()
    }
}

/// Request/response for a single historical exchange
#[derive(Debug)]
struct RecipeHistoryExchangeDirectory(RequestId);

impl FileNode for RecipeHistoryExchangeDirectory {
    fn name<'a>(&'a self, _context: &'a Context) -> Cow<'a, OsStr> {
        // TODO include the timestamp in here
        Cow::Owned(self.0.to_string().into())
    }

    fn file_type(&self) -> FileType {
        FileType::Directory
    }

    fn children(&self, _context: &Context) -> Vec<Box<dyn FileNode>> {
        vec![]
    }
}

fn to_cow(s: &str) -> Cow<'_, OsStr> {
    Cow::Borrowed(s.as_ref())
}

/// Convert a recipe tree node to a file node
fn recipe_to_file(node: &RecipeTreeNode) -> Box<dyn FileNode> {
    match node {
        RecipeTreeNode::Folder(folder) => {
            FolderDirectory(folder.id.clone()).boxed()
        }
        RecipeTreeNode::Recipe(recipe) => {
            RecipeDirectory(recipe.id.clone()).boxed()
        }
    }
}
