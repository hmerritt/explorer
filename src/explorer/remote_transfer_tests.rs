//! Exercises real SFTP packets against a controllable, sandboxed server.
use super::*;
use russh_sftp::protocol::{
    Attrs, Data, File as ProtocolFile, Handle as ProtocolHandle, Name, Packet, Status, StatusCode,
    Version,
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Faults {
    fail_writes_after: AtomicU64,
    written: AtomicU64,
    fail_close: AtomicBool,
    lose_rename_reply: AtomicBool,
    atomic_rename: AtomicBool,
}
struct Server {
    root: PathBuf,
    handles: HashMap<String, File>,
    directories: HashMap<String, PathBuf>,
    listed: HashSet<String>,
    next: u64,
    faults: Arc<Faults>,
}
fn status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}
fn code(e: io::Error) -> StatusCode {
    match e.kind() {
        io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}
impl Server {
    fn path(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let path = Path::new(path.trim_start_matches('/'));
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            && !path.as_os_str().is_empty()
        {
            return Err(StatusCode::PermissionDenied);
        }
        Ok(self.root.join(path))
    }
    fn attrs(&self, path: &str) -> Result<FileAttributes, StatusCode> {
        let meta = fs::symlink_metadata(self.path(path)?).map_err(code)?;
        let mut attrs = FileAttributes::empty();
        attrs.size = Some(meta.len());
        attrs.permissions = Some(if meta.is_dir() {
            0o040755
        } else if meta.file_type().is_symlink() {
            0o120777
        } else {
            0o100644
        });
        attrs.mtime = Some(
            meta.modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32,
        );
        attrs.atime = attrs.mtime;
        Ok(attrs)
    }
}
impl russh_sftp::server::Handler for Server {
    type Error = StatusCode;
    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }
    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, StatusCode> {
        let mut v = Version::new();
        if self.faults.atomic_rename.load(Ordering::Relaxed) {
            v.extensions
                .insert("posix-rename@openssh.com".into(), "1".into());
        }
        Ok(v)
    }
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs {
            id,
            attrs: self.attrs(&path)?,
        })
    }
    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        self.lstat(id, path).await
    }
    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _: FileAttributes,
    ) -> Result<ProtocolHandle, StatusCode> {
        let mut options = OpenOptions::new();
        options
            .read(flags.contains(OpenFlags::READ))
            .write(flags.contains(OpenFlags::WRITE));
        if flags.contains(OpenFlags::EXCLUDE) {
            options.create_new(true);
        } else {
            options.create(flags.contains(OpenFlags::CREATE));
        }
        let file = options.open(self.path(&filename)?).map_err(code)?;
        self.next += 1;
        let handle = self.next.to_string();
        self.handles.insert(handle.clone(), file);
        Ok(ProtocolHandle { id, handle })
    }
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        let had_file = self.handles.remove(&handle).is_some();
        self.directories.remove(&handle);
        if had_file && self.faults.fail_close.load(Ordering::Relaxed) {
            Err(StatusCode::Failure)
        } else {
            Ok(status(id))
        }
    }
    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusCode> {
        let f = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        let mut data = vec![0; len as usize];
        let read = f.read(&mut data).map_err(code)?;
        data.truncate(read);
        if read == 0 {
            Err(StatusCode::Eof)
        } else {
            Ok(Data { id, data })
        }
    }
    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusCode> {
        let limit = self.faults.fail_writes_after.load(Ordering::Relaxed);
        if limit > 0 && self.faults.written.load(Ordering::Relaxed) >= limit {
            return Err(StatusCode::ConnectionLost);
        }
        let f = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        f.write_all(&data).map_err(code)?;
        self.faults
            .written
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(status(id))
    }
    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> Result<Status, StatusCode> {
        fs::create_dir(self.path(&path)?).map_err(code)?;
        Ok(status(id))
    }
    async fn opendir(&mut self, id: u32, path: String) -> Result<ProtocolHandle, StatusCode> {
        let path = self.path(&path)?;
        if !path.is_dir() {
            return Err(StatusCode::NoSuchFile);
        }
        self.next += 1;
        let handle = self.next.to_string();
        self.directories.insert(handle.clone(), path);
        Ok(ProtocolHandle { id, handle })
    }
    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        if !self.listed.insert(handle.clone()) {
            return Err(StatusCode::Eof);
        }
        let directory = self.directories.get(&handle).ok_or(StatusCode::Failure)?;
        let files = fs::read_dir(directory)
            .map_err(code)?
            .map(|e| {
                let e = e.map_err(code)?;
                let path = e
                    .path()
                    .strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                Ok(ProtocolFile::new(
                    e.file_name().to_string_lossy().as_ref(),
                    self.attrs(&path)?,
                ))
            })
            .collect::<Result<Vec<_>, StatusCode>>()?;
        Ok(Name { id, files })
    }
    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, StatusCode> {
        if let Some(t) = attrs.mtime {
            filetime::set_file_mtime(
                self.path(&path)?,
                filetime::FileTime::from_unix_time(t as i64, 0),
            )
            .map_err(code)?;
        }
        Ok(status(id))
    }
    async fn rename(&mut self, id: u32, old: String, new: String) -> Result<Status, StatusCode> {
        let old = self.path(&old)?;
        let new = self.path(&new)?;
        if new.exists() {
            return Err(StatusCode::Failure);
        }
        fs::rename(old, new).map_err(code)?;
        if self.faults.lose_rename_reply.swap(false, Ordering::Relaxed) {
            return Err(StatusCode::ConnectionLost);
        }
        Ok(status(id))
    }
    async fn extended(
        &mut self,
        id: u32,
        request: String,
        mut data: Vec<u8>,
    ) -> Result<Packet, StatusCode> {
        if request != "posix-rename@openssh.com"
            || !self.faults.atomic_rename.load(Ordering::Relaxed)
        {
            return Err(StatusCode::OpUnsupported);
        }
        let mut names = Vec::new();
        for _ in 0..2 {
            if data.len() < 4 {
                return Err(StatusCode::BadMessage);
            }
            let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
            data.drain(..4);
            if len > data.len() {
                return Err(StatusCode::BadMessage);
            }
            names.push(
                String::from_utf8(data.drain(..len).collect())
                    .map_err(|_| StatusCode::BadMessage)?,
            );
        }
        fs::rename(self.path(&names[0])?, self.path(&names[1])?).map_err(code)?;
        if self.faults.lose_rename_reply.swap(false, Ordering::Relaxed) {
            return Err(StatusCode::ConnectionLost);
        }
        Ok(Packet::Status(status(id)))
    }
    async fn remove(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
        fs::remove_file(self.path(&path)?).map_err(code)?;
        Ok(status(id))
    }
    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
        fs::remove_dir(self.path(&path)?).map_err(code)?;
        Ok(status(id))
    }
}
async fn server(root: &Path, faults: Arc<Faults>) -> Session {
    let (client, stream) = tokio::io::duplex(128 * 1024);
    tokio::spawn(russh_sftp::server::run(
        stream,
        Server {
            root: root.into(),
            handles: HashMap::new(),
            directories: HashMap::new(),
            listed: HashSet::new(),
            next: 0,
            faults,
        },
    ));
    let raw = russh_sftp::client::RawSftpSession::new(client);
    raw.set_timeout(2);
    let version = raw.init().await.unwrap();
    Session {
        endpoint: None,
        raw,
        extensions: version.extensions,
        chunk_size: 1024,
        _ssh: None,
    }
}
fn job(root: &Path, source: Location, destination: Location) -> Job {
    Job {
        store: root.join("job.json"),
        data: Mutex::new(Manifest {
            endpoint: None,
            version: 1,
            id: 42,
            sources: vec![source],
            destination,
            state: State::Queued,
            message: String::new(),
            bytes: 0,
            total: 0,
            current: 0,
            items: vec![],
            planned: false,
            move_sources: false,
            verify: true,
            conflict: Conflict::Ask,
            warnings: vec![],
        }),
        control: AtomicU8::new(0),
        running: AtomicBool::new(false),
    }
}
fn remote(path: &str) -> Location {
    Location::Remote(RemoteLocation::parse(&format!("sftp://fixture{path}")).unwrap())
}
fn faults() -> Arc<Faults> {
    let f = Arc::new(Faults::default());
    f.atomic_rename.store(true, Ordering::Relaxed);
    f
}

