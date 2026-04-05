use crate::node::NodeMap;
use anyhow::Context as _;
use fuser::{
    BackgroundSession, Errno, FileHandle, FileType, Filesystem, INodeNo,
    LockOwner, MountOption, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEntry,
};
use slumber_core::{
    collection::{Collection, CollectionFile},
    database::{CollectionDatabase, Database},
};
use slumber_util::ResultTracedAnyhow;
use std::{
    borrow::Cow,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tracing::info;

const TTL: Duration = Duration::from_secs(1);

/// TODO
#[derive(Debug)]
pub struct CollectionFilesystem {
    /// TODO
    context: Context,
    /// TODO
    nodes: NodeMap,
}

impl CollectionFilesystem {
    /// TODO
    pub fn new(
        collection_path: Option<PathBuf>,
        mount_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let collection_file = CollectionFile::new(collection_path)?;
        let collection = collection_file.load()?;
        let database = Database::load()?.into_collection(&collection_file)?;
        let mount_path = env::current_dir()?.join(mount_path);
        let context = Context {
            mount_path,
            collection_file,
            collection,
            database,
        };
        let nodes = NodeMap::new(&context);
        Ok(Self { context, nodes })
    }

    /// Spawn the filesystem in a background thread
    ///
    /// This returns a handle for the background thread. To unmount the
    /// filesystem and stop the thread, just drop the handle.
    pub fn spawn(self) -> anyhow::Result<BackgroundSession> {
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

impl Filesystem for CollectionFilesystem {
    fn flush(
        &self,
        _req: &fuser::Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: LockOwner,
        _reply: fuser::ReplyEmpty,
    ) {
        // Nothing to flush...
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
pub struct Context {
    /// Path where the filesystem is mounted
    pub mount_path: PathBuf,
    /// Path to the loaded collection file
    pub collection_file: CollectionFile,
    /// Loaded Slumber collection
    pub collection: Collection,
    /// Loaded database for the collection
    pub database: CollectionDatabase,
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
