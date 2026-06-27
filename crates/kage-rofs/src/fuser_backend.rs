use crate::{GitTreeView, Result, RofsMount};
use std::path::Path;

pub fn mount_rofs_fuser(_view: &GitTreeView, _mountpoint: &Path) -> Result<RofsMount> {
    Err("fuser backend selected but the fuser crate is not available in this offline CI workspace; error_kind=fuser_backend_unavailable".into())
}