#[test]
fn recursive_upload_download_roundtrip_and_empty_files() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let remote_dir = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        fs::create_dir_all(local.path().join("folder/empty")).unwrap();
        fs::write(local.path().join("folder/a.txt"), vec![3; 8193]).unwrap();
        fs::write(local.path().join("folder/zero"), []).unwrap();
        let session = server(remote_dir.path(), faults()).await;
        let upload = job(
            output.path(),
            Location::Local(local.path().join("folder")),
            remote("/"),
        );
        run(&upload, &session).await.unwrap();
        assert_eq!(
            fs::read(remote_dir.path().join("folder/a.txt")).unwrap(),
            vec![3; 8193]
        );
        assert!(remote_dir.path().join("folder/empty").is_dir());
        let download = job(
            output.path(),
            remote("/folder"),
            Location::Local(output.path().into()),
        );
        run(&download, &session).await.unwrap();
        assert_eq!(
            fs::read(output.path().join("folder/a.txt")).unwrap(),
            vec![3; 8193]
        );
        assert_eq!(
            fs::metadata(output.path().join("folder/zero"))
                .unwrap()
                .len(),
            0
        );
    });
}
#[test]
fn interrupted_upload_resumes_only_validated_prefix() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), vec![7; 8192]).unwrap();
        let f = faults();
        f.fail_writes_after.store(2048, Ordering::Relaxed);
        let session = server(destination.path(), f.clone()).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        assert!(run(&job, &session).await.is_err());
        assert!(!destination.path().join("file").exists());
        let partial = destination.path().join(".explorer-42-0.filepart");
        assert_eq!(fs::metadata(&partial).unwrap().len(), 2048);
        f.fail_writes_after.store(0, Ordering::Relaxed);
        run(&job, &session).await.unwrap();
        assert_eq!(f.written.load(Ordering::Relaxed), 8192);
        assert_eq!(
            fs::read(destination.path().join("file")).unwrap(),
            vec![7; 8192]
        );
    });
}
#[test]
fn corrupted_partial_is_rejected_without_replacing_destination() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), vec![7; 8192]).unwrap();
        fs::write(destination.path().join("file"), b"original").unwrap();
        let f = faults();
        f.fail_writes_after.store(2048, Ordering::Relaxed);
        let session = server(destination.path(), f.clone()).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        job.data.lock().unwrap().conflict = Conflict::Replace;
        assert!(run(&job, &session).await.is_err());
        fs::write(
            destination.path().join(".explorer-42-0.filepart"),
            b"corrupt",
        )
        .unwrap();
        f.fail_writes_after.store(0, Ordering::Relaxed);
        assert!(
            run(&job, &session)
                .await
                .unwrap_err()
                .to_string()
                .contains("Partial content")
        );
        assert_eq!(
            fs::read(destination.path().join("file")).unwrap(),
            b"original"
        );
    });
}
#[test]
fn failed_close_never_publishes_a_file() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), b"new").unwrap();
        let f = faults();
        f.fail_close.store(true, Ordering::Relaxed);
        let session = server(destination.path(), f).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        assert!(run(&job, &session).await.is_err());
        assert!(!destination.path().join("file").exists());
    });
}
#[test]
fn unsupported_atomic_replace_preserves_original_and_completed_partial() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), b"new").unwrap();
        fs::write(destination.path().join("file"), b"original").unwrap();
        let f = faults();
        f.atomic_rename.store(false, Ordering::Relaxed);
        let session = server(destination.path(), f).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        job.data.lock().unwrap().conflict = Conflict::Replace;
        assert_eq!(
            run(&job, &session).await.unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            fs::read(destination.path().join("file")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(destination.path().join(".explorer-42-0.filepart")).unwrap(),
            b"new"
        );
    });
}
#[test]
fn lost_rename_reply_is_reconciled_after_reloading_manifest() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), b"new").unwrap();
        fs::write(destination.path().join("file"), b"original").unwrap();
        let f = faults();
        f.lose_rename_reply.store(true, Ordering::Relaxed);
        let session = server(destination.path(), f).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        job.data.lock().unwrap().conflict = Conflict::Replace;
        assert!(run(&job, &session).await.is_err());
        let stored = serde_json::from_slice(&fs::read(&job.store).unwrap()).unwrap();
        *job.data.lock().unwrap() = stored;
        run(&job, &session).await.unwrap();
        assert_eq!(job.data.lock().unwrap().current, 1);
        assert_eq!(fs::read(destination.path().join("file")).unwrap(), b"new");
    });
}
#[test]
fn destination_changed_during_interruption_is_not_overwritten() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), vec![7; 8192]).unwrap();
        fs::write(destination.path().join("file"), b"original").unwrap();
        let f = faults();
        f.fail_writes_after.store(2048, Ordering::Relaxed);
        let session = server(destination.path(), f.clone()).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        job.data.lock().unwrap().conflict = Conflict::Replace;
        assert!(run(&job, &session).await.is_err());
        fs::write(destination.path().join("file"), b"changed externally").unwrap();
        f.fail_writes_after.store(0, Ordering::Relaxed);
        assert!(
            run(&job, &session)
                .await
                .unwrap_err()
                .to_string()
                .contains("Destination changed")
        );
        assert_eq!(
            fs::read(destination.path().join("file")).unwrap(),
            b"changed externally"
        );
    });
}

