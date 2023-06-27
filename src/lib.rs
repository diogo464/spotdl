#![feature(io_error_other)]
#![feature(io_error_more)]
#![feature(try_blocks)]

pub mod id;
pub use id::{Resource, ResourceId, SpotifyId};

pub mod session;
pub use session::{Credentials, LoginCredentials};

pub mod metadata;

pub mod download;

pub mod pipeline;

pub mod scan;

pub mod tag;
