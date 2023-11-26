#![feature(io_error_more)]
#![feature(try_blocks)]
#![feature(let_chains)]

pub mod id;
pub use id::{Resource, ResourceId, SpotifyId};

pub mod session;
pub use session::{Credentials, LoginCredentials};

pub mod metadata;

pub mod fetcher;

pub mod download;

pub mod pipeline;

pub mod scan;

pub mod tag;
