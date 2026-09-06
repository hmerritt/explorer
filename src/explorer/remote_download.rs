use std::{
    collections::HashSet,
    env,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use percent_encoding::percent_decode_str;
use russh::{
    client::{self, AuthResult},
    keys::{
        HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
        agent::client::AgentClient, load_secret_key,
    },
};
use russh_sftp::client::SftpSession;
use ssh2_config::{ParseRule, SshConfig};
use suppaftp::{tokio::AsyncFtpStream, types::FileType};
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;

use crate::explorer::{
    clipboard::ClipboardDownload,
    download::{DOWNLOAD_BUFFER_SIZE, DownloadProgress, PendingDownload},
    user_home_dir,
};

const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Eq, PartialEq)]
pub(super) struct RemoteCredentials {
    pub(super) username: String,
    pub(super) password: String,
    pub(super) passphrase: bool,
}

#[derive(Clone)]
pub(super) struct RemoteHostKey {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) algorithm: String,
    pub(super) fingerprint: String,
    public_key: PublicKey,
    known_hosts_path: Option<PathBuf>,
}

pub(super) enum RemoteDownloadError {
    PassphraseRequired {
        host: String,
        username: String,
        key_path: PathBuf,
    },
    CredentialsRequired {
        host: String,
        username: String,
        message: Option<String>,
    },
    UnknownHost(Box<RemoteHostKey>),
    Fatal(String),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct RemoteEndpointKey {
    scheme: String,
    host: String,
    port: u16,
    username: String,
}

pub(super) struct RemoteTarget {
    scheme: String,
    host: String,
    port: u16,
    username: String,
    path: String,
    identity_files: Vec<PathBuf>,
    embedded_password: Option<String>,
    known_hosts: Vec<PathBuf>,
    identity_agent: Option<String>,
    identities_only: bool,
    connect_timeout: Duration,
    keepalive: Duration,
}
impl RemoteTarget {
    pub(super) fn endpoint(&self) -> RemoteEndpointKey {
        RemoteEndpointKey {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
        }
    }
    pub(super) fn username(&self) -> &str {
        &self.username
    }
}

#[derive(Clone)]
enum HostKeyCheck {
    Clear,
    Unknown(Box<RemoteHostKey>),
    Changed(String),
}

pub(super) struct SftpHandler {
    host: String,
    port: u16,
    check: Arc<Mutex<HostKeyCheck>>,
    known_hosts: Vec<PathBuf>,
}

impl client::Handler for SftpHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public_key = server_public_key.public_key();
        let verified = if self.known_hosts.is_empty() {
            russh::keys::check_known_hosts(&self.host, self.port, &public_key)
        } else {
            let mut result = Ok(false);
            for path in &self.known_hosts {
                match russh::keys::check_known_hosts_path(&self.host, self.port, &public_key, path)
                {
                    Ok(true) => {
                        result = Ok(true);
                        break;
                    }
                    Ok(false) => {}
                    Err(error) => result = Err(error),
                }
            }
            result
        };
        match verified {
            Ok(true) => Ok(true),
            Ok(false) => {
                let key = RemoteHostKey {
                    host: self.host.clone(),
                    port: self.port,
                    algorithm: public_key.algorithm().to_string(),
                    fingerprint: public_key.fingerprint(HashAlg::Sha256).to_string(),
                    public_key,
                    known_hosts_path: self.known_hosts.first().cloned(),
                };
                *self.check.lock().expect("host-key check lock") =
                    HostKeyCheck::Unknown(Box::new(key));
                Ok(false)
            }
            Err(russh::keys::Error::KeyChanged { line }) => {
                *self.check.lock().expect("host-key check lock") = HostKeyCheck::Changed(format!(
                    "The SSH host key for {} changed (known_hosts line {line}).",
                    self.host
                ));
                Ok(false)
            }
            Err(error) => {
                *self.check.lock().expect("host-key check lock") = HostKeyCheck::Changed(format!(
                    "Could not verify the SSH host key for {}: {error}",
                    self.host
                ));
                Ok(false)
            }
        }
    }
}