#[test]
fn lost_new_file_rename_reply_is_reconciled() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), b"new").unwrap();
        let f = faults();
        f.lose_rename_reply.store(true, Ordering::Relaxed);
        let session = server(destination.path(), f).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        assert!(run(&job, &session).await.is_err());
        run(&job, &session).await.unwrap();
        assert_eq!(job.data.lock().unwrap().current, 1);
        assert_eq!(fs::read(destination.path().join("file")).unwrap(), b"new");
    });
}

#[test]
fn keep_both_recovers_when_atomic_replace_is_unsupported() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file.txt"), b"new").unwrap();
        fs::write(destination.path().join("file.txt"), b"original").unwrap();
        let f = faults();
        f.atomic_rename.store(false, Ordering::Relaxed);
        let session = server(destination.path(), f).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file.txt")),
            remote("/"),
        );
        job.data.lock().unwrap().conflict = Conflict::Replace;
        assert!(run(&job, &session).await.is_err());
        job.data.lock().unwrap().conflict = Conflict::KeepBoth;
        run(&job, &session).await.unwrap();
        assert_eq!(
            fs::read(destination.path().join("file.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(destination.path().join("file (2).txt")).unwrap(),
            b"new"
        );
        assert!(valid_manifest(&job.data.lock().unwrap()));
    });
}

