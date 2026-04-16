use std::path::{Component, Path, PathBuf};

pub trait NormalizePath {
    fn normalize(&self) -> PathBuf;
}

impl<T: AsRef<Path>> NormalizePath for T {
    fn normalize(&self) -> PathBuf {
        let mut components = self.as_ref().components().peekable();
        let mut result = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
            components.next();
            PathBuf::from(c.as_os_str())
        } else {
            PathBuf::new()
        };

        for component in components {
            match component {
                Component::Prefix(..) => unreachable!(),
                Component::RootDir => {
                    result.push(std::path::MAIN_SEPARATOR.to_string());
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if result.ends_with("..") {
                        result.push("..");
                    } else {
                        let popped = result.pop();
                        if !popped && !result.has_root() {
                            result.push("..");
                        }
                    }
                }
                Component::Normal(c) => {
                    result.push(c);
                }
            }
        }
        result
    }
}