pub(super) fn is_remote_download(download: &ClipboardDownload) -> bool {
    matches!(download.url.scheme(), "ftp" | "sftp")
}

pub(super) fn endpoint_key(download: &ClipboardDownload) -> Option<RemoteEndpointKey> {
    let target = remote_target(download).ok()?;
    Some(RemoteEndpointKey {
        scheme: target.scheme,
        host: target.host,
        port: target.port,
        username: target.username,
    })
}

pub(super) fn embedded_credentials(download: &ClipboardDownload) -> Option<RemoteCredentials> {
    let target = remote_target(download).ok()?;
    target.embedded_password.map(|password| RemoteCredentials {
        passphrase: false,
        username: target.username,
        password,
    })
}

pub(super) fn remember_host_key(key: &RemoteHostKey) -> Result<(), String> {
    if let Some(path) = &key.known_hosts_path {
        return russh::keys::known_hosts::learn_known_hosts_path(
            &key.host,
            key.port,
            &key.public_key,
            path,
        )
        .map_err(|e| format!("Could not remember SSH host key: {e}"));
    }
    russh::keys::known_hosts::learn_known_hosts(key.host.as_str(), key.port, &key.public_key)
        .map_err(|error| format!("Could not remember the SSH host key: {error}"))
}

pub(super) fn download_remote_to_temporary_file(
    download: ClipboardDownload,
    credentials: Option<RemoteCredentials>,
    destination: &Path,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress) + Send,
) -> Result<PendingDownload, RemoteDownloadError> {
    remote_runtime().block_on(async {
        tokio::select! {
            biased;
            _ = wait_cancelled(&cancel) => Err(RemoteDownloadError::Fatal("Download cancelled.".to_owned())),
            result = download_remote_async(download, credentials, destination, on_progress) => result,
        }
    })
}

async fn wait_cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(super) fn remote_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("remote transfer Tokio runtime")
    })
}

async fn download_remote_async(
    download: ClipboardDownload,
    credentials: Option<RemoteCredentials>,
    destination: &Path,
    mut on_progress: impl FnMut(DownloadProgress) + Send,
) -> Result<PendingDownload, RemoteDownloadError> {
    let target = remote_target(&download).map_err(RemoteDownloadError::Fatal)?;
    let mut temporary = NamedTempFile::new_in(destination).map_err(|error| {
        RemoteDownloadError::Fatal(format!(
            "Could not create a temporary file for \"{}\": {error}",
            download.file_name
        ))
    })?;

    let total_bytes = match target.scheme.as_str() {
        "ftp" => {
            download_ftp(
                &target,
                credentials,
                temporary.as_file_mut(),
                &mut on_progress,
            )
            .await?
        }
        "sftp" => {
            download_sftp(
                &target,
                credentials,
                temporary.as_file_mut(),
                &mut on_progress,
            )
            .await?
        }
        _ => {
            return Err(RemoteDownloadError::Fatal(
                "Unsupported remote URL scheme.".to_owned(),
            ));
        }
    };
    temporary.as_file_mut().flush().map_err(|error| {
        RemoteDownloadError::Fatal(format!(
            "Could not save \"{}\": {error}",
            download.file_name
        ))
    })?;
    if let Some(expected) = total_bytes {
        let received = temporary
            .as_file()
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if received != expected {
            return Err(RemoteDownloadError::Fatal(format!(
                "Could not download \"{}\": expected {expected} bytes but received {received}",
                download.file_name
            )));
        }
    }

    Ok(PendingDownload {
        temporary,
        destination: destination.to_path_buf(),
        file_name: download.file_name,
    })
}

