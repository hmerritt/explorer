//! Native SFTP locations. The path codec is an adapter for Explorer's existing
//! provider UI; decoded remote paths must never be passed to std::fs.
use super::{
    clipboard::ClipboardDownload,
    entry::FileEntry,
    filesystem::EntryVisibility,
    remote_download::{self, RemoteCredentials, RemoteDownloadError, SftpHandler},
};
use gpui::http_client::Url;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use russh::client;
use russh_sftp::{
    client::RawSftpSession,
    protocol::{FileAttributes, Packet, StatusCode},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub(super) struct RemoteLocation {
    /// Credential-free SSH URL, retaining the SSH config alias.
    pub site: String,
    pub path: String,
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}
fn decode(value: &str) -> Option<String> {
    percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(|s| s.into_owned())
}

pub(super) fn virtual_root() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"\\?\explorer.sftp\sites")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/__explorer_sftp__/sites")
    }
}

pub(super) fn is_remote(path: &Path) -> bool {
    path.starts_with(virtual_root())
}

impl RemoteLocation {
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut url = Url::parse(input).map_err(|e| e.to_string())?;
        if url.scheme() != "sftp"
            || url.host_str().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Use sftp://[user@]host[:port]/folder.".into());
        }
        if url.password().is_some() {
            return Err(
                "Enter credentials in the sign-in dialog, not in the server address.".into(),
            );
        }
        let path = decode(url.path()).ok_or("The remote path is not valid UTF-8.")?;
        if path.contains('\0') {
            return Err("A remote path cannot contain a NUL character.".into());
        }
        url.set_path("/");
        Ok(Self {
            site: url.to_string(),
            path: if path.is_empty() { "/".into() } else { path },
        })
    }
    pub fn provider_path(&self) -> PathBuf {
        let mut path = virtual_root().join(encode(&self.site));
        for name in self.path.split('/').filter(|s| !s.is_empty()) {
            path.push(encode(name));
        }
        path
    }
    pub fn from_provider(path: &Path) -> Option<Self> {
        let mut components = path.strip_prefix(virtual_root()).ok()?.iter();
        let site = decode(components.next()?.to_str()?)?;
        let base = Self::parse(&site).ok()?;
        if base.site != site {
            return None;
        }
        let names: Option<Vec<_>> = components.map(|s| decode(s.to_str()?)).collect();
        let names = names?;
        if names
            .iter()
            .any(|s| s.contains('/') || s.contains('\0') || s == "." || s == "..")
        {
            return None;
        }
        Some(Self {
            site,
            path: format!("/{}", names.join("/")),
        })
    }
    pub fn child(&self, name: &str) -> Result<Self, String> {
        if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\0']) {
            return Err("Invalid remote filename.".into());
        }
        Ok(Self {
            site: self.site.clone(),
            path: format!("{}/{}", self.path.trim_end_matches('/'), name),
        })
    }
    pub fn address(&self) -> String {
        let mut url = Url::parse(&self.site).expect("validated site");
        url.set_path(&self.path);
        url.to_string()
    }
}

