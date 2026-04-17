use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rbx_rsml::typechecker::{Definitions, TokenTypes};

pub struct Document {
    pub source: String,
    pub dependencies: HashSet<PathBuf>,
    pub definitions: Definitions,
    pub token_types: TokenTypes,
}

impl Document {
    pub fn new(source: String) -> Self {
        Self {
            source,
            dependencies: HashSet::new(),
            definitions: Definitions::new(),
            token_types: HashMap::new(),
        }
    }
}
