use crate::{git_tree::Result, mount::mount_rofs_handwritten, GitTreeView, RofsMount};
use std::path::Path;

/// Mount through the strict read-only FUSE backend selected by `KAGE_ROFS_BACKEND=fuser`.
///
/// The repository does not vendor the external `fuser` crate yet, so this route delegates to
/// kage-rofs' in-tree FUSE server instead of returning the previous offline-workspace stub error.
/// This keeps strict CI on a real kernel FUSE read-only mount while the crate-backed backend is
/// evaluated separately.
pub fn mount_rofs_fuser(view: &GitTreeView, mountpoint: &Path) -> Result<RofsMount> {
    eprintln!(
        "kage-rofs fuser backend: using in-tree FUSE implementation; external fuser crate backend is not wired yet"
    );
    mount_rofs_handwritten(view, mountpoint)
}