pub(super) fn display_address(path: &Path) -> Option<String> {
    Some(RemoteLocation::from_provider(path)?.address())
}
pub(super) fn parent(path: &Path) -> Option<PathBuf> {
    let loc = RemoteLocation::from_provider(path)?;
    if loc.path == "/" {
        None
    } else {
        path.parent().map(Path::to_path_buf)
    }
}
pub(super) fn breadcrumb_segments(path: &Path) -> Option<Vec<(String, PathBuf)>> {
    let loc = RemoteLocation::from_provider(path)?;
    let mut current = RemoteLocation {
        site: loc.site.clone(),
        path: "/".into(),
    };
    let mut segments = vec![(
        loc.site.trim_end_matches('/').into(),
        current.provider_path(),
    )];
    for name in loc.path.split('/').filter(|s| !s.is_empty()) {
        current = current.child(name).ok()?;
        segments.push((name.into(), current.provider_path()));
    }
    Some(segments)
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SavedSite {
    pub name: String,
    pub location: RemoteLocation,
}
pub(super) fn saved_sites() -> Vec<SavedSite> {
    sites_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Vec<SavedSite>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            RemoteLocation::parse(&s.location.site).is_ok_and(|base| base.site == s.location.site)
                && s.location.path.starts_with('/')
                && !s.location.path.contains('\0')
                && !s
                    .location
                    .path
                    .split('/')
                    .any(|part| matches!(part, "." | ".."))
        })
        .collect()
}
fn sites_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|p| p.join("sftp-sites.json"))
}
pub(super) fn update_site(location: RemoteLocation, name: String) -> Result<(), String> {
    let path = sites_path().ok_or("Configuration directory unavailable.")?;
    let mut sites = saved_sites();
    sites.retain(|site| site.location.site != location.site);
    let name = if name.trim().is_empty() {
        location.site.trim_end_matches('/').to_owned()
    } else {
        name.trim().to_owned()
    };
    sites.push(SavedSite { name, location });
    atomic_json(&path, &sites)
}
pub(super) fn forget_site(site: &str) -> Result<(), String> {
    let path = sites_path().ok_or("Configuration directory unavailable.")?;
    let mut sites = saved_sites();
    sites.retain(|saved| saved.location.site != site);
    atomic_json(&path, &sites)
}
pub(super) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("Missing state directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut tmp, value).map_err(|e| e.to_string())?;
    tmp.flush()
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) enum PromptReply {
    Credentials(RemoteCredentials),
    Accept,
    Cancel,
}
pub(super) struct Prompt {
    pub id: u64,
    pub error: RemoteDownloadError,
    reply: oneshot::Sender<PromptReply>,
}
type SessionSlot = Arc<AsyncMutex<Option<Arc<Session>>>>;
#[derive(Default)]
struct Service {
    metadata: Mutex<HashMap<PathBuf, FileAttributes>>,
    slots: Mutex<HashMap<(String, usize), SessionSlot>>,
    credentials: Mutex<HashMap<String, RemoteCredentials>>,
    prompts: Mutex<VecDeque<Prompt>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<PromptReply>>>,
}
fn service() -> &'static Service {
    static S: OnceLock<Service> = OnceLock::new();
    S.get_or_init(Service::default)
}
pub(super) fn take_prompt() -> Option<(u64, RemoteDownloadError)> {
    let mut queue = service().prompts.lock().unwrap();
    while let Some(prompt) = queue.pop_front() {
        if prompt.reply.is_closed() {
            continue;
        }
        service()
            .pending
            .lock()
            .unwrap()
            .insert(prompt.id, prompt.reply);
        return Some((prompt.id, prompt.error));
    }
    None
}
pub(super) fn reply(id: u64, value: PromptReply) -> bool {
    let sender = service().pending.lock().unwrap().remove(&id);
    if let Some(sender) = sender {
        let _ = sender.send(value);
        true
    } else {
        false
    }
}
async fn prompt(error: RemoteDownloadError) -> PromptReply {
    static IDS: AtomicU64 = AtomicU64::new(1 << 63);
    let (tx, rx) = oneshot::channel();
    service().prompts.lock().unwrap().push_back(Prompt {
        id: IDS.fetch_add(1, Ordering::Relaxed),
        error,
        reply: tx,
    });
    rx.await.unwrap_or(PromptReply::Cancel)
}

pub(super) struct Session {
    pub endpoint: Option<remote_download::RemoteEndpointKey>,
    pub raw: RawSftpSession,
    pub extensions: HashMap<String, String>,
    pub chunk_size: u32,
    pub(super) _ssh: Option<client::Handle<SftpHandler>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.raw.close_session();
    }
}
pub(super) fn runtime() -> &'static tokio::runtime::Runtime {
    remote_download::remote_runtime()
}

