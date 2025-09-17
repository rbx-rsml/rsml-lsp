use std::{collections::HashMap, ops::{Deref, DerefMut}, path::{Path, PathBuf}, sync::Arc};
use guarded::guarded_unwrap;
use tokio::sync::Mutex;

mod documents;
pub use documents::*;

mod document;
pub use document::*;

mod workspace;
pub use workspace::Workspace;

use crate::luaurc::Luaurc;

#[derive(Debug)]
pub struct Workspaces(HashMap<PathBuf, Arc<Mutex<Workspace>>>);

impl Deref for Workspaces {
    type Target = HashMap<PathBuf, Arc<Mutex<Workspace>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Workspaces {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Workspaces {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub async fn remove_luaurc_for_workspace(&mut self, workspace_path: &Path) -> Option<Luaurc> {
        let workspace = guarded_unwrap!(self.get(workspace_path), return None);

        workspace.lock().await.luaurc.take()
    }

    pub async fn set_luaurc_for_workspace(&mut self, workspace_path: &Path, luaurc: Luaurc) -> Option<Luaurc> {
        let workspace = guarded_unwrap!(self.get(workspace_path), return None);

        workspace.lock().await.luaurc.replace(luaurc)
    }

    /*pub fn get_for_path(&self, path: &Path) -> Option<Arc<Mutex<Documents>>> {
        match self.iter().find(|(x, _)| path.starts_with(x)) {
            Some(workspace) => Some(workspace.1.clone()),
            None => None
        }
    }*/
}