//! TODO
//!
//! The file structure looks like this:
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
//!         preview.txt
//!         go.sh
//!         history/
//!           2026-02-28T112233_request_guid/
//!             request.txt
//!             request_body.json
//!             response.txt
//!             response_body.json
//! ```

mod node;

use crate::node::{Node, NodeKind, NodeMap};
use anyhow::Context;
use bytes::Bytes;
use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, INodeNo, LockOwner,
    MountOption, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
};
use slumber_core::collection::{Collection, CollectionFile};
use slumber_util::ResultTracedAnyhow;
use std::{
    borrow::Cow, env, ffi::{OsStr, OsString}, fs, path::{Path, PathBuf}, process::Command, time::{Duration, UNIX_EPOCH}
};
use tracing::info;

const TTL: Duration = Duration::from_secs(1);

/// TODO
/// TODO better name
#[derive(Debug)]
pub struct SlumberFs {
    /// TODO
    mount_path: PathBuf,
    /// TODO
    collection_file: CollectionFile,
    /// TODO
    collection: Collection,
    /// TODO
    nodes: NodeMap,
}

impl SlumberFs {
    /// TODO
    pub fn run(
        collection_path: Option<PathBuf>,
        mount_path: PathBuf,
    ) -> anyhow::Result<()> {
        let collection_file = CollectionFile::new(collection_path)?;
        let collection = collection_file.load()?;
        let mount_path = env::current_dir()?.join(mount_path);
        let nodes = NodeMap::new(&collection);
        let filesystem = Self {
            mount_path: mount_path.clone(),
            collection_file,
            collection,
            nodes,
        };

        info!(?mount_path, "Starting filesystem server");

        // Unmount if already existing
        // TODO check if it exists before calling this
        let _ = unmount(&mount_path).traced();

        // Mount point has to be created first
        fs::create_dir_all(&mount_path).with_context(|| {
            format!(
                "Error creating mount point {path}. \
                If it already exists, unmount it first with `umount {path}`",
                path = mount_path.display()
            )
        })?;

        let mut config = fuser::Config::default();
        config.mount_options.push(MountOption::DefaultPermissions);
        fuser::mount2(filesystem, &mount_path, &config).with_context(|| {
            format!("Error mounting filesystem at {}", mount_path.display())
        })
    }

