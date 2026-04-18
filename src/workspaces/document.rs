use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rbx_rsml::typechecker::{Definitions, ResolvedTypes};

pub struct Document {
    pub source: String,
    pub dependencies: HashSet<PathBuf>,
    pub definitions: Definitions,
    pub resolved_types: ResolvedTypes,
}

impl Document {
    pub fn new(source: String) -> Self {
        Self {
            source,
            dependencies: HashSet::new(),
            definitions: Definitions::new(),
            resolved_types: HashMap::new(),
        }
    }
}