async fn download_ftp(
    target: &RemoteTarget,
    credentials: Option<RemoteCredentials>,
    output: &mut std::fs::File,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<Option<u64>, RemoteDownloadError> {
    let address = format!("{}:{}", target.host, target.port);
    let mut ftp = tokio::time::timeout(REMOTE_CONNECT_TIMEOUT, AsyncFtpStream::connect(address))
        .await
        .map_err(|_| {
            RemoteDownloadError::Fatal(format!("Could not connect to {}: timed out", target.host))
        })?
        .map_err(|error| {
            RemoteDownloadError::Fatal(format!("Could not connect to {}: {error}", target.host))
        })?;

    let credentials = credentials.or_else(|| {
        target
            .embedded_password
            .as_ref()
            .map(|password| RemoteCredentials {
                passphrase: false,
                username: target.username.clone(),
                password: password.clone(),
            })
    });
    let (username, password) = match credentials {
        Some(credentials) => (credentials.username, credentials.password),
        None if target.username.is_empty() => {
            ("anonymous".to_owned(), "explorer@localhost".to_owned())
        }
        None => {
            return Err(RemoteDownloadError::CredentialsRequired {
                host: target.host.clone(),
                username: target.username.clone(),
                message: None,
            });
        }
    };
    if let Err(error) = ftp.login(&username, &password).await {
        return Err(RemoteDownloadError::CredentialsRequired {
            host: target.host.clone(),
            username: if username == "anonymous" {
                String::new()
            } else {
                username
            },
            message: Some(format!("Sign-in failed: {error}")),
        });
    }
    ftp.transfer_type(FileType::Binary).await.map_err(|error| {
        RemoteDownloadError::Fatal(format!("Could not select binary FTP transfer: {error}"))
    })?;
    let total_bytes = ftp.size(&target.path).await.ok().map(|size| size as u64);
    on_progress(DownloadProgress {
        downloaded_bytes: 0,
        total_bytes,
    });
    let mut stream = ftp.retr_as_stream(&target.path).await.map_err(|error| {
        RemoteDownloadError::Fatal(format!("Could not download {}: {error}", target.path))
    })?;
    copy_async_stream(&mut stream, output, total_bytes, on_progress).await?;
    ftp.finalize_retr_stream(stream).await.map_err(|error| {
        RemoteDownloadError::Fatal(format!("Could not finish FTP download: {error}"))
    })?;
    let _ = ftp.quit().await;
    Ok(total_bytes)
}

async fn download_sftp(
    target: &RemoteTarget,
    credentials: Option<RemoteCredentials>,
    output: &mut std::fs::File,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<Option<u64>, RemoteDownloadError> {
    let session = connect_sftp(target, credentials).await?;
    let channel = session.channel_open_session().await.map_err(|error| {
        RemoteDownloadError::Fatal(format!("Could not open the SFTP session: {error}"))
    })?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| {
            RemoteDownloadError::Fatal(format!("Could not start the SFTP subsystem: {error}"))
        })?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|error| {
            RemoteDownloadError::Fatal(format!("Could not initialize SFTP: {error}"))
        })?;
    let total_bytes = sftp
        .metadata(&target.path)
        .await
        .ok()
        .map(|metadata| metadata.len());
    let mut file = sftp.open(&target.path).await.map_err(|error| {
        RemoteDownloadError::Fatal(format!("Could not download {}: {error}", target.path))
    })?;
    on_progress(DownloadProgress {
        downloaded_bytes: 0,
        total_bytes,
    });
    copy_async_stream(&mut file, output, total_bytes, on_progress).await?;
    Ok(total_bytes)
}

