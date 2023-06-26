#![feature(io_error_other)]

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde::{de::DeserializeOwned, Serialize};
use spotdl::{
    metadata::{FsMetadataCache, MetadataFetcher},
    session::Session,
    Credentials, LoginCredentials, Resource, ResourceId,
};

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    subcmd: SubCmd,
}

#[derive(Debug, Parser)]
struct EnvCacheDir {
    #[clap(long, env = "SPOTDL_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct EnvAuth {
    #[clap(long, env = "SPOTDL_USER")]
    username: Option<String>,
    #[clap(long, env = "SPOTDL_PASS")]
    password: Option<String>,
}

#[derive(Debug, Parser)]
enum SubCmd {
    Login(LoginArgs),
    Logout(LogoutArgs),
    Info(InfoArgs),
    Download(DownloadArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ResourceKind {
    Artist,
    Album,
    Track,
    Playlist,
}

#[derive(Debug, Parser)]
struct ResourceSelector {
    /// Identifier kind.
    /// If not specified, it will be inferred from the identifier.
    /// It is only required if the identifier is ambiguous, for example, '5ujWPN9c4lh8pohnf4iVYK'.
    #[clap(long)]
    kind: Option<ResourceKind>,

    /// Identifier.
    /// This can be a Spotify URI, URL or ID.
    /// Ex:
    ///     - spotify:track:5ujWPN9c4lh8pohnf4iVYK
    ///     - https://open.spotify.com/track/5ujWPN9c4lh8pohnf4iVYK
    ///     - 5ujWPN9c4lh8pohnf4iVYK
    id: String,
}

#[derive(Debug, Parser)]
struct LoginArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    /// Spotify username
    username: String,

    /// Spotify password
    password: String,
}

#[derive(Debug, Parser)]
struct LogoutArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    username: String,
}

#[derive(Debug, Parser)]
struct InfoArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(flatten)]
    resource: ResourceSelector,
}

#[derive(Debug, Parser)]
struct DownloadArgs {
    #[clap(flatten)]
    env_cache_dir: EnvCacheDir,

    #[clap(flatten)]
    env_auth: EnvAuth,

    #[clap(flatten)]
    resource: ResourceSelector,
}

async fn subcmd_login(args: LoginArgs) -> Result<()> {
    let credentials_dir = helper_get_credentials_dir(&args.env_cache_dir)?;
    let credentials_path = credentials_dir.join(&args.username);
    let login_credentials = LoginCredentials {
        username: args.username,
        password: args.password,
    };
    let credentials = spotdl::session::login(&login_credentials)
        .await
        .context("logging in")?;
    tokio::fs::create_dir_all(&credentials_dir)
        .await
        .context("creating credentials directory")?;
    helper_write_json(&credentials_path, &credentials).await?;
    Ok(())
}

async fn subcmd_logout(args: LogoutArgs) -> Result<()> {
    let credentials_dir = helper_get_credentials_dir(&args.env_cache_dir)?;
    let credentials_path = credentials_dir.join(args.username);
    if credentials_path.exists() {
        tokio::fs::remove_file(&credentials_path)
            .await
            .context("removing credentials")?;
    }
    Ok(())
}

async fn subcmd_info(args: InfoArgs) -> Result<()> {
    let rid = helper_resource_selector_to_id(&args.resource)?;
    let credentials = helper_get_credentials(&args.env_cache_dir, &args.env_auth).await?;
    let session = Session::connect(credentials).await?;
    let cache = FsMetadataCache::new(helper_get_metadata_dir(&args.env_cache_dir)?);
    let fetcher = MetadataFetcher::new(session, cache);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    match rid.resource {
        Resource::Artist => {
            let artist = fetcher.get_artist(rid.id).await?;
            serde_json::to_writer_pretty(&mut stdout, &artist)?;
        }
        Resource::Album => {
            let album = fetcher.get_album(rid.id).await?;
            serde_json::to_writer_pretty(&mut stdout, &album)?;
        }
        Resource::Track => {
            let track = fetcher.get_track(rid.id).await?;
            serde_json::to_writer_pretty(&mut stdout, &track)?;
        }
        Resource::Playlist => {
            let playlist = fetcher.get_playlist(rid.id).await?;
            serde_json::to_writer_pretty(&mut stdout, &playlist)?;
        }
    }

    Ok(())
}

