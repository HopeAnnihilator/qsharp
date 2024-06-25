// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module contains a project implementation using [`std::fs`].
//! Only a sync API is provided for now, because our binary targets
//! are only sync at the time of writing this (qsi and qsc).

use crate::{DirEntry, EntryType, FileSystem};
use miette::{Context, IntoDiagnostic};
use std::convert::Infallible;
use std::fs::DirEntry as StdEntry;
use std::path::Path;
use std::{path::PathBuf, sync::Arc};

/// This struct represents management of Q# projects from the [`std::fs`] filesystem implementation.
#[derive(Default)]
pub struct StdFs;

impl DirEntry for PathBuf {
    type Error = Infallible;

    fn entry_type(&self) -> Result<EntryType, Self::Error> {
        if self.is_file() {
            Ok(EntryType::File)
        } else if self.is_dir() {
            Ok(EntryType::Folder)
        } else if self.is_symlink() {
            Ok(EntryType::Symlink)
        } else {
            unreachable!()
        }
    }

    fn entry_extension(&self) -> String {
        self.extension()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    fn entry_name(&self) -> String {
        self.file_name()
            .expect("canonicalized symlink cannot end in relative path")
            .to_string_lossy()
            .to_string()
    }

    fn path(&self) -> PathBuf {
        self.clone()
    }
}

impl DirEntry for StdEntry {
    type Error = crate::Error;
    fn entry_type(&self) -> Result<EntryType, Self::Error> {
        Ok(self.file_type()?.into())
    }

    fn path(&self) -> PathBuf {
        self.path()
    }
}

impl std::convert::From<std::fs::FileType> for EntryType {
    fn from(file_type: std::fs::FileType) -> Self {
        if file_type.is_dir() {
            EntryType::Folder
        } else if file_type.is_file() {
            EntryType::File
        } else if file_type.is_symlink() {
            EntryType::Symlink
        } else {
            unreachable!()
        }
    }
}

impl FileSystem for StdFs {
    type Entry = StdEntry;

    fn read_file(&self, path: &Path) -> miette::Result<(Arc<str>, Arc<str>)> {
        let contents = std::fs::read_to_string(path)
            .into_diagnostic()
            .with_context(|| format!("could not read source file `{}`", path.display()))?;

        Ok((path.to_string_lossy().into(), contents.into()))
    }

    fn list_directory(&self, path: &Path) -> miette::Result<Vec<StdEntry>> {
        let listing = std::fs::read_dir(path).map_err(crate::Error::from)?;
        Ok(listing
            .collect::<Result<_, _>>()
            .map_err(crate::Error::from)?)
    }

    fn resolve_path(&self, base: &Path, path: &Path) -> miette::Result<PathBuf> {
        Ok(base.join(path))
    }

    fn fetch_github(
        &self,
        _owner: &str,
        _repo: &str,
        r#ref: &str,
        _path: &str,
    ) -> miette::Result<Arc<str>> {
        let _ = r#ref;
        Err(miette::Error::msg(
            "github references not supported for this file system",
        ))
    }
}