pub(super) async fn connect_sftp(
    target: &RemoteTarget,
    credentials: Option<RemoteCredentials>,
) -> Result<client::Handle<SftpHandler>, RemoteDownloadError> {
    let check = Arc::new(Mutex::new(HostKeyCheck::Clear));
    let handler = SftpHandler {
        host: target.host.clone(),
        port: target.port,
        check: check.clone(),
        known_hosts: target.known_hosts.clone(),
    };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(target.keepalive),
        keepalive_max: 3,
        ..Default::default()
    });
    let connected = tokio::time::timeout(
        target.connect_timeout,
        client::connect(config, (target.host.as_str(), target.port), handler),
    )
    .await;
    let mut session = match connected {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            return match check.lock().expect("host-key check lock").clone() {
                HostKeyCheck::Unknown(key) => Err(RemoteDownloadError::UnknownHost(key)),
                HostKeyCheck::Changed(error) => Err(RemoteDownloadError::Fatal(error)),
                HostKeyCheck::Clear => Err(RemoteDownloadError::Fatal(format!(
                    "Could not connect to {}: {error}",
                    target.host
                ))),
            };
        }
        Err(_) => {
            return Err(RemoteDownloadError::Fatal(format!(
                "Could not connect to {}: timed out",
                target.host
            )));
        }
    };

    let attempted_password = credentials.is_some() || target.embedded_password.is_some();
    let attempted_username = credentials
        .as_ref()
        .map(|credentials| credentials.username.clone())
        .unwrap_or_else(|| target.username.clone());
    let passphrase = credentials
        .as_ref()
        .filter(|c| c.passphrase)
        .map(|c| c.password.as_str());
    let authenticated = if let Some(credentials) = credentials.as_ref().filter(|c| !c.passphrase) {
        session
            .authenticate_password(&credentials.username, &credentials.password)
            .await
            .map_err(|e| RemoteDownloadError::Fatal(format!("SSH authentication failed: {e}")))?
            .success()
    } else {
        try_agent_authentication(&mut session, &target.username, target).await
            || try_key_files(
                &mut session,
                &target.username,
                &target.identity_files,
                passphrase,
            )
            .await
    };
    if !authenticated {
        if credentials.as_ref().is_none_or(|c| c.passphrase) {
            if let Some(key_path) = target.identity_files.iter().find(|p| {
                russh::keys::PrivateKey::read_openssh_file(p).is_ok_and(|key| key.is_encrypted())
            }) {
                return Err(RemoteDownloadError::PassphraseRequired {
                    host: target.host.clone(),
                    username: target.username.clone(),
                    key_path: key_path.clone(),
                });
            }
        }
        return Err(RemoteDownloadError::CredentialsRequired {
            host: target.host.clone(),
            username: attempted_username,
            message: attempted_password
                .then(|| "The username or password was not accepted.".to_owned()),
        });
    }

    Ok(session)
}

async fn copy_async_stream(
    input: &mut (impl tokio::io::AsyncRead + Unpin),
    output: &mut std::fs::File,
    total_bytes: Option<u64>,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<(), RemoteDownloadError> {
    let mut buffer = vec![0; DOWNLOAD_BUFFER_SIZE];
    let mut downloaded_bytes = 0u64;
    loop {
        let read = input.read(&mut buffer).await.map_err(|error| {
            RemoteDownloadError::Fatal(format!("Could not read the remote file: {error}"))
        })?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            RemoteDownloadError::Fatal(format!("Could not save the remote file: {error}"))
        })?;
        downloaded_bytes = downloaded_bytes.saturating_add(read as u64);
        on_progress(DownloadProgress {
            downloaded_bytes,
            total_bytes,
        });
    }
    Ok(())
}

async fn try_agent_authentication(
    session: &mut client::Handle<SftpHandler>,
    username: &str,
    target: &RemoteTarget,
) -> bool {
    let Some(mut agent) = selected_agent(target.identity_agent.as_deref()).await else {
        return false;
    };
    let Ok(identities) = agent.request_identities().await else {
        return false;
    };
    for identity in identities {
        let public_key = identity.public_key().into_owned();
        if target.identities_only
            && !target.identity_files.iter().any(|path| {
                russh::keys::PrivateKey::read_openssh_file(path)
                    .is_ok_and(|key| *key.public_key() == public_key)
                    || std::fs::read_to_string(format!("{}.pub", path.display()))
                        .ok()
                        .and_then(|s| PublicKey::from_openssh(&s).ok())
                        .is_some_and(|key| key == public_key)
            })
        {
            continue;
        }
        let hash = session
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        if session
            .authenticate_publickey_with(username, public_key, hash, &mut agent)
            .await
            .is_ok_and(|result| result.success())
        {
            return true;
        }
    }
    false
}

async fn selected_agent(
    path: Option<&str>,
) -> Option<AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>>> {
    match path {
        Some("none") => None,
        None | Some("SSH_AUTH_SOCK") => connect_agent().await,
        Some(path) => {
            #[cfg(unix)]
            {
                AgentClient::connect_uds(path)
                    .await
                    .ok()
                    .map(AgentClient::dynamic)
            }
            #[cfg(windows)]
            {
                AgentClient::connect_named_pipe(path)
                    .await
                    .ok()
                    .map(AgentClient::dynamic)
            }
            #[cfg(not(any(unix, windows)))]
            {
                let _ = path;
                None
            }
        }
    }
}