pub(super) async fn session(site: &str, lane: usize) -> io::Result<Arc<Session>> {
    let slot = service()
        .slots
        .lock()
        .unwrap()
        .entry((site.into(), lane))
        .or_default()
        .clone();
    let mut guard = slot.lock().await;
    if let Some(session) = guard.as_ref() {
        return Ok(session.clone());
    }
    let download = ClipboardDownload {
        url: Url::parse(site).map_err(io::Error::other)?,
        file_name: String::new(),
    };
    let target = remote_download::remote_target(&download).map_err(io::Error::other)?;
    loop {
        let credentials = service().credentials.lock().unwrap().get(site).cloned();
        if credentials
            .as_ref()
            .is_some_and(|credentials| credentials.username != target.username())
        {
            service().credentials.lock().unwrap().remove(site);
            return Err(io::Error::other(
                "The sign-in username differs from this site's identity. Connect using sftp://username@host/ to select another account.",
            ));
        }
        let connected = tokio::time::timeout(
            Duration::from_secs(60),
            remote_download::connect_sftp(&target, credentials),
        )
        .await;
        let ssh = match connected {
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SSH connection/authentication timed out",
                ));
            }
            Ok(Ok(ssh)) => ssh,
            Ok(Err(RemoteDownloadError::Fatal(message))) => return Err(io::Error::other(message)),
            Ok(Err(error)) => {
                match prompt(error).await {
                    PromptReply::Credentials(credentials) => {
                        service()
                            .credentials
                            .lock()
                            .unwrap()
                            .insert(site.into(), credentials);
                    }
                    PromptReply::Accept => {}
                    PromptReply::Cancel => {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "Sign-in cancelled",
                        ));
                    }
                }
                continue;
            }
        };
        let channel = ssh.channel_open_session().await.map_err(io::Error::other)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(io::Error::other)?;
        let mut raw = RawSftpSession::new(channel.into_stream());
        raw.set_timeout(30);
        let version = raw.init().await.map_err(sftp_error)?;
        let mut chunk_size = 32 * 1024;
        if version.extensions.contains_key("limits@openssh.com") {
            let limits = russh_sftp::client::rawsession::Limits::from(
                raw.limits().await.map_err(sftp_error)?,
            );
            for limit in [
                limits.read_len,
                limits.write_len,
                limits.packet_len.map(|n| n.saturating_sub(1024)),
            ]
            .into_iter()
            .flatten()
            {
                if limit > 0 {
                    chunk_size = chunk_size.min(limit.min(u32::MAX as u64) as u32);
                }
            }
            raw.set_limits(limits);
        }
        let connected = Arc::new(Session {
            endpoint: Some(target.endpoint()),
            raw,
            extensions: version.extensions,
            chunk_size,
            _ssh: Some(ssh),
        });
        *guard = Some(connected.clone());
        return Ok(connected);
    }
}
pub(super) async fn disconnect(site: &str, lane: usize) {
    let slot = service()
        .slots
        .lock()
        .unwrap()
        .get(&(site.into(), lane))
        .cloned();
    if let Some(slot) = slot {
        *slot.lock().await = None;
    }
}
pub(super) fn sftp_error(error: russh_sftp::client::error::Error) -> io::Error {
    use russh_sftp::client::error::Error;
    let kind = match &error {
        Error::Status(s) if s.status_code == StatusCode::NoSuchFile => io::ErrorKind::NotFound,
        Error::Status(s) if s.status_code == StatusCode::PermissionDenied => {
            io::ErrorKind::PermissionDenied
        }
        Error::Status(s)
            if matches!(
                s.status_code,
                StatusCode::ConnectionLost | StatusCode::NoConnection
            ) =>
        {
            io::ErrorKind::ConnectionReset
        }
        Error::Status(_) => io::ErrorKind::Other,
        Error::Timeout => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::ConnectionReset,
    };
    io::Error::new(kind, error)
}

