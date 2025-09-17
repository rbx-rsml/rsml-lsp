use std::{collections::HashSet, path::PathBuf};

use crate::typechecker::Definitions;

pub struct Document {
    pub source: String,
    pub dependencies: HashSet<PathBuf>,
    pub definitions: Definitions
}

impl Document {
    pub fn new(source: String) -> Self {
        Self {
            source,
            dependencies: HashSet::new(),
            definitions: Definitions::new()
        }
    }
}