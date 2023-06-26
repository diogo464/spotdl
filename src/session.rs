use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("Login failed")]
pub struct LoginError;

#[derive(Debug, Error)]
#[error("Session error: {0}")]
pub struct SessionError(Box<dyn std::error::Error + Send + Sync + 'static>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials(librespot::discovery::Credentials);

#[derive(Clone)]
pub struct Session {
    session: librespot::core::Session,
    cache_dir: Arc<tempfile::TempDir>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Session").finish()
    }
}

impl Session {
    pub async fn connect(credentials: Credentials) -> Result<Session, SessionError> {
        let tempdir = tempfile::TempDir::new().map_err(|err| {
            tracing::error!("failed to create tempdir: {}", err);
            SessionError(Box::new(err))
        })?;
        let cache = librespot::core::cache::Cache::new(Some(tempdir.path()), None, None, None)
            .map_err(|err| {
                tracing::error!("failed to create cache: {}", err);
                SessionError(Box::new(err))
            })?;
        let session = librespot::core::Session::new(Default::default(), Some(cache));
        session
            .connect(credentials.0, false)
            .await
            .map_err(|err| SessionError(Box::new(err)))?;
        Ok(Session {
            session,
            cache_dir: Arc::new(tempdir),
        })
    }

    pub(crate) fn librespot(&self) -> &librespot::core::Session {
        &self.session
    }
}

pub async fn login(credentials: &LoginCredentials) -> Result<Credentials, LoginError> {
    let credentials = librespot::discovery::Credentials::with_password(
        credentials.username.clone(),
        credentials.password.clone(),
    );

    let tempdir = tempfile::TempDir::new().map_err(|err| {
        tracing::error!("failed to create tempdir: {}", err);
        LoginError
    })?;

    let cache = librespot::core::cache::Cache::new(Some(tempdir.path()), None, None, None)
        .map_err(|err| {
            tracing::error!("failed to create cache: {}", err);
            LoginError
        })?;

    let session = librespot::core::Session::new(Default::default(), Some(cache));
    session.connect(credentials, true).await.map_err(|err| {
        tracing::error!("failed to connect: {}", err);
        LoginError
    })?;

    let credentials = session
        .cache()
        .expect("this session has a cache")
        .credentials()
        .ok_or_else(|| {
            tracing::error!("failed to get credentials from cache, they should be there");
            LoginError
        })?;

    Ok(Credentials(credentials))
}