impl Session {
    pub async fn metadata(&self, path: &str) -> io::Result<FileAttributes> {
        Ok(self.raw.lstat(path).await.map_err(sftp_error)?.attrs)
    }
    pub async fn list(&self, loc: &RemoteLocation) -> io::Result<Vec<(String, FileAttributes)>> {
        let handle = self
            .raw
            .opendir(&loc.path)
            .await
            .map_err(sftp_error)?
            .handle;
        let mut entries = Vec::new();
        let result = async {
            loop {
                match self.raw.readdir(&handle).await {
                    Ok(list) => {
                        for file in list.files {
                            if matches!(file.filename.as_str(), "." | "..") {
                                continue;
                            }
                            loc.child(&file.filename).map_err(io::Error::other)?;
                            entries.push((file.filename, file.attrs));
                        }
                    }
                    Err(russh_sftp::client::error::Error::Status(s))
                        if s.status_code == StatusCode::Eof =>
                    {
                        break;
                    }
                    Err(e) => return Err(sftp_error(e)),
                }
            }
            Ok(entries)
        }
        .await;
        let closed = self.raw.close(handle).await.map_err(sftp_error);
        let entries = result?;
        closed?;
        Ok(entries)
    }
    pub async fn replace(&self, from: &str, to: &str, overwrite: bool) -> io::Result<()> {
        if !overwrite {
            self.raw.rename(from, to).await.map_err(sftp_error)?;
            return Ok(());
        }
        if !self.extensions.contains_key("posix-rename@openssh.com") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This server cannot atomically replace the destination. Choose Keep both or Skip.",
            ));
        }
        let mut data = Vec::new();
        for path in [from, to] {
            data.extend_from_slice(&(path.len() as u32).to_be_bytes());
            data.extend_from_slice(path.as_bytes());
        }
        match self
            .raw
            .extended("posix-rename@openssh.com", data)
            .await
            .map_err(sftp_error)?
        {
            Packet::Status(s) if s.status_code == StatusCode::Ok => Ok(()),
            Packet::Status(s) => Err(sftp_error(s.into())),
            _ => Err(io::Error::other("Unexpected rename response")),
        }
    }
}

pub(super) fn list_dir(path: &Path, visibility: EntryVisibility) -> io::Result<Vec<FileEntry>> {
    let loc = RemoteLocation::from_provider(path)
        .ok_or_else(|| io::Error::other("Invalid SFTP location"))?;
    runtime().block_on(async {
        let session = session(&loc.site, 0).await?;
        let result = session.list(&loc).await;
        if result.is_err() {
            disconnect(&loc.site, 0).await;
        }
        let listed = result?;
        {
            let mut cache = service().metadata.lock().unwrap();
            cache.retain(|candidate, _| candidate.parent() != Some(path));
            if cache.len() > 100_000 {
                cache.clear();
            }
            let mut directory = FileAttributes::empty();
            directory.permissions = Some(0o040755);
            cache.insert(path.to_owned(), directory);
        }
        let mut entries = Vec::new();
        for (name, attrs) in listed {
            if !visibility.show_dotfiles && name.starts_with('.') {
                continue;
            }
            let child = loc.child(&name).map_err(io::Error::other)?;
            let directory_link = attrs.is_symlink()
                && session
                    .raw
                    .stat(&child.path)
                    .await
                    .is_ok_and(|a| a.attrs.is_dir());
            let mut entry = FileEntry::from_provider(
                child.provider_path(),
                name,
                attrs.is_dir() || directory_link,
                attrs.size,
                attrs
                    .mtime
                    .map(|t| UNIX_EPOCH + Duration::from_secs(t as u64)),
            );
            if directory_link {
                entry.kind = super::entry::EntryKind::DirectoryLink(
                    super::entry::DirectoryLinkKind::FilesystemLink,
                );
            }
            let mut cached = attrs.clone();
            if directory_link {
                cached.permissions = Some(0o040755);
            }
            service()
                .metadata
                .lock()
                .unwrap()
                .insert(child.provider_path(), cached);
            entries.push(entry);
        }
        Ok(entries)
    })
}