#[cfg(unix)]
async fn connect_agent()
-> Option<AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>>> {
    AgentClient::connect_env()
        .await
        .ok()
        .map(AgentClient::dynamic)
}

#[cfg(target_os = "windows")]
async fn connect_agent()
-> Option<AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>>> {
    if let Some(path) = env::var_os("SSH_AUTH_SOCK")
        && let Ok(agent) = AgentClient::connect_named_pipe(path).await
    {
        return Some(agent.dynamic());
    }
    if let Ok(agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        return Some(agent.dynamic());
    }
    AgentClient::connect_pageant()
        .await
        .ok()
        .map(AgentClient::dynamic)
}

#[cfg(not(any(unix, target_os = "windows")))]
async fn connect_agent()
-> Option<AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>>> {
    None
}

async fn try_key_files(
    session: &mut client::Handle<SftpHandler>,
    username: &str,
    identity_files: &[PathBuf],
    passphrase: Option<&str>,
) -> bool {
    for path in identity_files {
        let Ok(private_key) = load_secret_key(path, passphrase) else {
            continue;
        };
        let hash = session
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        let key = PrivateKeyWithHashAlg::new(Arc::new(private_key), hash);
        if matches!(
            session.authenticate_publickey(username, key).await,
            Ok(AuthResult::Success)
        ) {
            return true;
        }
    }
    false
}

pub(super) fn remote_target(download: &ClipboardDownload) -> Result<RemoteTarget, String> {
    let url = &download.url;
    let scheme = url.scheme().to_owned();
    let original_host = url
        .host_str()
        .ok_or_else(|| "Remote URL has no host.".to_owned())?;
    let config =
        if scheme == "sftp" && user_home_dir().is_some_and(|p| p.join(".ssh/config").exists()) {
            Some(
                SshConfig::parse_default_file(
                    ParseRule::ALLOW_UNKNOWN_FIELDS | ParseRule::ALLOW_UNSUPPORTED_FIELDS,
                )
                .map_err(|e| format!("Could not read SSH config: {e}"))?,
            )
        } else {
            None
        };
    let params = config.as_ref().map(|config| config.query(original_host));
    let host = params
        .as_ref()
        .and_then(|params| params.host_name.clone())
        .unwrap_or_else(|| original_host.to_owned());
    let url_username = decode_url_component(url.username())?;
    let username = if !url_username.is_empty() {
        url_username
    } else if let Some(user) = params.as_ref().and_then(|params| params.user.clone()) {
        user
    } else if scheme == "sftp" {
        local_username().unwrap_or_default()
    } else {
        String::new()
    };
    let port = url
        .port()
        .or_else(|| params.as_ref().and_then(|params| params.port))
        .unwrap_or_else(|| if scheme == "sftp" { 22 } else { 21 });
    let path = percent_decode_str(url.path())
        .decode_utf8()
        .map_err(|_| "Remote URL path is not valid UTF-8.".to_owned())?
        .into_owned();
    let embedded_password = url.password().map(decode_url_component).transpose()?;
    let field = |key: &str| -> Option<String> {
        params
            .as_ref()
            .and_then(|p| {
                p.unsupported_fields
                    .get(key)
                    .or_else(|| p.ignored_fields.get(key))
            })
            .and_then(|v| v.first())
            .cloned()
    };
    if params
        .as_ref()
        .is_some_and(|p| p.proxy_jump.is_some() || p.certificate_file.is_some())
        || field("proxycommand").is_some()
        || field("hostkeyalias").is_some()
    {
        return Err("This SSH alias requires a proxy, certificate, or host-key alias that Explorer does not yet support. Use an ordinary direct SSH alias.".into());
    }
    let identities_only = field("identitiesonly").is_some_and(|s| s.eq_ignore_ascii_case("yes"));
    let identity_agent = field("identityagent").map(|s| {
        expand_identity_path(s.into(), user_home_dir().as_deref(), &host, &username)
            .to_string_lossy()
            .into_owned()
    });
    let known_hosts = params
        .as_ref()
        .and_then(|p| {
            p.unsupported_fields
                .get("userknownhostsfile")
                .or_else(|| p.ignored_fields.get("userknownhostsfile"))
        })
        .map(|v| {
            v.iter()
                .map(|p| {
                    expand_identity_path(p.into(), user_home_dir().as_deref(), &host, &username)
                })
                .collect()
        })
        .unwrap_or_default();
    let connect_timeout = params
        .as_ref()
        .and_then(|p| p.connect_timeout)
        .unwrap_or(REMOTE_CONNECT_TIMEOUT);
    let keepalive = params
        .as_ref()
        .and_then(|p| p.server_alive_interval)
        .unwrap_or(Duration::from_secs(15));
    let configured = params.as_ref().and_then(|p| p.identity_file.clone());
    let identity_files = ssh_identity_files(configured, &host, &username);
    Ok(RemoteTarget {
        scheme,
        host,
        port,
        username,
        path,
        identity_files,
        embedded_password,
        known_hosts,
        identity_agent,
        identities_only,
        connect_timeout,
        keepalive,
    })
}