async fn subcmd_download(args: DownloadArgs) -> Result<()> {
    Ok(())
}

fn helper_resource_selector_to_id(selector: &ResourceSelector) -> Result<ResourceId> {
    let kind = match selector.kind {
        Some(ResourceKind::Artist) => Some(Resource::Artist),
        Some(ResourceKind::Album) => Some(Resource::Album),
        Some(ResourceKind::Track) => Some(Resource::Track),
        Some(ResourceKind::Playlist) => Some(Resource::Playlist),
        None => None,
    };
    spotdl::id::parse(&selector.id, kind).context("parsing resource id")
}

fn helper_get_cache_dir(dir: &EnvCacheDir) -> Result<PathBuf> {
    match dir.cache_dir {
        Some(ref dir) => Ok(dir.clone()),
        None => {
            let home = std::env::var("HOME").context("getting $HOME")?;
            let dir = PathBuf::from(home).join(".cache/spotdl");
            Ok(dir)
        }
    }
}

fn helper_get_credentials_dir(dir: &EnvCacheDir) -> Result<PathBuf> {
    let cache_dir = helper_get_cache_dir(dir)?;
    Ok(cache_dir.join("credentials"))
}

fn helper_get_metadata_dir(dir: &EnvCacheDir) -> Result<PathBuf> {
    let cache_dir = helper_get_cache_dir(dir)?;
    Ok(cache_dir.join("metadata"))
}

async fn helper_get_credentials(dir: &EnvCacheDir, auth: &EnvAuth) -> Result<Credentials> {
    let credentials_dir = helper_get_credentials_dir(dir)?;
    tokio::fs::create_dir_all(&credentials_dir)
        .await
        .context("creating credentials dir")?;
    if let Some(username) = auth.username.clone() {
        let credentials_path = credentials_dir.join(&username);
        if credentials_path.is_file() {
            let credentials = helper_read_json(&credentials_path)
                .await
                .context("reading credentials")?;
            return Ok(credentials);
        } else {
            let credentials = spotdl::session::login(&LoginCredentials {
                username: username.clone(),
                password: auth
                    .password
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("missing password"))?,
            })
            .await?;
            helper_write_json(&credentials_path, &credentials)
                .await
                .context("writing credentials")?;
            return Ok(credentials);
        }
    } else {
        for entry in credentials_dir.read_dir()? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let credentials = helper_read_json(&path)
                    .await
                    .context("reading credentials")?;
                return Ok(credentials);
            }
        }
    }

    Err(anyhow::anyhow!("no credentials found"))
}

async fn helper_read_json<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned,
{
    let json = tokio::fs::read_to_string(path)
        .await
        .context("reading json")?;
    let obj = serde_json::from_str(&json).context("deserializing json")?;
    Ok(obj)
}

async fn helper_write_json<T>(path: &Path, v: &T) -> Result<()>
where
    T: Serialize,
{
    let serialized = serde_json::to_string_pretty(v).context("serializing json")?;
    tokio::fs::write(path, serialized)
        .await
        .context("writing json")?;
    Ok(())
}

mod test {
    async fn entry() {}
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    match args.subcmd {
        SubCmd::Login(args) => subcmd_login(args).await?,
        SubCmd::Logout(args) => subcmd_logout(args).await?,
        SubCmd::Info(args) => subcmd_info(args).await?,
        SubCmd::Download(args) => subcmd_download(args).await?,
    };

    Ok(())
}
