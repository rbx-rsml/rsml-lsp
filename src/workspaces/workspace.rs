use rbx_rsml::typechecker::luaurc::Luaurc;

#[derive(Debug)]
pub struct Workspace {
    pub luaurc: Option<Luaurc>
}

impl Workspace {
    pub fn new(luaurc: Option<Luaurc>) -> Self {
        Self {
            luaurc
        }
    }
}