pub(super) fn cached_is_dir(path: &Path) -> Result<bool, String> {
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    Ok(loc.path == "/"
        || service()
            .metadata
            .lock()
            .unwrap()
            .get(path)
            .is_some_and(FileAttributes::is_dir))
}
pub(super) fn metadata(path: &Path) -> Result<FileAttributes, String> {
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    runtime()
        .block_on(async { session(&loc.site, 0).await?.metadata(&loc.path).await })
        .map_err(|e: io::Error| e.to_string())
}
pub(super) fn exists(path: &Path) -> Result<bool, String> {
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    runtime()
        .block_on(async {
            match session(&loc.site, 0).await?.metadata(&loc.path).await {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e),
            }
        })
        .map_err(|e: io::Error| e.to_string())
}
pub(super) fn create_dir(path: &Path) -> Result<(), String> {
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    runtime()
        .block_on(async {
            session(&loc.site, 0)
                .await?
                .raw
                .mkdir(&loc.path, FileAttributes::empty())
                .await
                .map_err(sftp_error)?;
            Ok(())
        })
        .map_err(|e: io::Error| e.to_string())
}
pub(super) fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use russh_sftp::protocol::OpenFlags;
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    runtime()
        .block_on(async {
            let session = session(&loc.site, 0).await?;
            let handle = session
                .raw
                .open(
                    &loc.path,
                    OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    FileAttributes::empty(),
                )
                .await
                .map_err(sftp_error)?
                .handle;
            let mut result = Ok(());
            for (index, chunk) in bytes.chunks(session.chunk_size as usize).enumerate() {
                if let Err(error) = session
                    .raw
                    .write(
                        &handle,
                        (index * session.chunk_size as usize) as u64,
                        chunk.to_vec(),
                    )
                    .await
                    .map_err(sftp_error)
                {
                    result = Err(error);
                    break;
                }
            }
            let closed = session.raw.close(handle).await.map_err(sftp_error);
            result?;
            closed?;
            Ok(())
        })
        .map_err(|e: io::Error| e.to_string())
}
pub(super) fn rename(path: &Path, name: &str) -> Result<PathBuf, String> {
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    let parent = parent(path)
        .and_then(|p| RemoteLocation::from_provider(&p))
        .ok_or("Cannot rename a server root")?;
    let destination = parent.child(name)?;
    runtime()
        .block_on(async {
            session(&loc.site, 0)
                .await?
                .replace(&loc.path, &destination.path, false)
                .await
        })
        .map_err(|e| e.to_string())?;
    Ok(destination.provider_path())
}
pub(super) fn delete(path: &Path) -> Result<(), String> {
    let loc = RemoteLocation::from_provider(path).ok_or("Invalid SFTP location")?;
    if loc.path == "/" {
        return Err("Cannot delete a server root".into());
    }
    runtime()
        .block_on(async {
            let session = session(&loc.site, 0).await?;
            let mut stack = vec![(loc, false)];
            while let Some((loc, visited)) = stack.pop() {
                let meta = session.metadata(&loc.path).await?;
                if meta.is_dir() {
                    if visited {
                        session.raw.rmdir(&loc.path).await.map_err(sftp_error)?;
                    } else {
                        stack.push((loc.clone(), true));
                        for (name, _) in session.list(&loc).await? {
                            stack.push((loc.child(&name).map_err(io::Error::other)?, false));
                        }
                    }
                } else {
                    session.raw.remove(&loc.path).await.map_err(sftp_error)?;
                }
            }
            Ok(())
        })
        .map_err(|e: io::Error| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn location_round_trips_platform_sensitive_names() {
        let root = RemoteLocation::parse("sftp://alice@example.com:2222/").unwrap();
        for name in ["a\\b", "CON", "a:b", "100%", "café", "trailing. ", "a#b"] {
            let child = root.child(name).unwrap();
            assert_eq!(
                RemoteLocation::from_provider(&child.provider_path()),
                Some(child.clone())
            );
            assert_eq!(RemoteLocation::parse(&child.address()).unwrap(), child);
        }
    }
    #[test]
    fn credentials_and_invalid_children_are_rejected() {
        assert!(RemoteLocation::parse("sftp://user:secret@host/").is_err());
        let root = RemoteLocation::parse("sftp://host/").unwrap();
        for name in ["", ".", "..", "a/b", "a\0b"] {
            assert!(root.child(name).is_err());
        }
        assert!(parent(&root.provider_path()).is_none());
    }
}
