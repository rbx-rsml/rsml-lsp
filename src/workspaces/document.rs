use std::collections::HashSet;
use std::path::PathBuf;

use rbx_rsml::typechecker::Definitions;

pub struct Document {
    pub source: String,
    pub dependencies: HashSet<PathBuf>,
    pub definitions: Definitions,
}

impl Document {
    pub fn new(source: String) -> Self {
        Self {
            source,
            dependencies: HashSet::new(),
            definitions: Definitions::new(),
        }
    }
}
