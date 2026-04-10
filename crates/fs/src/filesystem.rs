mod node;

use crate::filesystem::node::NodeMap;
use anyhow::Context as _;
use fuser::{
    BackgroundSession, Errno, FileHandle, FileType, Filesystem, INodeNo,
    LockOwner, MountOption, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEntry,
};
use slumber_core::{
    collection::{Collection, CollectionFile},
    database::CollectionDatabase,
};
use slumber_util::ResultTracedAnyhow;
use std::{
    borrow::Cow,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};
use tracing::info;

const TTL: Duration = Duration::from_secs(1);

/// A FUSE filesystem for a Slumber request collection
///
/// This mounts a virtual filesystem at the requested mount point with this
/// structure:
///
/// ```notrust
/// mount_dir/
///   slumber.yml
///   profiles/
///     profile1/
///       profile.yml
///       preview.yml
///   requests/
///     folder1/
///       request1/
///         recipe.yml
///         preview.yml
///         go
///         history/
///           20260228_112233_guid/
///             request_metadata.txt
///             request.json
///             response_metadata.txt
///             response.json
/// ```
#[derive(Debug)]
pub struct CollectionFilesystem {
    collection_file: CollectionFile,
    collection: Arc<Collection>,
    database: CollectionDatabase,
    /// Join handle for the filesystem's background thread
    handle: BackgroundSession,
    /// The path... where it's mounted...
    mount_path: PathBuf,
}

impl CollectionFilesystem {
    /// Build a new filesystem for a collection and mount it
    ///
    /// The FUSE server runs on a background thread. Use [Self::unmount] to
    /// stop the thread and unmount the filesystem.
    pub fn mount(
        collection_file: CollectionFile,
        database: CollectionDatabase,
        mount_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let collection = Arc::new(collection_file.load()?);
        let mount_path = env::current_dir()?.join(mount_path);
        let context = Context {
            // This all has to get cloned because it gets passed into the fs
            // impl, but we also hang onto them so they can be exposed for
            // other operations
            mount_path: mount_path.clone(),
            collection_file: collection_file.clone(),
            collection: Arc::clone(&collection),
            database: database.clone(),
        };
        let nodes = NodeMap::new(&context);
        let inner = FilesystemInner { context, nodes };

        let handle = inner.mount()?;

        Ok(Self {
            collection_file,
            collection,
            database,
            handle,
            mount_path,
        })
    }

    /// Stop the filesystem thread and unmount it
    ///
    /// Waits until the thread has been stopped.
    pub fn unmount(self) -> anyhow::Result<()> {
        self.handle.umount_and_join().with_context(|| {
            format!("Error unmounting filesystem {}", self.mount_path.display())
        })
    }

    pub fn collection_file(&self) -> &CollectionFile {
        &self.collection_file
    }

    pub fn collection(&self) -> &Arc<Collection> {
        &self.collection
    }

    pub fn database(&self) -> &CollectionDatabase {
        &self.database
    }
}

/// TODO
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

/// Internal implementation of the filesystem
///
/// `fuser` requires an owned object for the filesystem, so this is the value
/// passed along. Externally, [CollectionFilesystem] represents a mounted
/// filesystem.
struct FilesystemInner {
    /// Data passed to all fs operations
    context: Context,
    /// A map of all nodes in the filesystem, keyed by inode
    ///
    /// This is populated lazily as nodes are built out.
    nodes: NodeMap,
}

impl FilesystemInner {
    /// Mount the filesystem and spawn a background thread to run the server
    ///
    /// This returns a handle for the background thread. To unmount the
    /// filesystem and stop the thread, call
    /// [BackgroundSession::umount_and_join].
    fn mount(self) -> anyhow::Result<BackgroundSession> {
        let mount_path = self.context.mount_path.clone();
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

        // Spawn the fuse server in a background thread, since it's synchronous
        fuser::spawn_mount2(self, &mount_path, &config).with_context(|| {
            format!("Error mounting filesystem at {}", mount_path.display())
        })
    }
}

impl Filesystem for FilesystemInner {
    fn flush(
        &self,
        _req: &fuser::Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: LockOwner,
        reply: fuser::ReplyEmpty,
    ) {
        // Nothing to flush...
        reply.ok();
    }

    fn getattr(
        &self,
        _req: &fuser::Request,
        inode: INodeNo,
        _fh: Option<FileHandle>,
        reply: ReplyAttr,
    ) {
        let node = get_node!(self.nodes, inode, reply);
        reply.attr(&TTL, &node.attr(&self.context));
    }

    fn listxattr(
        &self,
        _req: &fuser::Request,
        inode: INodeNo,
        size: u32,
        reply: fuser::ReplyXattr,
    ) {
        let node = get_node!(self.nodes, inode, reply);

        // It'd be great if we could get file size without getting the full
        // content, but alas, I've built an inferior API
        let content = node.content(&self.context);
        let len = content.len() as u32;

        // See listxattr docs for the description of this behavior
        if size == 0 {
            reply.size(len);
        } else if len <= size {
            reply.data(&content);
        } else {
            reply.error(Errno::ERANGE);
        }
    }

    fn ioctl(
        &self,
        _req: &fuser::Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _flags: fuser::IoctlFlags,
        _cmd: u32,
        _in_data: &[u8],
        _out_size: u32,
        reply: fuser::ReplyIoctl,
    ) {
        // I don't really know what ioctl is meant to do but I don't feel like
        // implementing it.
        reply.error(Errno::EINVAL);
    }

    fn lookup(
        &self,
        _req: &fuser::Request,
        parent: INodeNo,
        name: &OsStr,
        reply: ReplyEntry,
    ) {
        // Find a node matching the given (parent, path)
        let node = self
            .nodes
            .children(parent)
            .find(|node| node.name(&self.context) == name);
        if let Some(node) = node {
            // TODO what is generation?
            reply.entry(&TTL, &node.attr(&self.context), fuser::Generation(0));
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
        let content = node.content(&self.context);
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
        // First, make sure the parent exists. Return an error if not
        get_node!(self.nodes, inode, reply);

        // Find all nodes with the given parent
        let children = self.nodes.children(inode);
        let entries = [
            (inode, FileType::Directory, Cow::Borrowed(".".as_ref())),
            // TODO is this inode correct?
            (inode, FileType::Directory, Cow::Borrowed("..".as_ref())),
        ]
        .into_iter()
        .chain(
            // Flatten into a tuple
            children.map(|node| {
                (node.inode, node.file_type(), node.name(&self.context))
            }),
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
        if let Some(link) = node.link(&self.context) {
            reply.data(link.as_os_str().as_encoded_bytes());
        } else {
            todo!("node wasn't a link")
        }
    }
}

/// Data available to all filesystem operations
#[derive(Debug)]
struct Context {
    /// Path where the filesystem is mounted
    mount_path: PathBuf,
    /// Path to the loaded collection file
    collection_file: CollectionFile,
    /// Loaded Slumber collection
    collection: Arc<Collection>,
    /// Loaded database for the collection
    database: CollectionDatabase,
}

/// TODO
fn unmount(path: &Path) -> anyhow::Result<()> {
    info!("Unmounting {}", path.display());
    Command::new("umount")
        // .arg("-l")
        .arg(path)
        .output()
        .and_then(|_| fs::remove_dir(path))
        .with_context(|| format!("Error unmounting {}", path.display()))?;
    Ok(())
}
