use std::borrow::Cow;
use std::{
    collections::HashSet,
    fs::{self},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use crate::decompressors::utils::normalize_mode;
use crate::{DecompressError, ExtractOpts, ObserveEvent};
use tar::Archive;

pub fn tar_list(out: &mut Archive<Box<dyn Read>>) -> Result<Vec<PathBuf>, DecompressError> {
    out.entries()?
        .map(|entry| entry.and_then(|entry| entry.path().map(Cow::into_owned)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(DecompressError::from)
}

pub fn tar_extract(
    out: &mut Archive<Box<dyn Read>>,
    to: &Path,
    opts: &ExtractOpts,
) -> Result<Vec<String>, DecompressError> {
    let mut files = vec![];
    let mut prepared_parents = HashSet::new();
    if !to.exists() {
        fs::create_dir_all(to)?;
        opts.observer.observe(ObserveEvent::DirectoryCreate);
    }

    // alternative impl: just unpack, and then mv everything back X levels
    for entry in out.entries()? {
        let entry = entry?;
        let filepath = entry.path()?;

        // strip prefixed components. this can be 0 parts, in which case strip does not happen.
        // it's done for when archives contain an enclosing folder
        let filepath = filepath.components().skip(opts.strip).collect::<PathBuf>();

        // because we potentially stripped a component, we may have an empty path, in which case
        // the joined target will be identical to the target folder
        // we take this approach to avoid hardcoding a check against empty ""
        let outpath = to.join(filepath);
        if to == outpath {
            continue;
        }

        if !(opts.filter)(outpath.as_path()) {
            continue;
        }

        let outpath: Cow<'_, Path> = (opts.map)(outpath.as_path());
        let is_directory = entry.header().entry_type() == tar::EntryType::Directory;
        opts.observer.observe(ObserveEvent::EntryStart {
            path: &outpath,
            is_directory,
        });

        if !is_directory {
            if let Some(p) = outpath.parent() {
                if prepared_parents.insert(p.to_path_buf()) {
                    fs::create_dir_all(p)?;
                    opts.observer.observe(ObserveEvent::DirectoryCreate);
                }
            }

            let mut outfile = fs::File::create(&outpath)?;
            opts.observer.observe(ObserveEvent::FileCreate);

            #[cfg(unix)]
            let h = entry.header().mode();

            let started = std::time::Instant::now();
            let bytes = io::copy(&mut BufReader::new(entry), &mut outfile)?;
            opts.observer.observe(ObserveEvent::OutputWrite {
                bytes,
                elapsed: started.elapsed(),
            });
            opts.observer.observe(ObserveEvent::EntryComplete {
                path: &outpath,
                bytes,
                is_directory: false,
            });
            if opts.collect_output_paths {
                files.push(outpath.to_string_lossy().to_string());
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mode) = h {
                    let mode = normalize_mode(mode);
                    fs::set_permissions(&outpath, fs::Permissions::from_mode(mode))?;
                    opts.observer.observe(ObserveEvent::MetadataOperation);
                }
            }
        } else {
            opts.observer.observe(ObserveEvent::EntryComplete {
                path: &outpath,
                bytes: 0,
                is_directory: true,
            });
        }
    }
    Ok(files)
}
