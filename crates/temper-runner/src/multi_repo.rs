//! Multi-repository runner wrappers over per-repository role and mechanical workers.

mod mechanical_worker;
mod report;
mod repository_set;
mod role_worker;

pub use mechanical_worker::MultiRepoMechanicalWorker;
pub use report::{
    MultiRepoConfigError, MultiRepoError, MultiRepoTickReport, RepositoryFailure,
    RepositoryJournal, RepositoryProgress,
};
pub use repository_set::{RepositorySet, RepositoryTarget};
pub use role_worker::MultiRepoRoleWorker;