#[test]
fn unowned_partial_is_never_adopted_on_retry() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), b"new").unwrap();
        fs::write(
            destination.path().join(".explorer-42-0.filepart"),
            b"someone else's file",
        )
        .unwrap();
        let session = server(destination.path(), faults()).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        for _ in 0..2 {
            assert!(
                run(&job, &session)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("unowned")
            );
        }
        assert!(job.data.lock().unwrap().items[0].partial.is_none());
        assert_eq!(
            fs::read(destination.path().join(".explorer-42-0.filepart")).unwrap(),
            b"someone else's file"
        );
    });
}

#[test]
fn changed_source_blocks_resume_and_preserves_destination() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(local.path().join("file"), vec![7; 8192]).unwrap();
        let f = faults();
        f.fail_writes_after.store(2048, Ordering::Relaxed);
        let session = server(destination.path(), f.clone()).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("file")),
            remote("/"),
        );
        assert!(run(&job, &session).await.is_err());
        fs::write(local.path().join("file"), b"different").unwrap();
        f.fail_writes_after.store(0, Ordering::Relaxed);
        assert!(
            run(&job, &session)
                .await
                .unwrap_err()
                .to_string()
                .contains("Source changed")
        );
        assert!(!destination.path().join("file").exists());
    });
}

#[test]
fn same_server_folder_move_uses_rename_and_recovers_lost_reply() {
    remote_fs::runtime().block_on(async {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("source/sub")).unwrap();
        fs::create_dir(root.path().join("destination")).unwrap();
        fs::write(root.path().join("source/sub/file"), b"contents").unwrap();
        let f = faults();
        f.lose_rename_reply.store(true, Ordering::Relaxed);
        let session = server(root.path(), f.clone()).await;
        let job = job(state.path(), remote("/source"), remote("/destination"));
        job.data.lock().unwrap().move_sources = true;
        assert!(run(&job, &session).await.is_err());
        run(&job, &session).await.unwrap();
        assert!(!root.path().join("source").exists());
        assert_eq!(
            fs::read(root.path().join("destination/source/sub/file")).unwrap(),
            b"contents"
        );
        assert_eq!(f.written.load(Ordering::Relaxed), 0);
        assert_eq!(job.data.lock().unwrap().items.len(), 1);
    });
}

#[test]
fn recursive_move_cleanup_is_idempotent_after_restart() {
    remote_fs::runtime().block_on(async {
        let local = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::create_dir_all(local.path().join("folder/sub")).unwrap();
        fs::write(local.path().join("folder/sub/file"), b"contents").unwrap();
        let session = server(destination.path(), faults()).await;
        let job = job(
            local.path(),
            Location::Local(local.path().join("folder")),
            remote("/"),
        );
        job.data.lock().unwrap().move_sources = true;
        run(&job, &session).await.unwrap();
        let saved = serde_json::from_slice(&fs::read(&job.store).unwrap()).unwrap();
        *job.data.lock().unwrap() = saved;
        run(&job, &session).await.unwrap();
        assert!(!local.path().join("folder").exists());
        assert_eq!(
            fs::read(destination.path().join("folder/sub/file")).unwrap(),
            b"contents"
        );
    });
}
