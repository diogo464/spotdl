use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use crate::{id, tag, SpotifyId};

const EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a"];

#[derive(Debug, Clone, Default)]
pub struct ScanParams {
    pub include: Vec<PathBuf>,
    pub exclude: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ScanItem {
    pub path: PathBuf,
    pub id: SpotifyId,
}

impl ScanParams {
    pub fn include(&mut self, path: impl Into<PathBuf>) {
        self.include.push(path.into());
    }

    pub fn exclude(&mut self, path: impl Into<PathBuf>) {
        self.exclude.push(path.into());
    }

    pub fn with_include(self, path: impl Into<PathBuf>) -> Self {
        let mut this = self;
        this.include.push(path.into());
        this
    }

    pub fn with_exclude(self, path: impl Into<PathBuf>) -> Self {
        let mut this = self;
        this.exclude.push(path.into());
        this
    }

    pub fn should_exclude(&self, path: &Path) -> bool {
        let path_canon = match path.canonicalize() {
            Ok(path) => path,
            Err(_) => {
                tracing::warn!("failed to canonicalize path '{}'", path.display());
                return true;
            }
        };
        for exclude in &self.exclude {
            let exclude_canon = match exclude.canonicalize() {
                Ok(_) => exclude,
                Err(_) => {
                    tracing::warn!(
                        "failed to canonicalize exclude path '{}'",
                        exclude.display()
                    );
                    continue;
                }
            };
            if path_canon.starts_with(exclude_canon) {
                return true;
            }
        }
        false
    }
}

pub async fn scan(dir: impl Into<PathBuf>) -> std::io::Result<Vec<ScanItem>> {
    scan_with(ScanParams::default().with_include(dir)).await
}

pub async fn scan_with(mut params: ScanParams) -> std::io::Result<Vec<ScanItem>> {
    // TODO: this does not handle loops in the filesystem
    // It might be better to first list all the files and then spawn the tasks to scan them
    const WORKERS: usize = 128;

    struct Work {
        path: PathBuf,
        sender: flume::Sender<Work>,
    }

    tracing::debug!("scanning with params: {:?}", params);

    for path in params.include.iter_mut() {
        tracing::trace!("canonicalizing path: {}", path.display());
        *path = path.canonicalize()?;
    }

    let params = Arc::new(params);
    let start_time = Instant::now();
    let (wtx, wrx) = flume::unbounded::<Work>();
    let (itx, irx) = flume::bounded::<Option<ScanItem>>(128);

    for _ in 0..WORKERS {
        let params = params.clone();
        let wrx = wrx.clone();
        let itx = itx.clone();
        tokio::spawn(async move {
            while let Ok(Work { path, sender }) = wrx.recv_async().await {
                if path.is_symlink() {
                    tracing::warn!("skipping symlinked path '{}'", path.display());
                    continue;
                }

                let path = match path.canonicalize() {
                    Ok(path) => path,
                    Err(err) => {
                        tracing::warn!("failed to canonicalize path '{}': {}", path.display(), err);
                        continue;
                    }
                };

                if params.should_exclude(&path) {
                    continue;
                }

                tracing::trace!("scanning path: {}", path.display());
                if path.is_file() {
                    let item = scan_file(&path).await.map(move |id| ScanItem { path, id });
                    itx.send_async(item).await.unwrap();
                } else if path.is_dir() {
                    let mut readdir = tokio::fs::read_dir(path).await.expect("reading directory");
                    while let Some(entry) = readdir.next_entry().await.unwrap() {
                        let work = Work {
                            path: entry.path(),
                            sender: sender.clone(),
                        };
                        sender.send_async(work).await.unwrap();
                    }
                }
            }
        });
    }

    // kick of the workers
    for dir in params.include.iter() {
        wtx.send_async(Work {
            path: dir.clone(),
            sender: wtx.clone(),
        })
        .await
        .unwrap();
    }

    // prevent dead locks
    drop(wtx);
    drop(itx);

    let mut items = Vec::new();
    let mut scanned = 0;
    while let Ok(item) = irx.recv_async().await {
        if let Some(item) = item {
            items.push(item);
        }
        scanned += 1;
    }

    let finish_time = Instant::now();
    tracing::debug!(
        "scanned {} files in {:?} and found {} spotify ids",
        scanned,
        finish_time - start_time,
        items.len()
    );

    Ok(items)
}

pub async fn scan_file(path: &Path) -> Option<SpotifyId> {
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    if !EXTENSIONS.contains(&ext) {
        tracing::trace!(
            "skipping file '{}' because it has an unknown extension",
            path.display()
        );
        return None;
    }

    let tag_result = if ext == "wav" {
        id3::Tag::read_from_wav_path(path)
    } else {
        id3::Tag::read_from_path(path)
    };

    let tag = match tag_result {
        Ok(tag) => tag,
        Err(err) => {
            tracing::debug!(
                "failed to read id3 tag from file '{}': {}",
                path.display(),
                err
            );
            return None;
        }
    };

    for extended in tag.extended_texts() {
        if extended.description == tag::TAG_SPOTIFY_TRACK_ID {
            if extended.value.len() < 22 {
                tracing::warn!(
                    "spotify id '{}' in file '{}' is too short",
                    extended.value,
                    path.display()
                );
                continue;
            }

            // the spotify id is always 22 bytes long
            // extended.value can sometimes include a null terminator so remove that.
            let value = &extended.value[..22];

            match id::parse_id(value) {
                Ok(id) => {
                    tracing::trace!("found spotify id '{}' in file '{}'", id, path.display());
                    return Some(id);
                }
                Err(err) => {
                    tracing::warn!("failed to parse spotify id '{}': {}", value, err);
                }
            }
        }
    }
    tracing::trace!(
        "skipping file '{}' because it has no spotify id",
        path.display()
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_params() {
        let mut params = ScanParams::default();
        params.include("foo/m1.mp3");
        params.exclude("foo/m2.mp3");
        params.include("bar");
        params.exclude("zoo");

        assert!(!params.should_exclude(Path::new("foo/m1.mp3")));
        assert!(params.should_exclude(Path::new("foo/m2.mp3")));
        assert!(!params.should_exclude(Path::new("bar/m3.mp3")));
        assert!(params.should_exclude(Path::new("zoo/m4.mp3")));
    }
}