    /// Get a node's name, i.e. the end of its path
    fn name<'a>(&'a self, node: &'a Node) -> Cow<'a, OsStr> {
        fn cow(s:&str) -> Cow<'_,OsStr> {
            Cow::Borrowed(s.as_ref())
        }
        match &node.kind {
            NodeKind::Root => {
                self.mount_path.file_name().unwrap_or("".as_ref()).into()
            }
            NodeKind::CollectionFile => cow("slumber.yml"),
            NodeKind::Profiles => cow("profiles"),
            NodeKind::Profile(profile_id) => cow(profile_id),
            NodeKind::ProfileDefinition(_) => cow("profile.yml"),
            NodeKind::Recipes => cow("recipes"),
            NodeKind::Folder(recipe_id) => cow(recipe_id),
            NodeKind::Recipe(recipe_id) => cow(recipe_id),
            NodeKind::RecipeDefinition(_) => cow("recipe.yml"),
            NodeKind::RecipeSend(_) => cow("go.sh"),
            NodeKind::RecipeHistory(_) => cow("history"),
            // TODO include date/time in name
            NodeKind::RecipeHistoryExchange(_, request_id) => {
                OsString::from(request_id.to_string()).into()
            }
        }
    }

    /// Get a node's attributes
    fn attr(&self, node: &Node) -> FileAttr {
        let ino = node.inode;
        // This may be inefficient for some files, but in most cases the only
        // way to get the length is to generate the full content.
        let size = self.content(node).len() as u64;

        match &node.kind {
            // All directories have the same attributes. Only files can vary
            NodeKind::Root
            | NodeKind::Profiles
            | NodeKind::Profile(_)
            | NodeKind::Recipes
            | NodeKind::Folder(_)
            | NodeKind::Recipe(_)
            | NodeKind::RecipeHistory(_)
            | NodeKind::RecipeHistoryExchange(_, _) => FileAttr {
                // TODO fix all of these
                ino,
                size,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: node.file_type(),
                perm: 0o755,
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
            NodeKind::CollectionFile => FileAttr {
                // TODO fix these
                ino,
                size,
                blocks: 0, // TODO set this based on size?
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: node.file_type(),
                perm: 0o644,
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
            NodeKind::ProfileDefinition(_) => FileAttr {
                // TODO fix these
                ino,
                size,
                blocks: 0, // TODO set this based on size?
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: node.file_type(),
                perm: 0o644,
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
            NodeKind::RecipeDefinition(_) => FileAttr {
                // TODO fix these
                ino,
                size,
                blocks: 0, // TODO set this based on size?
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: node.file_type(),
                perm: 0o644,
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
            NodeKind::RecipeSend(_) => FileAttr {
                // TODO fix these
                ino,
                size,
                blocks: 0, // TODO set this based on size?
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: node.file_type(),
                perm: 0o755,
                nlink: 1,
                uid: 501,
                gid: 20,
                rdev: 0,
                flags: 0,
                blksize: 512,
            },
        }
    }

    /// Get the target for a symbol link
    ///
    /// Return `None` if the node is not a link
    fn link(&self, node: &Node) -> Option<&Path> {
        match &node.kind {
            NodeKind::CollectionFile => Some(self.collection_file.path()),
            // There aren't many link types, so this saves having to add here
            // for every new node kind
            _ => None,
        }
    }

    /// Get the contents of a file node
    ///
    /// If the node is a directory, return empty bytes
    fn content(&self, node: &Node) -> Bytes {
        match &node.kind {
            // TODO these should use the actual file contents?
            NodeKind::ProfileDefinition(profile_id) => {
                let profile = self
                    .collection
                    .profiles
                    .get(profile_id)
                    .expect("TODO error");
                serde_yaml::to_string(profile).unwrap().into()
            }
            NodeKind::RecipeDefinition(recipe_id) => {
                let recipe = self
                    .collection
                    .recipes
                    .get_recipe(recipe_id)
                    .expect("TODO error");
                serde_yaml::to_string(recipe).unwrap().into()
            }
            NodeKind::RecipeSend(recipe_id) => {
                // TODO include profile
                // TODO make this persist
                format!("#!/bin/sh
slumber --file {collection} request {recipe_id}
", collection = self.collection_file.path().display()).into()
            }
            NodeKind::Root
            | NodeKind::CollectionFile // This is a symlink, it has no contents
            | NodeKind::Profiles
            | NodeKind::Profile(_)
            | NodeKind::Recipes
            | NodeKind::Folder(_)
            | NodeKind::Recipe(_) 
            | NodeKind::RecipeHistory(_)
            | NodeKind::RecipeHistoryExchange(_, _) => Bytes::new(),
        }
    }
}

impl Filesystem for SlumberFs {
    fn getattr(
        &self,
        _req: &fuser::Request,
        inode: INodeNo,
        _fh: Option<FileHandle>,
        reply: ReplyAttr,
    ) {
        let node = get_node!(self.nodes, inode, reply);
        reply.attr(&TTL, &self.attr(node));
    }

    fn lookup(
        &self,
        _req: &fuser::Request,
        parent: INodeNo,
        name: &OsStr,
        reply: ReplyEntry,
    ) {
        // Find a node matching the given (parent, path)
        let node = self.nodes.iter().find(|node| {
            node.parent == Some(parent) && self.name(node) == name
        });
        if let Some(node) = node {
            // TODO what is generation?
            reply.entry(&TTL, &self.attr(node), fuser::Generation(0));
        } else {
            reply.error(Errno::ENOENT);
        }
    }

    fn read(
        &self,
        _req: &fuser::Request,
        inode: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let node = get_node!(self.nodes, inode, reply);
        // TODO do we need to make sure it's not a dir?
        let content = self.content(node);
        let start = (offset as usize).min(content.len());
        let end = (start + (size as usize)).min(content.len());
        reply.data(&content[start..end]);
    }

    fn readdir(
        &self,
        _req: &fuser::Request,
        inode: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        // If the parent doesn't exist, return an error instead of empty
        get_node!(self.nodes, inode, reply);

        // Find all nodes with the given parent
        let children =
            self.nodes.iter().filter(|node| node.parent == Some(inode));
        let entries = [
            (inode, FileType::Directory, Cow::Borrowed(".".as_ref())),
            // TODO is this inode correct?
            (inode, FileType::Directory, Cow::Borrowed("..".as_ref())),
        ]
        .into_iter()
        .chain(
            // Flatten into a tuple
            children
                .map(|node| (node.inode, node.file_type(), self.name(node))),
        )
        .enumerate()
        .skip(offset as usize);
        for (i, (inode, file_type, name)) in entries {
            // offset is the index of the *next* entry
            let offset = i as u64 + 1;
            let full = reply.add(inode, offset, file_type, name);
            if full {
                break; // Caller doesn't want us to add any more
            }
        }
        reply.ok();
    }

    fn readlink(
        &self,
        _req: &fuser::Request,
        inode: INodeNo,
        reply: ReplyData,
    ) {
        let node = get_node!(self.nodes, inode, reply);
        if let Some(link) = self.link(node) {
            reply.data(link.as_os_str().as_encoded_bytes());
        } else {
            todo!("node wasn't a link")
        }
    }
}

impl Drop for SlumberFs {
    fn drop(&mut self) {
        // Unmount on exit
        let _ = unmount(&self.mount_path).traced();
    }
}

/// TODO
fn unmount(path: &Path) -> anyhow::Result<()> {
    info!("Unmounting {}", path.display());
    Command::new("umount")
        .arg(path)
        .output()
        .with_context(|| format!("Error unmounting {}", path.display()))?;
    Ok(())
}
