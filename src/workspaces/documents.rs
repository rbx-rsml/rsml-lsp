use std::{collections::HashMap, ops::{Deref, DerefMut}, path::PathBuf, sync::Arc};

use tokio::sync::Mutex;

use crate::workspaces::Document;

pub struct Documents(HashMap<PathBuf, Arc<Mutex<Document>>>);

impl Deref for Documents {
    type Target = HashMap<PathBuf, Arc<Mutex<Document>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Documents {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Documents {
    pub fn new() -> Self {
        Self(HashMap::new())
    }
}