fn ssh_identity_files(
    configured: Option<Vec<PathBuf>>,
    host: &str,
    username: &str,
) -> Vec<PathBuf> {
    let home = user_home_dir();
    let has_configured = configured.as_ref().is_some_and(|v| !v.is_empty());
    let mut files = configured
        .unwrap_or_default()
        .into_iter()
        .filter(|path| path.as_os_str() != "none")
        .map(|path| expand_identity_path(path, home.as_deref(), host, username))
        .collect::<Vec<_>>();
    if !has_configured && let Some(home) = home {
        let ssh = home.join(".ssh");
        files.extend([
            ssh.join("id_ed25519"),
            ssh.join("id_ecdsa"),
            ssh.join("id_rsa"),
        ]);
    }
    let mut seen = HashSet::new();
    files.retain(|path| seen.insert(path.clone()));
    files
}

fn expand_identity_path(path: PathBuf, home: Option<&Path>, host: &str, username: &str) -> PathBuf {
    let mut value = path.to_string_lossy().into_owned();
    if let Some(home) = home {
        value = value
            .replace("%d", &home.to_string_lossy())
            .replace("%h", host)
            .replace("%r", username)
            .replace("%%", "%");
        if value == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = value
            .strip_prefix("~/")
            .or_else(|| value.strip_prefix("~\\"))
        {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

fn decode_url_component(value: &str) -> Result<String, String> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| "Remote URL credentials are not valid UTF-8.".to_owned())
}

fn local_username() -> Option<String> {
    #[cfg(target_os = "windows")]
    let names = ["USERNAME", "USER"];
    #[cfg(not(target_os = "windows"))]
    let names = ["USER", "LOGNAME"];
    names
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use gpui::http_client::Url;

    use super::*;

    fn download(url: &str) -> ClipboardDownload {
        ClipboardDownload {
            url: Url::parse(url).unwrap(),
            file_name: "file.zip".to_owned(),
        }
    }

    #[test]
    fn remote_targets_use_protocol_default_ports_and_decode_paths() {
        let ftp = remote_target(&download("ftp://example.com/a%20file.zip")).unwrap();
        assert_eq!(ftp.port, 21);
        assert_eq!(ftp.path, "/a file.zip");

        let sftp = remote_target(&download("sftp://alice@example.com:2200/a.zip")).unwrap();
        assert_eq!(sftp.port, 2200);
        assert_eq!(sftp.username, "alice");
    }

    #[test]
    fn endpoint_keys_never_include_passwords() {
        let first = endpoint_key(&download("ftp://alice:one@example.com/a.zip")).unwrap();
        let second = endpoint_key(&download("ftp://alice:two@example.com/b.zip")).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn identity_files_expand_home_and_openssh_tokens() {
        let home = Path::new("/home/alice");
        assert_eq!(
            expand_identity_path(
                PathBuf::from("~/.ssh/%r@%h"),
                Some(home),
                "files.example.com",
                "alice",
            ),
            home.join(".ssh/alice@files.example.com")
        );
    }
}
