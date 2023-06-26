#![feature(io_error_other)]

pub mod id;
pub use id::{Resource, ResourceId, SpotifyId};

pub mod session;
pub use session::{Credentials, LoginCredentials};

pub mod metadata;

pub mod download;
