use std::{collections::HashMap};
use serde::{Deserialize};

#[derive(Deserialize)]
pub struct Luaurc {
    pub aliases: HashMap<String, String>
}

impl Luaurc {
    pub fn new(contents: &str) -> Self {
        serde_json::from_str::<Luaurc>(&contents)
            .unwrap_or_else(|_| Self { aliases: HashMap::new() })
    }
}