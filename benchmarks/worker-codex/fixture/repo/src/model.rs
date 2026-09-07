#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Policy {
    pub required: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decision {
    pub accepted: bool,
    pub missing: Vec<String>,
}
