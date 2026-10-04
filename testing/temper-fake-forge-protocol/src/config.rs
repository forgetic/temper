//! Provider spellings and fixed wire identities kept outside the fake domain.
use alloc::boxed::Box;
use skein_lib::bytes;
use temper_fake_forge_domain as domain;
use temper_forge_forgejo::{
    Limits as DocumentLimits,
    types::{Label, ObjectFormat, User},
};

#[expect(missing_debug_implementations, reason = "API tokens are secrets")]
pub struct Identity {
    pub id: u64,
    pub login: Box<[u8]>,
    pub token: Box<[u8]>,
}
#[derive(Debug)]
pub struct Repository {
    pub id: u64,
    pub name: Box<[u8]>,
    pub object_format: ObjectFormat,
    pub has_wiki: bool,
    pub labels: Box<[Label]>,
}
#[expect(missing_debug_implementations, reason = "configuration owns API tokens")]
pub struct Config {
    pub users: Box<[Identity]>,
    pub repositories: Box<[Repository]>,
    pub default_page: u32,
}
impl Config {
    #[must_use]
    pub fn valid(&self, limits: &DocumentLimits) -> bool {
        if self.users.is_empty()
            || self.users.len() > usize::try_from(limits.fields).expect("u32 fits")
            || self.repositories.len() > usize::try_from(limits.fields).expect("u32 fits")
            || self.default_page == 0
            || self.default_page > limits.page
        {
            return false;
        }
        for (index, user) in self.users.iter().enumerate() {
            if user.login.is_empty()
                || user.token.is_empty()
                || user.login.len() > usize::try_from(limits.name_bytes).expect("u32 fits")
                || user.token.len() > usize::try_from(limits.name_bytes).expect("u32 fits")
            {
                return false;
            }
            for old in self.users.iter().take(index) {
                if old.id == user.id || old.login == user.login || old.token == user.token {
                    return false;
                }
            }
        }
        for (index, repository) in self.repositories.iter().enumerate() {
            if repository.name.len() > usize::try_from(limits.name_bytes).expect("u32 fits")
                || repository.labels.len() > usize::try_from(limits.fields).expect("u32 fits")
            {
                return false;
            }
            for old in self.repositories.iter().take(index) {
                if old.id == repository.id || old.name == repository.name {
                    return false;
                }
            }
            for (index, label) in repository.labels.iter().enumerate() {
                if label.name.len() > usize::try_from(limits.name_bytes).expect("u32 fits") {
                    return false;
                }
                for old in repository.labels.iter().take(index) {
                    if old.id == label.id || old.name == label.name {
                        return false;
                    }
                }
            }
        }
        true
    }
    #[must_use]
    pub fn authenticate(&self, header: Option<&[u8]>) -> Option<u64> {
        let token = header?.strip_prefix(b"token ")?;
        for user in &self.users {
            if user.token.as_ref() == token {
                return Some(user.id);
            }
        }
        None
    }
    pub fn user(&self, id: u64) -> Result<User, domain::api::Error> {
        for user in &self.users {
            if user.id == id {
                return Ok(User { id, login: bytes::copy_of(&user.login) });
            }
        }
        Err(domain::api::Error::Forbidden)
    }
    pub fn user_id(&self, login: &[u8]) -> Result<u64, domain::api::Error> {
        for user in &self.users {
            if user.login.as_ref() == login {
                return Ok(user.id);
            }
        }
        Err(domain::api::Error::Forbidden)
    }
    pub fn repository(&self, name: &[u8]) -> Result<&Repository, domain::api::Error> {
        for repository in &self.repositories {
            if repository.name.as_ref() == name {
                return Ok(repository);
            }
        }
        Err(domain::api::Error::Missing(domain::api::What::Repository))
    }
}
